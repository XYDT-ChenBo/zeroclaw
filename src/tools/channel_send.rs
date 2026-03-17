use super::traits::{Tool, ToolResult};
use crate::config::Config;
use crate::cron::scheduler::deliver_announcement;
use crate::security::SecurityPolicy;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct ChannelSendTool {
    config: Arc<Config>,
    security: Arc<SecurityPolicy>,
}

impl ChannelSendTool {
    pub fn new(config: Arc<Config>, security: Arc<SecurityPolicy>) -> Self {
        Self { config, security }
    }
}

#[async_trait]
impl Tool for ChannelSendTool {
    fn name(&self) -> &str {
        "channel_send"
    }

    fn description(&self) -> &str {
        "Send a plain text message directly to a configured messaging channel (telegram/discord/slack/mattermost)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "enum": ["telegram", "discord", "slack", "mattermost"],
                    "description": "Channel type to deliver to"
                },
                "to": {
                    "type": "string",
                    "description": "Target: Discord channel ID, Telegram chat ID, Slack channel, etc."
                },
                "message": {
                    "type": "string",
                    "description": "Message content to send"
                }
            },
            "required": ["channel", "to", "message"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        if !self.security.can_act() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: autonomy is read-only".into()),
            });
        }

        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: rate limit exceeded".into()),
            });
        }

        let channel = args
            .get("channel")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing or empty 'channel' parameter"))?;

        let target = args
            .get("to")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing or empty 'to' parameter"))?;

        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing or empty 'message' parameter"))?;

        if let Err(e) = deliver_announcement(&self.config, channel, target, message).await {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to send message: {e}")),
            });
        }

        Ok(ToolResult {
            success: true,
            output: "message sent".to_string(),
            error: None,
        })
    }
}

