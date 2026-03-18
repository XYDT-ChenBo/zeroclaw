//! A2A HTTP endpoints backed by `ra2a`.

use super::AppState;
use anyhow::Result;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};

use parking_lot::RwLock;
use std::sync::OnceLock;
use ra2a::server::{AgentExecutor, Event, EventQueue, RequestContext, ServerState};
use ra2a::types::{AgentCapabilities, AgentCard, AgentSkill, Message, Part, Task, TaskState, TaskStatus};
use std::{future::Future, pin::Pin};
use crate::tools::traits::ToolSpec;

const METHOD_MESSAGE_STREAM: &str = "message/stream";
const METHOD_TASKS_RESUBSCRIBE: &str = "tasks/resubscribe";


static A2A_SERVER_STATE: OnceLock<RwLock<Option<ra2a::server::ServerState>>> = OnceLock::new();

fn a2a_server_state_cell() -> &'static RwLock<Option<ra2a::server::ServerState>> {
    A2A_SERVER_STATE.get_or_init(|| RwLock::new(None))
}

fn current_a2a_server_state() -> Option<ra2a::server::ServerState> {
    a2a_server_state_cell().read().clone()
}

fn build_agent_skills(tool_specs: &[ToolSpec]) -> Vec<AgentSkill> {
    let mut skills = Vec::with_capacity(tool_specs.len() + 1);
    skills.push(AgentSkill::new(
        "chat",
        "Conversational Assistant",
        "Answer user requests with tool/memory assisted execution when needed.",
        vec![
            "chat".to_string(),
            "assistant".to_string(),
            "reasoning".to_string(),
        ],
    ));
    skills
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

pub fn init(
    config: &crate::config::Config,
    base_url: &str,
    tool_specs: &[ToolSpec],
) -> Result<()> {

    struct ZeroClawExecutor {
        config_template: crate::config::Config,
    }

    impl AgentExecutor for ZeroClawExecutor {
        fn execute<'a>(
            &'a self,
            ctx: &'a RequestContext,
            queue: &'a EventQueue,
        ) -> Pin<Box<dyn Future<Output = ra2a::Result<()>> + Send + 'a>> {
            Box::pin(async move {
                let mut working = ctx
                    .stored_task
                    .clone()
                    .unwrap_or_else(|| Task::new(&ctx.task_id, &ctx.context_id));
                if let Some(message) = ctx.message.clone() {
                    working.history.push(message);
                }
                working.status = TaskStatus::new(TaskState::Working);
                queue.send(Event::Task(working))?;

                let input = ctx
                    .message
                    .as_ref()
                    .and_then(Message::text_content)
                    .unwrap_or_default();
                let output = crate::agent::process_message(
                    self.config_template.clone(),
                    input.trim(),
                    None,
                    Some(&ctx.context_id),
                )
                .await;

                let mut task = ctx
                    .stored_task
                    .clone()
                    .unwrap_or_else(|| Task::new(&ctx.task_id, &ctx.context_id));
                if let Some(message) = ctx.message.clone() {
                    task.history.push(message);
                }

                match output {
                    Ok(answer) => {
                        let reply = Message::agent(vec![Part::text(answer)])
                            .with_task_id(&ctx.task_id)
                            .with_context_id(&ctx.context_id);
                        task.history.push(reply.clone());
                        task.status = TaskStatus::with_message(TaskState::Completed, reply);
                    }
                    Err(error) => {
                        task.status = TaskStatus::failed(error.to_string());
                    }
                }
                queue.send(Event::Task(task))?;
                Ok(())
            })
        }

        fn cancel<'a>(
            &'a self,
            ctx: &'a RequestContext,
            queue: &'a EventQueue,
        ) -> Pin<Box<dyn Future<Output = ra2a::Result<()>> + Send + 'a>> {
            Box::pin(async move {
                let mut task = ctx
                    .stored_task
                    .clone()
                    .unwrap_or_else(|| Task::new(&ctx.task_id, &ctx.context_id));
                task.status = TaskStatus::new(TaskState::Canceled);
                queue.send(Event::Task(task))?;
                Ok(())
            })
        }
    }

    let mut card = AgentCard::new("ZeroClaw A2A Agent", join_url(base_url, "/a2a"));
    card.description = "ZeroClaw A2A entrypoint powered by ra2a (v0.3.0 integration)".to_string();
    card.version = env!("CARGO_PKG_VERSION").to_string();
    card.capabilities = AgentCapabilities {
        streaming: config.gateway.a2a.stream_enabled,
        state_transition_history: true,
        ..AgentCapabilities::default()
    };
    card.skills = build_agent_skills(tool_specs);

    let server_state = ServerState::from_executor(
        ZeroClawExecutor {
            config_template: config.clone(),
        },
        card,
    );
    *a2a_server_state_cell().write() = Some(server_state);
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/a2a/.well-known/agent-card.json", get(handle_a2a_agent_card))
        .route("/a2a", post(handle_a2a_rpc))
}

fn is_authorized(state: &AppState, headers: &HeaderMap) -> bool {
    if !state.pairing.require_pairing() {
        return true;
    }

    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
        .unwrap_or("");
    state.pairing.is_authenticated(token)
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "Unauthorized — pair first via POST /pair, then send Authorization: Bearer <token>"
        })),
    )
        .into_response()
}

fn not_enabled_response() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "A2A runtime is disabled in gateway.a2a.enabled"
        })),
    )
        .into_response()
}

fn rpc_method_name(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("method")
                .and_then(|m| m.as_str())
                .map(ToOwned::to_owned)
        })
}

fn is_streaming_method(method: &str) -> bool {
    matches!(method, METHOD_MESSAGE_STREAM | METHOD_TASKS_RESUBSCRIBE)
}

fn streaming_disabled_response(method: &str) -> Response {
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": -32601,
                "message": format!("Method '{method}' is disabled on this server")
            },
            "id": serde_json::Value::Null
        })),
    )
        .into_response()
}

pub async fn handle_a2a_agent_card(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.lock().gateway.a2a.enabled {
        return not_enabled_response();
    }

    if let Some(server_state) = current_a2a_server_state() {
        return ra2a::server::handle_agent_card(State(server_state))
            .await
            .into_response();
    }

    not_enabled_response()
}

/// POST /a2a — unified A2A endpoint.
/// Non-streaming methods are dispatched as JSON-RPC responses; streaming methods
/// (`message/stream`, `tasks/resubscribe`) are dispatched as SSE on the same URL.
pub async fn handle_a2a_rpc(
    State(state): State<AppState>,
    headers: HeaderMap,
    _body: String,
) -> impl IntoResponse {
    if !is_authorized(&state, &headers) {
        return unauthorized_response();
    }
    if !state.config.lock().gateway.a2a.enabled {
        return not_enabled_response();
    }

    if let Some(server_state) = current_a2a_server_state() {
        if let Some(method) = rpc_method_name(&_body) {
            if is_streaming_method(&method) {
                if !state.config.lock().gateway.a2a.stream_enabled {
                    return streaming_disabled_response(&method);
                }
                return ra2a::server::handle_sse(State(server_state), headers, _body).await;
            }
        }
        return ra2a::server::handle_jsonrpc(State(server_state), headers, _body).await;
    }

    not_enabled_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Memory, MemoryCategory, MemoryEntry};
    use crate::providers::Provider;
    use crate::security::pairing::PairingGuard;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    #[derive(Default)]
    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            Ok(format!("echo: {message}"))
        }
    }

    #[derive(Default)]
    struct MockMemory;

    #[async_trait]
    impl Memory for MockMemory {
        fn name(&self) -> &str {
            "mock"
        }

        async fn store(
            &self,
            _key: &str,
            _content: &str,
            _category: MemoryCategory,
            _session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }

        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }

        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }

        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            Ok(0)
        }

        async fn health_check(&self) -> bool {
            true
        }
    }

    fn test_state(a2a_enabled: bool, require_pairing: bool) -> AppState {
        let mut config = crate::config::Config::default();
        config.gateway.a2a.enabled = a2a_enabled;
        config.gateway.a2a.stream_enabled = true;

        if a2a_enabled {
            let _ = init(&config, "http://127.0.0.1:42617", &[]);
        }

        let existing_tokens = if require_pairing {
            vec!["zc_test_token".to_string()]
        } else {
            Vec::new()
        };

        AppState {
            config: Arc::new(Mutex::new(config)),
            provider: Arc::new(MockProvider),
            model: "test-model".into(),
            temperature: 0.0,
            mem: Arc::new(MockMemory),
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(require_pairing, &existing_tokens)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(crate::gateway::GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(crate::gateway::IdempotencyStore::new(
                Duration::from_secs(300),
                1000,
            )),
            whatsapp: None,
            whatsapp_app_secret: None,
            linq: None,
            linq_signing_secret: None,
            nextcloud_talk: None,
            nextcloud_talk_webhook_secret: None,
            wati: None,
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            shutdown_tx: tokio::sync::watch::channel(false).0,
            node_registry: None,
        }
    }

    #[tokio::test]
    async fn a2a_routes_return_not_implemented_when_disabled() {
        let app = router().with_state(test_state(false, false));
        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":"1","method":"message/send","params":{}}"#,
            ))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn a2a_routes_require_pairing_token_when_enabled() {
        let app = router().with_state(test_state(true, true));
        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":"1","method":"message/send","params":{}}"#,
            ))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a2a_message_send_and_task_get_flow_works() {
        let app = router().with_state(test_state(true, false));

        let send_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "send-1",
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": "msg-1",
                    "role": "user",
                    "parts": [{ "kind": "text", "text": "hello a2a" }]
                }
            }
        });
        let send_req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(send_body.to_string()))
            .unwrap();
        let send_res = app.clone().oneshot(send_req).await.unwrap();
        assert_eq!(send_res.status(), StatusCode::OK);
        let send_bytes = send_res.into_body().collect().await.unwrap().to_bytes();
        let send_json: serde_json::Value = serde_json::from_slice(&send_bytes).unwrap();

        let task_id = send_json["result"]["id"]
            .as_str()
            .expect("task id should be present")
            .to_string();
        assert_eq!(send_json["jsonrpc"], "2.0");

        let get_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "get-1",
            "method": "tasks/get",
            "params": { "id": task_id, "historyLength": 10 }
        });
        let get_req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(get_body.to_string()))
            .unwrap();
        let get_res = app.oneshot(get_req).await.unwrap();
        assert_eq!(get_res.status(), StatusCode::OK);
        let get_bytes = get_res.into_body().collect().await.unwrap().to_bytes();
        let get_json: serde_json::Value = serde_json::from_slice(&get_bytes).unwrap();
        assert_eq!(get_json["jsonrpc"], "2.0");
        assert_eq!(get_json["result"]["kind"], "task");
    }

    #[tokio::test]
    async fn a2a_message_stream_uses_unified_a2a_endpoint() {
        let app = router().with_state(test_state(true, false));
        let stream_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "stream-1",
            "method": "message/stream",
            "params": {
                "message": {
                    "messageId": "msg-stream-1",
                    "role": "user",
                    "parts": [{ "kind": "text", "text": "hello stream" }]
                }
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(Body::from(stream_body.to_string()))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_lowercase();
        assert!(
            content_type.contains("text/event-stream"),
            "expected SSE response from unified /a2a endpoint, got content-type={content_type}"
        );
    }

    #[tokio::test]
    async fn a2a_agent_card_exposes_capabilities_and_skills() {
        let app = router().with_state(test_state(true, false));
        let req = Request::builder()
            .method("GET")
            .uri("/a2a/.well-known/agent-card.json")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["capabilities"]["stateTransitionHistory"], true);
        assert_eq!(json["capabilities"]["streaming"], true);
        assert!(
            json["skills"].as_array().is_some_and(|skills| !skills.is_empty()),
            "skills should include at least the base chat capability"
        );
    }
}
