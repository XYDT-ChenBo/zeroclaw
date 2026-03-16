use super::traits::{Tool, ToolResult};
use crate::config::{BroadcastAllowRule, BroadcastConfig, BroadcastExtraType};
use crate::runtime::RuntimeAdapter;
use crate::security::SecurityPolicy;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_BROADCAST_TIMEOUT_SECS: u64 = 10;

/// Generic broadcast tool backed by Android `am broadcast`.
///
/// This tool does not interpret payloads beyond validating them against an
/// allowlist; higher-level skills are responsible for constructing extras
/// such as `remindInfo` for alarm integrations.
pub struct BroadcastTool {
    config: BroadcastConfig,
    security: Arc<SecurityPolicy>,
    runtime: Arc<dyn RuntimeAdapter>,
}

impl BroadcastTool {
    pub fn new(
        config: BroadcastConfig,
        security: Arc<SecurityPolicy>,
        runtime: Arc<dyn RuntimeAdapter>,
    ) -> Self {
        Self {
            config,
            security,
            runtime,
        }
    }

    fn validate_shell_token(label: &'static str, value: &str) -> Result<()> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Broadcast tool: {label} is empty");
        }
        if trimmed.chars().any(char::is_whitespace) {
            anyhow::bail!("Broadcast tool: {label} must not contain whitespace");
        }
        let allowed = trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
        if !allowed {
            anyhow::bail!("Broadcast tool: {label} contains unsupported characters");
        }
        Ok(())
    }

    /// Escape JSON/string for safe use inside single-quoted shell argument.
    fn escape_for_shell_single_quoted(json_str: &str) -> String {
        let mut out = String::with_capacity(json_str.len() + 8);
        for c in json_str.chars() {
            if c == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(c);
            }
        }
        out
    }

    fn find_rule<'a>(&'a self, package: &str, action: &str) -> Option<&'a BroadcastAllowRule> {
        self.config
            .allowlist
            .iter()
            .find(|rule| rule.package == package && rule.action == action)
    }

    fn validate_extras(
        rule: &BroadcastAllowRule,
        extras: &serde_json::Map<String, Value>,
    ) -> Result<Vec<(String, BroadcastExtraType, Value)>> {
        let mut validated = Vec::with_capacity(extras.len());
        for (k, v) in extras {
            let Some(extra_type) = rule.extras.get(k) else {
                anyhow::bail!("Broadcast tool: extra key '{k}' not allowed for this action");
            };
            match extra_type {
                BroadcastExtraType::String | BroadcastExtraType::JsonString => {
                    if !v.is_string() {
                        anyhow::bail!(
                            "Broadcast tool: extra '{k}' expected string value (including JSON string)"
                        );
                    }
                }
                BroadcastExtraType::Int => {
                    if !v.is_i64() && !v.is_u64() {
                        anyhow::bail!("Broadcast tool: extra '{k}' expected integer value");
                    }
                }
                BroadcastExtraType::Bool => {
                    if !v.is_boolean() {
                        anyhow::bail!("Broadcast tool: extra '{k}' expected boolean value");
                    }
                }
            }
            validated.push((k.clone(), extra_type.clone(), v.clone()));
        }
        Ok(validated)
    }

    async fn run_broadcast(
        &self,
        package: &str,
        action: &str,
        extras: Option<serde_json::Map<String, Value>>,
    ) -> Result<ToolResult> {
        if !self.runtime.has_shell_access() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Runtime has no shell access; cannot run am broadcast".into()),
            });
        }

        let am_binary = self.config.am_binary.trim();
        Self::validate_shell_token("am_binary", am_binary)?;
        Self::validate_shell_token("package", package)?;
        Self::validate_shell_token("action", action)?;

        let Some(rule) = self.find_rule(package, action) else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Broadcast tool: (package, action) not allowlisted".into()),
            });
        };

        let validated_extras = if let Some(map) = extras {
            Self::validate_extras(rule, &map)?
        } else {
            Vec::new()
        };

        let mut cmd_str = format!(
            "{} broadcast -a {} -p {}",
            am_binary,
            action.trim(),
            package.trim()
        );

        for (key, ty, value) in validated_extras {
            match ty {
                BroadcastExtraType::String | BroadcastExtraType::JsonString => {
                    let s = value.as_str().unwrap_or_default();
                    let escaped = Self::escape_for_shell_single_quoted(s);
                    cmd_str.push_str(&format!(" --es {} '{}'", key, escaped));
                }
                BroadcastExtraType::Int => {
                    let n = value.as_i64().unwrap_or(0);
                    cmd_str.push_str(&format!(" --ei {} {}", key, n));
                }
                BroadcastExtraType::Bool => {
                    let b = value.as_bool().unwrap_or(false);
                    cmd_str.push_str(&format!(" --ez {} {}", key, b));
                }
            }
        }

        // Whitelist: only our configured am_binary and fixed broadcast form.
        if !cmd_str.starts_with(am_binary) || !cmd_str.contains(" broadcast ") {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Broadcast tool: invalid command shape".into()),
            });
        }

        tracing::info!("broadcast cmd: {}", cmd_str);
        let mut cmd = self
            .runtime
            .build_shell_command(&cmd_str, &self.security.workspace_dir)?;
        cmd.env_clear();

        let timeout_secs = if self.config.timeout_secs == 0 {
            DEFAULT_BROADCAST_TIMEOUT_SECS
        } else {
            self.config.timeout_secs
        };

        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await;
        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let err_msg = if stderr.is_empty() {
                    None
                } else {
                    Some(stderr)
                };
                if !output.status.success() {
                    return Ok(ToolResult {
                        success: false,
                        output: stdout,
                        error: err_msg.or_else(|| {
                            Some("am broadcast failed. On Termux, install TermuxAm so `am` is in PATH.".into())
                        }),
                    });
                }
                Ok(ToolResult {
                    success: true,
                    output: if stdout.is_empty() {
                        "Broadcast sent.".into()
                    } else {
                        stdout
                    },
                    error: err_msg,
                })
            }
            Ok(Err(e)) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to execute am broadcast: {e}")),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("am broadcast timed out after {}s", timeout_secs)),
            }),
        }
    }
}

#[async_trait]
impl Tool for BroadcastTool {
    fn name(&self) -> &str {
        "broadcast"
    }

    fn description(&self) -> &str {
        "Send Android broadcast intents via `am broadcast`, constrained by a strict allowlist of \
         (package, action, extras). Typically used by higher-level skills such as alarm \
         integrations that construct JSON payloads like `remindInfo`."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        use serde_json::json;

        let mut packages: Vec<String> = self
            .config
            .allowlist
            .iter()
            .map(|r| r.package.clone())
            .collect();
        packages.sort();
        packages.dedup();

        let mut actions: Vec<String> = self
            .config
            .allowlist
            .iter()
            .map(|r| r.action.clone())
            .collect();
        actions.sort();
        actions.dedup();

        // Extras schema: union of all configured extras; runtime will still
        // enforce per-(package, action) rules.
        let mut extras_props = serde_json::Map::new();
        for rule in &self.config.allowlist {
            for (name, ty) in &rule.extras {
                if extras_props.contains_key(name) {
                    continue;
                }
                let (type_str, desc) = match ty {
                    BroadcastExtraType::String => ("string", "String extra passed via --es"),
                    BroadcastExtraType::Int => ("integer", "Integer extra passed via --ei"),
                    BroadcastExtraType::Bool => ("boolean", "Boolean extra passed via --ez"),
                    BroadcastExtraType::JsonString => (
                        "string",
                        "JSON-encoded string extra (tool will not inspect its structure)",
                    ),
                };
                extras_props.insert(
                    name.clone(),
                    json!({
                        "type": type_str,
                        "description": desc
                    }),
                );
            }
        }

        json!({
            "type": "object",
            "properties": {
                "package": {
                    "type": "string",
                    "enum": packages,
                    "description": "Target package name. Must match an allowlisted rule."
                },
                "action": {
                    "type": "string",
                    "enum": actions,
                    "description": "Broadcast action. Must match an allowlisted rule."
                },
                "extras": {
                    "type": "object",
                    "description": "Extras to attach to the broadcast intent. Keys and value types must match the allowlist.",
                    "properties": extras_props,
                    "additionalProperties": false
                }
            },
            "required": ["package", "action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let package = args
            .get("package")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing 'package' parameter"))?;
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;
        let extras = args.get("extras").and_then(Value::as_object).cloned();

        self.run_broadcast(package, action, extras).await
    }
}
