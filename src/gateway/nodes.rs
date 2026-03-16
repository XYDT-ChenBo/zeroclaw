//! Connected-node registry: holds WebSocket node sessions and pending request/response.

use async_trait::async_trait;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::{
    sync::{mpsc, oneshot},
    time::{self, MissedTickBehavior},
};
use uuid::Uuid;
use crate::gateway::AppState;

/// Default timeout for invoke/run when waiting for node response.
pub const DEFAULT_NODE_INVOKE_TIMEOUT_SECS: u64 = 60;


/// Trait for the connected-node registry. Implemented by the gateway when
/// node-control is enabled; used by [`NodesTool`] and HTTP node-control API.
#[async_trait]
pub trait NodeRegistry: Send + Sync {
    /// List all connected nodes (optionally filtered by allowlist elsewhere).
    fn list(&self) -> Vec<NodeInfo>;

    /// Describe one node by id; None if not connected.
    fn describe(&self, node_id: &str) -> Option<NodeDescription>;

    /// Send a structured invoke to the node; waits for response with timeout.
    async fn invoke(
        &self,
        node_id: &str,
        capability: &str,
        arguments: Value,
    ) -> anyhow::Result<NodeCommandResult>;

    /// Send a raw command (e.g. shell) to the node; waits for response with timeout.
    async fn run(&self, node_id: &str, raw_command: &str) -> anyhow::Result<NodeCommandResult>;
}

/// Info for one connected node (list entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub status: String,
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Full description for a single node (describe).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDescription {
    pub node_id: String,
    pub status: String,
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Result of an invoke or run command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCommandResult {
    pub success: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Message sent from registry to the connection handler to be forwarded to the node.
#[derive(Debug)]
pub enum OutgoingMessage {
    Invoke {
        request_id: String,
        capability: String,
        arguments: Value,
    },
    Run {
        request_id: String,
        command: String,
    },
}

/// One connected node session: channel to send invoke/run to the handler.
struct NodeSession {
    node_id: String,
    capabilities: Vec<String>,
    meta: Option<Value>,
    tx: mpsc::Sender<OutgoingMessage>,
}

/// In-memory registry of connected nodes; implements [`NodeRegistry`].
pub struct ConnectedNodeRegistry {
    sessions: RwLock<HashMap<String, NodeSession>>,
    pending: Mutex<HashMap<String, oneshot::Sender<NodeCommandResult>>>,
    invoke_timeout: Duration,
}

static CONNECTED_NODE_REGISTRY: OnceLock<Arc<ConnectedNodeRegistry>> = OnceLock::new();

impl ConnectedNodeRegistry {
    /// Get global singleton instance of connected-node registry.
    pub fn global() -> Arc<ConnectedNodeRegistry> {
        CONNECTED_NODE_REGISTRY
            .get_or_init(|| Arc::new(ConnectedNodeRegistry::new()))
            .clone()
    }

    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(DEFAULT_NODE_INVOKE_TIMEOUT_SECS))
    }

    pub fn with_timeout(invoke_timeout: Duration) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            invoke_timeout,
        }
    }

    /// Register a node (called from WebSocket handler after receiving register message).
    /// Returns a receiver for outgoing messages the handler must forward to the node.
    pub fn register(
        &self,
        node_id: String,
        capabilities: Vec<String>,
        meta: Option<Value>,
    ) -> mpsc::Receiver<OutgoingMessage> {
        let (tx, rx) = mpsc::channel(32);
        let session = NodeSession {
            node_id: node_id.clone(),
            capabilities: capabilities.clone(),
            meta: meta.clone(),
            tx,
        };
        self.sessions.write().insert(node_id, session);
        rx
    }

    /// Remove a node (called when WebSocket disconnects).
    /// In-flight requests for this node will time out in the caller.
    pub fn unregister(&self, node_id: &str) {
        self.sessions.write().remove(node_id);
    }

    /// Complete a pending request (called from WebSocket handler on invoke_result/run_result).
    pub fn complete_pending(&self, request_id: &str, result: NodeCommandResult) {
        if let Some(tx) = self.pending.lock().remove(request_id) {
            let _ = tx.send(result);
        }
    }

    /// Filter list by allowlist when non-empty; pass empty slice to allow all.
    fn filter_by_allowlist(&self, allowed_node_ids: &[String]) -> Vec<NodeInfo> {
        let list = self.list_inner();
        if allowed_node_ids.is_empty() {
            return list;
        }
        let allowed: std::collections::HashSet<_> = allowed_node_ids
            .iter()
            .filter(|s| *s != "*")
            .collect();
        if allowed.is_empty() {
            return list;
        }
        list.into_iter()
            .filter(|n| allowed.contains(&n.node_id))
            .collect()
    }
}

impl Default for ConnectedNodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectedNodeRegistry {
    fn list_inner(&self) -> Vec<NodeInfo> {
        self.sessions
            .read()
            .values()
            .map(|s| NodeInfo {
                node_id: s.node_id.clone(),
                status: "connected".to_string(),
                capabilities: s.capabilities.clone(),
                meta: s.meta.clone(),
            })
            .collect()
    }
}

#[async_trait]
impl NodeRegistry for ConnectedNodeRegistry {
    fn list(&self) -> Vec<NodeInfo> {
        self.list_inner()
    }

    fn describe(&self, node_id: &str) -> Option<NodeDescription> {
        self.sessions.read().get(node_id).map(|s| NodeDescription {
            node_id: s.node_id.clone(),
            status: "connected".to_string(),
            capabilities: s.capabilities.clone(),
            meta: s.meta.clone(),
        })
    }

    async fn invoke(
        &self,
        node_id: &str,
        capability: &str,
        arguments: Value,
    ) -> anyhow::Result<NodeCommandResult> {
        let tx = self
            .sessions
            .read()
            .get(node_id)
            .map(|s| s.tx.clone())
            .ok_or_else(|| anyhow::anyhow!("node '{node_id}' not connected"))?;

        let request_id = uuid::Uuid::new_v4().to_string();
        let (resp_tx, resp_rx) = oneshot::channel();
        self.pending.lock().insert(request_id.clone(), resp_tx);

        let msg = OutgoingMessage::Invoke {
            request_id: request_id.clone(),
            capability: capability.to_string(),
            arguments,
        };
        tx.send(msg)
            .await
            .map_err(|_| anyhow::anyhow!("node '{node_id}' channel closed"))?;

        match tokio::time::timeout(self.invoke_timeout, resp_rx).await {
            Ok(Ok(res)) => Ok(res),
            Ok(Err(_)) => {
                self.pending.lock().remove(&request_id);
                Ok(NodeCommandResult {
                    success: false,
                    output: String::new(),
                    error: Some("request cancelled".into()),
                })
            }
            Err(_) => {
                self.pending.lock().remove(&request_id);
                Ok(NodeCommandResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "timeout after {}s waiting for node response",
                        self.invoke_timeout.as_secs()
                    )),
                })
            }
        }
    }

    async fn run(&self, node_id: &str, raw_command: &str) -> anyhow::Result<NodeCommandResult> {
        let tx = self
            .sessions
            .read()
            .get(node_id)
            .map(|s| s.tx.clone())
            .ok_or_else(|| anyhow::anyhow!("node '{node_id}' not connected"))?;

        let request_id = uuid::Uuid::new_v4().to_string();
        let (resp_tx, resp_rx) = oneshot::channel();
        self.pending.lock().insert(request_id.clone(), resp_tx);

        let msg = OutgoingMessage::Run {
            request_id: request_id.clone(),
            command: raw_command.to_string(),
        };
        tx.send(msg)
            .await
            .map_err(|_| anyhow::anyhow!("node '{node_id}' channel closed"))?;

        match tokio::time::timeout(self.invoke_timeout, resp_rx).await {
            Ok(Ok(res)) => Ok(res),
            Ok(Err(_)) => {
                self.pending.lock().remove(&request_id);
                Ok(NodeCommandResult {
                    success: false,
                    output: String::new(),
                    error: Some("request cancelled".into()),
                })
            }
            Err(_) => {
                self.pending.lock().remove(&request_id);
                Ok(NodeCommandResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "timeout after {}s waiting for node response",
                        self.invoke_timeout.as_secs()
                    )),
                })
            }
        }
    }
}

/// Extension: list filtered by allowlist (for HTTP API).
impl ConnectedNodeRegistry {
    pub fn list_filtered(&self, allowed_node_ids: &[String]) -> Vec<NodeInfo> {
        self.filter_by_allowlist(allowed_node_ids)
    }

    pub fn describe_filtered(
        &self,
        node_id: &str,
        allowed_node_ids: &[String],
    ) -> Option<NodeDescription> {
        if !allowed_node_ids.is_empty()
            && !allowed_node_ids.iter().any(|a| a == "*" || a == node_id)
        {
            return None;
        }
        self.describe(node_id)
    }
}


// --------------------------------------------------
// nodes websocket
// --------------------------------------------------

/// Interval for gateway → node tick events (milliseconds).
/// Matches OpenClaw's TICK_INTERVAL_MS so shared clients can reuse watchdog logic.
const NODE_TICK_INTERVAL_MS: u64 = 30_000;

fn sanitize_ws_headers(headers: &HeaderMap) -> Value {
    let mut out = serde_json::Map::new();
    for (key, value) in headers {
        let k = key.as_str().to_ascii_lowercase();
        let v = value.to_str().unwrap_or("<non-utf8>");
        let redacted = matches!(
            k.as_str(),
            "authorization" | "x-node-control-token" | "cookie" | "set-cookie"
        );
        out.insert(
            key.as_str().to_string(),
            Value::String(if redacted {
                "<redacted>".to_string()
            } else {
                v.to_string()
            }),
        );
    }
    Value::Object(out)
}
/// GET / — WebSocket upgrade for node connections.
pub async fn handle_ws_nodes(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    tracing::info!(
        peer = %peer_addr,
        headers = %sanitize_ws_headers(&headers),
        "node websocket upgrade request received"
    );

    let registry = state.node_registry.clone();
    let Some(registry) = registry else {
        tracing::warn!(
            peer = %peer_addr,
            "node websocket rejected: node_control is disabled"
        );
        return (
            StatusCode::NOT_FOUND,
            "Node WebSocket is disabled (node_control.enabled = false)",
        )
            .into_response();
    };

    // Optional token check: header X-Node-Control-Token
    let config = state.config.lock().nodes.clone();
    if let Some(expected) = config.auth_token.as_deref().filter(|s: &&str| !s.is_empty()) {
        let provided = headers
            .get("X-Node-Control-Token")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .unwrap_or("");
        if !crate::security::pairing::constant_time_eq(expected, provided) {
            tracing::warn!(
                peer = %peer_addr,
                "node websocket rejected: invalid X-Node-Control-Token"
            );
            return (StatusCode::UNAUTHORIZED, "Invalid X-Node-Control-Token").into_response();
        }
    }

    tracing::info!(peer = %peer_addr, "node websocket upgrade accepted");
    ws.on_upgrade(move |socket| handle_node_socket(socket, registry, peer_addr))
        .into_response()
}

async fn handle_node_socket(
    mut socket: WebSocket,
    registry: std::sync::Arc<ConnectedNodeRegistry>,
    peer_addr: SocketAddr,
) {
    // ── Step 1: send connect.challenge ─────────────────────────────
    let nonce = Uuid::new_v4().to_string();
    let challenge = serde_json::json!({
        "type": "event",
        "event": "connect.challenge",
        "payload": { "nonce": nonce },
    });
    if socket
        .send(Message::Text(challenge.to_string().into()))
        .await
        .is_err()
    {
        tracing::warn!("failed to send connect.challenge to node");
        return;
    }

    // ── Step 2: wait for connect request ───────────────────────────
    let (node_id, mut out_rx) = loop {
        let msg = match socket.recv().await {
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(Message::Close(frame))) => {
                tracing::info!(?frame, "node websocket closed before connect");
                return;
            }
            Some(Err(error)) => {
                tracing::warn!(error = %error, "node websocket recv error before connect");
                return;
            }
            None => {
                tracing::info!("node websocket stream ended before connect");
                return;
            }
            _ => continue,
        };

        let parsed: serde_json::Value = match serde_json::from_str(&msg) {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!(payload = %msg, "node websocket invalid JSON before connect");
                continue;
            }
        };

        let frame_type = parsed["type"].as_str().unwrap_or("");
        let method = parsed["method"].as_str().unwrap_or("");
        if frame_type != "req" || method != "connect" {
            tracing::warn!(
                payload = %parsed,
                "node websocket expected connect request (type=req, method=connect)"
            );
            continue;
        }

        let connect_id = match parsed["id"].as_str().map(str::trim) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => {
                tracing::warn!(payload = %parsed, "node connect request missing id");
                continue;
            }
        };

        let params = parsed
            .get("params")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        // Role 分流：只接受 role = "node" 的连接，其它（例如 "operator"）直接拒绝。
        let role = params
            .get("role")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("node");

        if role != "node" {
            tracing::warn!(
                role = %role,
                params = %params,
                "node websocket connect rejected: unsupported role"
            );

            let connect_res = serde_json::json!({
                "type": "res",
                "id": connect_id,
                "ok": false,
                "error": {
                    "code": "invalid_request",
                    "message": format!("unsupported role: {role} (only 'node' is allowed)"),
                }
            });

            if let Err(error) = socket
                .send(Message::Text(connect_res.to_string().into()))
                .await
            {
                tracing::warn!(
                    role = %role,
                    error = %error,
                    "failed to send role rejection response to websocket client"
                );
            }

            return;
        }

        // Prefer device.id as node_id when present, otherwise fallback to client.id.
        let device_id = params
            .get("device")
            .and_then(|d| d.get("id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let client_id = params
            .get("client")
            .and_then(|c| c.get("id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let node_id = match device_id.or(client_id) {
            Some(id) => id,
            None => {
                tracing::warn!(payload = %params, "node connect params missing client.id/device.id");
                continue;
            }
        };

        // Caps + Commands as capabilities
        let mut capabilities: Vec<String> = Vec::new();
        if let Some(caps) = params.get("caps").and_then(|v| v.as_array()) {
            for v in caps {
                if let Some(s) = v.as_str().map(str::trim) {
                    if !s.is_empty() {
                        capabilities.push(s.to_string());
                    }
                }
            }
        }
        if let Some(commands) = params.get("commands").and_then(|v| v.as_array()) {
            for v in commands {
                if let Some(s) = v.as_str().map(str::trim) {
                    if !s.is_empty() {
                        capabilities.push(s.to_string());
                    }
                }
            }
        }

        let mut meta = params.clone();
        if let Some(obj) = meta.as_object_mut() {
            obj.insert(
                "remoteIp".to_string(),
                serde_json::Value::String(peer_addr.ip().to_string()),
            );
        }
        let meta = Some(meta);

        let rx = registry.register(node_id.clone(), capabilities, meta);
        tracing::info!(
            node_id = %node_id,
            connect_params = %params,
            "node connected via gateway protocol"
        );

        // Acknowledge connect (minimal ok=true response).
        let connect_res = serde_json::json!({
            "type": "res",
            "id": connect_id,
            "ok": true,
            "payload": {
                "nodeId": node_id,
            },
        });
        if socket
            .send(Message::Text(connect_res.to_string().into()))
            .await
            .is_err()
        {
            tracing::warn!(node_id = %node_id, "failed to send connect response to node");
            return;
        }

        break (node_id, rx);
    };

    // Periodic keepalive: send `event: "tick"` frames to the node so
    // node-side watchdogs can detect stalled connections.
    let mut tick_interval = time::interval(Duration::from_millis(NODE_TICK_INTERVAL_MS));
    tick_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Incoming from node: node.invoke.result (req frame)
            msg = socket.recv() => {
                let msg = match msg {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(frame))) => {
                        tracing::info!(node_id = %node_id, ?frame, "node websocket closed");
                        break;
                    }
                    Some(Err(error)) => {
                        tracing::warn!(node_id = %node_id, error = %error, "node websocket recv error");
                        break;
                    }
                    None => {
                        tracing::info!(node_id = %node_id, "node websocket stream ended");
                        break;
                    }
                    _ => continue,
                };

                let parsed: serde_json::Value = match serde_json::from_str(&msg) {
                    Ok(v) => v,
                    Err(_) => {
                        tracing::warn!(node_id = %node_id, payload = %msg, "node sent invalid JSON");
                        continue;
                    }
                };

                let frame_type = parsed["type"].as_str().unwrap_or("");
                let method = parsed["method"].as_str().unwrap_or("");

                if frame_type == "req" && method == "node.invoke.result" {
                    let params = parsed.get("params").cloned().unwrap_or(serde_json::json!({}));
                    let request_id = params["id"].as_str().unwrap_or("").to_string();
                    if request_id.is_empty() {
                        tracing::warn!(node_id = %node_id, payload = %params, "node.invoke.result missing id");
                        continue;
                    }

                    let ok = params["ok"].as_bool().unwrap_or(false);
                    let payload_json = params["payloadJSON"].as_str().map(String::from);
                    let payload_value = params.get("payload").cloned();
                    let error_value = params.get("error").cloned();

                    let output = if let Some(pj) = payload_json {
                        pj
                    } else if let Some(pv) = payload_value {
                        pv.to_string()
                    } else {
                        String::new()
                    };

                    let error = error_value.map(|e| e.to_string());

                    let result = NodeCommandResult {
                        success: ok,
                        output,
                        error,
                    };

                    tracing::info!(
                        node_id = %node_id,
                        request_id = %request_id,
                        success = result.success,
                        "node invoke result received"
                    );
                    registry.complete_pending(&request_id, result);
                } else {
                    tracing::debug!(
                        node_id = %node_id,
                        frame_type = %frame_type,
                        method = %method,
                        payload = %parsed,
                        "ignoring unsupported node frame"
                    );
                }
            }

            // Outgoing to node: invoke or run from registry
            out_msg = out_rx.recv() => {
                let out = match out_msg {
                    Some(m) => m,
                    None => break,
                };

                let wire = match &out {
                    OutgoingMessage::Invoke { request_id, capability, arguments } => {
                        // Map tool "invoke" to node.invoke.request; capability becomes command.
                        let params_json = arguments.to_string();
                        tracing::info!(
                            node_id = %node_id,
                            request_id = %request_id,
                            capability = %capability,
                            params_json = %params_json,
                            "sending node.invoke.request to node"
                        );
                        let payload = serde_json::json!({
                            "id": request_id,
                            "nodeId": node_id,
                            "command": capability,
                            "paramsJSON": params_json,
                            "timeoutMs": 15_000i64,
                        });
                        serde_json::json!({
                            "type": "event",
                            "event": "node.invoke.request",
                            "payload": payload
                        })
                    }
                    OutgoingMessage::Run { request_id, command } => {
                        // Map tool "run" to system.run command on node side.
                        let params = serde_json::json!({
                            "command": Vec::<String>::new(),
                            "rawCommand": command,
                        });
                        let params_json = params.to_string();
                        tracing::info!(
                            node_id = %node_id,
                            request_id = %request_id,
                            raw_command = %command,
                            "sending node.invoke.request(system.run) to node"
                        );
                        let payload = serde_json::json!({
                            "id": request_id,
                            "nodeId": node_id,
                            "command": "system.run",
                            "paramsJSON": params_json,
                            "timeoutMs": 15_000i64,
                        });
                        serde_json::json!({
                            "type": "event",
                            "event": "node.invoke.request",
                            "payload": payload
                        })
                    }
                };

                if socket.send(Message::Text(wire.to_string().into())).await.is_err() {
                    tracing::warn!(node_id = %node_id, "failed to send gateway frame to node");
                    break;
                }
            }

            // Periodic tick: gateway-initiated heartbeat for node clients.
            _ = tick_interval.tick() => {
                let payload = serde_json::json!({
                    "ts": chrono::Utc::now().timestamp_millis(),
                });
                let tick = serde_json::json!({
                    "type": "event",
                    "event": "tick",
                    "payload": payload,
                });

                if socket.send(Message::Text(tick.to_string().into())).await.is_err() {
                    tracing::warn!(node_id = %node_id, "failed to send tick event to node");
                    break;
                }
            }
        }
    }

    registry.unregister(&node_id);
    tracing::info!(node_id = %node_id, "node disconnected");
}

