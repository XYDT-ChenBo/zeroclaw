//! Weather tool: query weather and air quality (and future endpoints) via configured APIs.
//!
//! Uses `[weather]` config: endpoints (e.g. weather, air_quality), stationid (stationid), api_key.
//! Only requests URLs listed in config (allowlist). Returns raw API JSON to the LLM.
//!
//! Return value reference: output is the API response body as-is. For structured parsing or
//! field documentation, see your upstream API docs or add response types in this module.

use super::http_request::HttpRequestTool;
use super::traits::{Tool, ToolResult};
use crate::config::WeatherConfig;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// Tool that queries weather and air quality (and extensible endpoints) via configured APIs.
pub struct WeatherTool {
    config: WeatherConfig,
    http_tool: Arc<HttpRequestTool>,
}

impl WeatherTool {
    pub fn new(config: WeatherConfig, http_tool: Arc<HttpRequestTool>) -> Self {
        Self { config, http_tool }
    }

    fn endpoint_url_for(&self, query_type: &str) -> Result<String> {
        self.config
            .endpoints
            .get(query_type)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown or unconfigured query_type: {}", query_type))
    }

    fn validate_endpoint_url(url: &reqwest::Url) -> Result<()> {
        match url.scheme() {
            "http" | "https" => Ok(()),
            other => anyhow::bail!("Unsupported URL scheme for weather endpoint: {other}"),
        }
    }

    async fn execute_query(&self, query_type: &str, stationid: Option<&str>) -> Result<ToolResult> {
        let url_str = self.endpoint_url_for(query_type)?;
        let mut url = reqwest::Url::parse(&url_str)
            .map_err(|e| anyhow::anyhow!("Invalid endpoint URL: {e}"))?;
        Self::validate_endpoint_url(&url)?;
        let mut extra = Vec::new();
        if let Some(stationid) = stationid.or(self.config.stationid.as_deref()) {
            // Upstream APIs (e.g. Beijing Weather Online) use `stationid` as the location key.
            extra.push(format!("stationid={}", urlencoding::encode(stationid)));
        }
        if let Some(ref key) = self.config.api_key {
            extra.push(format!("key={}", urlencoding::encode(key)));
        }
        if !extra.is_empty() {
            let joined = extra.join("&");
            let query_str = match url.query() {
                Some(q) if !q.is_empty() => format!("{}&{}", q, joined),
                _ => joined,
            };
            url.set_query(Some(&query_str));
        }
        // Delegate the actual HTTP request to the shared HttpRequestTool so that
        // domain allowlisting, proxy configuration, timeouts and response size limits
        // are all enforced consistently in a single place.
        let final_url: String = url.into();
        let args = serde_json::json!({
            "url": final_url,
            "method": "GET",
        });
        self.http_tool.execute(args).await
    }
}

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "weather"
    }

    fn description(&self) -> &str {
        "Fetch raw weather/air-quality JSON via configured [weather] endpoints. \
         CRITICAL PRECONDITION: Before calling this tool, you MUST read and follow the weather Skill end-to-end.
            Only call this tool AFTER the Skill has produced a valid stationid (or explicitly decided to use the configured default).
        FORBIDDEN: guessing/deriving stationid from patterns; outputting raw JSON to the user.
        OUTPUT: After fetching, summarize in Chinese strictly per the weather Skill format."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let mut query_types: Vec<String> = self.config.endpoints.keys().cloned().collect();
        query_types.sort();
        serde_json::json!({
            "type": "object",
            "properties": {
                "query_type": {
                    "type": "string",
                    "enum": query_types,
                    "description": "Query type key; must be one of the configured [weather.endpoints] entries (e.g. weather, air_quality, forecast15d)"
                },
                "stationid": {
                    "type": "string",
                    "description": "Station id (single stationid or up to 20 stationids joined by '|'); optional; falls back to configured [weather].stationid when omitted"
                }
            },
            "required": ["query_type"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let query_type = args
            .get("query_type")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing 'query_type' parameter"))?;
        let stationid = args
            .get("stationid")
            .and_then(Value::as_str)
            .map(String::from);
        self.execute_query(query_type, stationid.as_deref()).await
    }
}
