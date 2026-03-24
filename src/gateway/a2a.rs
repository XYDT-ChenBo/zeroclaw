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
