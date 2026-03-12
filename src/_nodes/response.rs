use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::{sse::Event, IntoResponse, Json},
};
use chrono::Utc;
use crate::channels::http::{http_register_non_streaming, http_register_streaming, http_send};
use crate::channels::traits::ChannelMessage;
use crate::gateway::AppState;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{once, StreamExt};
use uuid::Uuid;

/// OpenAI-compatible chat message.
#[derive(Deserialize)]
pub struct OpenAiChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
}

/// OpenAI-compatible `/v1/chat/completions` request (subset).
#[derive(Deserialize)]
pub struct HttpChatRequest {
    /// Model override. Falls back to configured default model when omitted.
    pub model: Option<String>,
    /// Conversation history in OpenAI-compatible format.
    pub messages: Vec<OpenAiChatMessage>,
    /// When true, stream OpenAI-style SSE chunks instead of a single JSON response.
    #[serde(default)]
    pub stream: bool,
    /// Optional session identifier for multi-turn conversations.
    pub session_id: Option<String>,
}

/// POST /response — HTTP agent chat (non-streaming, single-turn)
pub async fn handle_http_response(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<HttpChatRequest>,
) -> impl IntoResponse {
    // Auth via Authorization header (same pairing model as WebSocket chat).
    if state.pairing.require_pairing() {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|auth| auth.strip_prefix("Bearer "))
            .map(str::trim)
            .unwrap_or("");
        if !state.pairing.is_authenticated(token) {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Unauthorized — provide Authorization: Bearer <token>"
                })),
            )
                .into_response();
        }
    }

    if body.messages.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "messages must not be empty"
            })),
        )
            .into_response();
    }

    // 使用最后一条 user 消息作为本轮输入，多轮上下文由 channel 的
    // conversation_histories 通过 session_id 维护。
    let user_content = body
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.trim())
        .unwrap_or("")
        .to_string();

    if user_content.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "last user message content must not be empty"
            })),
        )
            .into_response();
    }

    // 使用完整 Agent 流程（包含 tools 和 skills），并支持按请求覆盖模型。
    let mut config = state.config.lock().clone();
    if let Some(model) = &body.model {
        if !model.trim().is_empty() {
            config.default_model = Some(model.trim().to_string());
        }
    }

    let provider_label = config
        .default_provider
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let model_label = config
        .default_model
        .clone()
        .unwrap_or_else(|| "anthropic/claude-sonnet-4-20250514".into());

    // Broadcast agent_start event
    let _ = state.event_tx.send(serde_json::json!({
        "type": "agent_start",
        "provider": provider_label,
        "model": model_label,
    }));

    let created = Utc::now().timestamp();
    let id = format!("chatcmpl-{}", Uuid::new_v4().simple());

    // 将 session_id 作为 sender，让 channel 使用相同的会话 key 复用历史。
    let session = body
        .session_id
        .as_deref()
        .unwrap_or("http-default-agent-session")
        .to_string();
    let request_id = Uuid::new_v4().simple().to_string();

    // 非流式：优先通过 HttpChannel 走 channel 流水线，失败时回退到 agent。
    if !body.stream {
        let response_rx = http_register_non_streaming(request_id.clone()).await;

        let msg = ChannelMessage {
            id: id.clone(),
            sender: session.clone(),
            reply_target: request_id.clone(),
            content: user_content.clone(),
            channel: "http".to_string(),
            timestamp: created as u64,
            thread_ts: None,
        };

        if http_send(msg).await.is_ok() {
            match response_rx.await {
                Ok(response_text) => {
                    let body = serde_json::json!({
                        "id": id,
                        "object": "chat.completion",
                        "created": created,
                        "model": model_label,
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": response_text,
                            },
                            "finish_reason": "stop",
                        }],
                    });
                    return Json(body).into_response();
                }
                Err(e) => {
                    let sanitized =
                        crate::providers::sanitize_api_error(&format!("http channel error: {e}"));
                    return (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": sanitized })),
                    )
                        .into_response();
                }
            }
        }

        // 回退：仍然可以直接走 agent，实现兼容 gateway-only 场景。
        let result = crate::agent::process_message(config, &user_content, None).await;

        return match result {
            Ok(response_text) => {
                let body = serde_json::json!({
                    "id": id,
                    "object": "chat.completion",
                    "created": created,
                    "model": model_label,
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": response_text,
                        },
                        "finish_reason": "stop",
                    }],
                });
                Json(body).into_response()
            }
            Err(e) => {
                let sanitized = crate::providers::sanitize_api_error(&format!("{e}"));

                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": sanitized,
                    })),
                )
                    .into_response()
            }
        };
    }

    // 流式：优先通过 HttpChannel 的 draft 机制复用 channel 逻辑。
    let stream_rx = http_register_streaming(request_id.clone(), 64).await;

    let msg = ChannelMessage {
        id: id.clone(),
        sender: session,
        reply_target: request_id,
        content: user_content.clone(),
        channel: "http".to_string(),
        timestamp: created as u64,
        thread_ts: None,
    };

    if http_send(msg).await.is_err() {
        // 回退到原来的 agent streaming 实现。
        let (tx, rx) = mpsc::channel::<String>(16);
        let config_for_agent = config.clone();
        let user_content_for_agent = user_content.clone();
        let provider_for_agent = provider_label.clone();
        let model_for_agent = model_label.clone();
        let event_tx = state.event_tx.clone();

        tokio::spawn(async move {
            let _ = crate::agent::process_message_with_stream(
                config_for_agent,
                &user_content_for_agent,
                None,
                Some(tx),
            )
            .await;

            let _ = event_tx.send(serde_json::json!({
                "type": "agent_end",
                "provider": provider_for_agent,
                "model": model_for_agent,
            }));
        });

        let model_label_for_stream = model_label.clone();
        let stream = ReceiverStream::new(rx)
            .map(move |chunk| {
                let payload = serde_json::json!({
                    "id": id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": model_label_for_stream,
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "role": "assistant",
                            "content": chunk,
                        },
                        "finish_reason": null,
                    }],
                });
                Ok::<Event, axum::Error>(Event::default().data(payload.to_string()))
            })
            .chain(once(Ok::<Event, axum::Error>(
                Event::default().data("[DONE]"),
            )));

        return axum::response::Sse::new(stream).into_response();
    }

    let model_label_for_stream = model_label.clone();
    let stream = ReceiverStream::new(stream_rx)
        .map(move |chunk| {
            let payload = serde_json::json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model_label_for_stream,
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "content": chunk,
                    },
                    "finish_reason": null,
                }],
            });
            Ok::<Event, axum::Error>(Event::default().data(payload.to_string()))
        })
        .chain(once(Ok::<Event, axum::Error>(
            Event::default().data("[DONE]"),
        )));

    axum::response::Sse::new(stream).into_response()
}
