use super::traits::{Channel, ChannelMessage, SendMessage};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::future;
use std::sync::{LazyLock, OnceLock};
use tokio::sync::{mpsc, oneshot, Mutex};

/// Inbound bus for HTTP channel messages (`ChannelMessage` from /response handler).
static HTTP_INBOUND_TX: OnceLock<mpsc::Sender<ChannelMessage>> = OnceLock::new();

/// Per-request responders for HTTP replies (non-streaming and streaming).
static HTTP_RESPONDERS: LazyLock<Mutex<HashMap<String, HttpResponder>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

enum HttpResponder {
    NonStreaming(oneshot::Sender<String>),
    Streaming {
        tx: mpsc::Sender<String>,
        // Track the number of characters that have already been sent for this
        // streaming response so we can safely compute deltas without slicing
        // the UTF-8 string at a non‑char boundary.
        last_len: usize,
    },
}

/// Send a `ChannelMessage` into the HTTP channel bus.
pub async fn http_send(msg: ChannelMessage) -> Result<()> {
    if let Some(tx) = HTTP_INBOUND_TX.get() {
        tx.send(msg).await.map_err(|e| anyhow::anyhow!(e))
    } else {
        Err(anyhow::anyhow!(
            "HTTP channel bus not initialized (daemon channels not running)"
        ))
    }
}

/// Register a non-streaming responder for the given request id.
pub async fn http_register_non_streaming(id: String) -> oneshot::Receiver<String> {
    let (tx, rx) = oneshot::channel();
    let mut map = HTTP_RESPONDERS.lock().await;
    map.insert(id, HttpResponder::NonStreaming(tx));
    rx
}

/// Register a streaming responder for the given request id.
pub async fn http_register_streaming(id: String, cap: usize) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel(cap);
    let mut map = HTTP_RESPONDERS.lock().await;
    map.insert(
        id,
        HttpResponder::Streaming {
            tx,
            last_len: 0,
        },
    );
    rx
}

/// HTTP virtual channel — used by `/response` to reuse the channel pipeline.
#[derive(Clone)]
pub struct HttpChannel;

impl HttpChannel {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Channel for HttpChannel {
    fn name(&self) -> &str {
        "http"
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        // Non-streaming: final reply for this request id (recipient).
        let mut map = HTTP_RESPONDERS.lock().await;
        if let Some(HttpResponder::NonStreaming(tx)) = map.remove(&message.recipient) {
            let _ = tx.send(message.content.clone());
        }
        Ok(())
    }

    async fn listen(&self, tx: mpsc::Sender<ChannelMessage>) -> Result<()> {
        let _ = HTTP_INBOUND_TX.set(tx);
        future::pending::<()>().await;
        Ok(())
    }

    fn supports_draft_updates(&self) -> bool {
        true
    }

    async fn send_draft(&self, message: &SendMessage) -> Result<Option<String>> {
        // Draft id = recipient (request id)
        Ok(Some(message.recipient.clone()))
    }

    async fn update_draft(
        &self,
        _recipient: &str,
        message_id: &str,
        text: &str,
    ) -> Result<()> {
        let delta_opt = {
            let mut map = HTTP_RESPONDERS.lock().await;
            if let Some(HttpResponder::Streaming { tx, last_len }) = map.get_mut(message_id) {
                let prev_chars = *last_len;
                let total_chars = text.chars().count();
                if total_chars > prev_chars {
                    let delta: String = text.chars().skip(prev_chars).collect();
                    *last_len = total_chars;
                    Some((tx.clone(), delta))
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some((tx, delta)) = delta_opt {
            let _ = tx.send(delta).await;
        }
        Ok(())
    }

    async fn finalize_draft(
        &self,
        _recipient: &str,
        message_id: &str,
        text: &str,
    ) -> Result<()> {
        let entry = {
            let mut map = HTTP_RESPONDERS.lock().await;
            map.remove(message_id)
        };

        if let Some(HttpResponder::Streaming { tx, last_len }) = entry {
            let prev_chars = last_len;
            let total_chars = text.chars().count();
            if total_chars > prev_chars {
                let delta: String = text.chars().skip(prev_chars).collect();
                let _ = tx.send(delta).await;
            }
            drop(tx);
        }
        Ok(())
    }

    async fn cancel_draft(&self, _recipient: &str, message_id: &str) -> Result<()> {
        let mut map = HTTP_RESPONDERS.lock().await;
        map.remove(message_id);
        Ok(())
    }
}

