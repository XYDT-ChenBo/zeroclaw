use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::{sse::Event, IntoResponse, Json, Response},
};
use chrono::Utc;
use serde::Deserialize;
use tokio::sync::mpsc;
use futures_util::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::gateway::AppState;

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
    /// Optional session ID for conversation continuity. When set, history is loaded from and
    /// saved to workspace/sessions/{session_id}/history_conversation.json.
    pub session_id: Option<String>,
}

/// 已验证并准备完毕的请求上下文，供流式/非流式分支复用
struct ValidatedRequest {
    user_content: String,
    session_id: Option<String>,
    config: crate::config::Config,
    provider_label: String,
    model_label: String,
    id: String,
    created: i64,
}

/// 执行鉴权、校验和准备工作，成功返回 ValidatedRequest，失败返回错误响应
fn validate_and_prepare(
    state: &AppState,
    headers: &HeaderMap,
    body: &HttpChatRequest,
) -> Result<ValidatedRequest, Response> {
    if state.pairing.require_pairing() {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|auth| auth.strip_prefix("Bearer "))
            .map(str::trim)
            .unwrap_or("");
        if !state.pairing.is_authenticated(token) {
            return Err((
                axum::http::StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Unauthorized — provide Authorization: Bearer <token>"
                })),
            )
                .into_response());
        }
    }

    if body.messages.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "messages must not be empty"
            })),
        )
            .into_response());
    }

    let user_content = body
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.trim())
        .unwrap_or("")
        .to_string();

    if user_content.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "last user message content must not be empty"
            })),
        )
            .into_response());
    }

    let session_id = body
        .session_id
        .as_deref()
        .or(Some("http-agent-default-session"))
        .and_then(super::session_id::sanitize)
        .map(String::from);

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

    let created = Utc::now().timestamp();
    let id = format!("chatcmpl-{}", Uuid::new_v4().simple());

    Ok(ValidatedRequest {
        user_content,
        session_id,
        config,
        provider_label,
        model_label,
        id,
        created,
    })
}

/// 非流式响应：单次 process_message，返回完整 JSON
async fn respond_non_streaming(
    state: AppState,
    req: ValidatedRequest,
) -> Response {
    let _ = state.event_tx.send(serde_json::json!({
        "type": "agent_start",
        "provider": req.provider_label,
        "model": req.model_label,
    }));

    let result =
        crate::agent::process_message(
            req.config,
            &req.user_content,
            None,
            req.session_id.as_deref(),
        )
        .await;

    match result {
        Ok(response_text) => {
            // Broadcast agent_end event
            let _ = state.event_tx.send(serde_json::json!({
                "type": "agent_end",
                "provider": req.provider_label,
                "model": req.model_label,
            }));

            let body = serde_json::json!({
                "id": req.id,
                "object": "chat.completion",
                "created": req.created,
                "model": req.model_label,
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
            let _ = state.event_tx.send(serde_json::json!({
                "type": "error",
                "component": "http_chat",
                "message": sanitized,
            }));
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": sanitized })),
            )
                .into_response()
        }
    }
}

/// 流式响应：process_message_with_stream + SSE 流
async fn respond_streaming(
    state: AppState,
    req: ValidatedRequest,
) -> Response {
    let _ = state.event_tx.send(serde_json::json!({
        "type": "agent_start",
        "provider": req.provider_label,
        "model": req.model_label,
    }));

    let (tx, rx) = mpsc::channel::<String>(16);
    let config = req.config.clone();
    let user_content = req.user_content.clone();
    let provider_label = req.provider_label.clone();
    let model_label = req.model_label.clone();
    let model_label_for_stream = req.model_label.clone();
    let event_tx = state.event_tx.clone();
    let session_id = req.session_id.clone();

    tokio::spawn(async move {
        let _ = crate::agent::process_message_with_stream(
            config,
            &user_content,
            session_id.as_deref(),
            None,
            Some(tx)
        )
        .await;
        let _ = event_tx.send(serde_json::json!({
            "type": "agent_end",
            "provider": provider_label,
            "model": model_label,
        }));
    });

    let id = req.id.clone();
    let created = req.created;
    let stream = ReceiverStream::new(rx)
        .enumerate()
        .map({
            let id = id.clone();
            let created = created;
            let model_label = model_label_for_stream.clone();
            move |(idx, chunk)| {
                let delta = if idx == 0 {
                    serde_json::json!({ "role": "assistant", "content": chunk })
                } else {
                    serde_json::json!({ "content": chunk })
                };
                let payload = serde_json::json!({
                    "id": id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": model_label,
                    "choices": [{
                        "index": 0,
                        "delta": delta,
                        "finish_reason": null,
                    }],
                });
                Ok::<Event, axum::Error>(Event::default().data(payload.to_string()))
            }
        })
        .chain(tokio_stream::once({
            let payload = serde_json::json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model_label_for_stream,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop",
                }],
            });
            Ok::<Event, axum::Error>(Event::default().data(payload.to_string()))
        }))
        .chain(tokio_stream::once(Ok::<Event, axum::Error>(
            Event::default().data("[DONE]"),
        )));

    axum::response::Sse::new(stream).into_response()
}

/// POST /response — HTTP agent chat (流式与非流式统一入口)
pub async fn handle_http_response(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<HttpChatRequest>,
) -> Response {
    let req = match validate_and_prepare(&state, &headers, &body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    if body.stream {
        respond_streaming(state, req).await
    } else {
        respond_non_streaming(state, req).await
    }
}
