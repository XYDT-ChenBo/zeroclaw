use crate::config::schema::GuardrailConfig;
use crate::hooks::traits::{HookHandler, HookResult};
use crate::providers::{ChatMessage, ToolCall};
use anyhow::Result;
use async_trait::async_trait;
use reqwest;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::warn;

#[derive(Clone)]
pub struct GuardrailHook {
    config: GuardrailConfig,
}

impl GuardrailHook {
    pub fn new(config: GuardrailConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl HookHandler for GuardrailHook {
    fn name(&self) -> &str {
        "guardrail"
    }

    fn priority(&self) -> i32 {
        100
    }

    /// 用户输入检测（LLM 调用前）
    async fn before_llm_call(
        &self,
        messages: Vec<ChatMessage>,
        model: String,
    ) -> HookResult<(Vec<ChatMessage>, String)> {
        if !self.config.enabled || !self.config.check_user_input {
            return HookResult::Continue((messages, model));
        }

        let prompt = assemble_prompt(&messages);
        match check(&self.config, &prompt, "userQuery").await {
            Ok(true) => HookResult::Continue((messages, model)),
            Ok(false) => HookResult::Cancel("用户输入包含敏感内容，请修改后重试".to_string()),
            Err(e) => {
                warn!(error = %e, "guardrail user input check failed");
                HookResult::Cancel("安全检测服务暂不可用".to_string())
            }
        }
    }

    /// 模型输出检测（LLM 返回后，工具解析前）
    async fn before_llm_output(
        &self,
        response_text: String,
        tool_calls: Vec<ToolCall>,
    ) -> HookResult<(String, Vec<ToolCall>)> {
        if !self.config.enabled || !self.config.check_model_output {
            return HookResult::Continue((response_text, tool_calls));
        }

        match check(&self.config, &response_text, "modelResponse").await {
            Ok(true) => HookResult::Continue((response_text, tool_calls)),
            Ok(false) => HookResult::Cancel("模型输出包含敏感内容，已被拦截".to_string()),
            Err(e) => {
                warn!(error = %e, "guardrail model output check failed");
                HookResult::Cancel("输出安全检测服务暂不可用".to_string())
            }
        }
    }
}

/// 调用外部围栏接口
async fn check(config: &GuardrailConfig, content: &str, content_type: &str) -> Result<bool> {
    let client = reqwest::Client::new();
    let req = GuardrailRequest {
        tenant_id: config.tenant_id,
        robot_code: config.robot_code.clone(),
        robot_type: config.robot_type.clone(),
        content: content.to_string(),
        content_type: content_type.to_string(),
    };

    let resp = client
        .post(&config.url)
        .json(&req)
        .timeout(Duration::from_secs(config.timeout_secs))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Ok(false); // fail-closed
    }

    let body: GuardrailResponse = resp.json().await?;
    Ok(body.data.security_check_status)
}

/// 将 messages 组装为 prompt 文本
fn assemble_prompt(messages: &[ChatMessage]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        let role = match msg.role.as_str() {
            "system" => "SYSTEM",
            "user" => "USER",
            "assistant" => "ASSISTANT",
            "tool" => "TOOL",
            _ => &msg.role.to_uppercase(),
        };
        parts.push(format!("{}: {}", role, msg.content));
    }
    parts.join("\n")
}

/// 请求/响应类型定义
#[derive(Serialize)]
struct GuardrailRequest {
    #[serde(rename = "tenantId")]
    tenant_id: i64,
    #[serde(rename = "robotCode")]
    robot_code: String,
    #[serde(rename = "robotType")]
    robot_type: String,
    content: String,
    #[serde(rename = "contentType")]
    content_type: String,
}

#[derive(Deserialize)]
struct GuardrailResponse {
    code: String,
    message: String,
    #[serde(rename = "data")]
    data: GuardrailData,
}

#[derive(Deserialize)]
struct GuardrailData {
    #[serde(rename = "securityCheckStatus")]
    security_check_status: bool,
    #[serde(rename = "commonAnomalyTags")]
    common_anomaly_tags: Vec<AnomalyTag>,
}

#[derive(Deserialize)]
struct AnomalyTag {
    #[serde(rename = "firstLevelTagName")]
    first_level: String,
    #[serde(rename = "secondLevelTagName")]
    second_level: String,
    #[serde(rename = "thirdLevelTagName")]
    third_level: String,
}
