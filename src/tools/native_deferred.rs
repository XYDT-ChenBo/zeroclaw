//! Deferred loading for built-in (non-MCP) tools.
//!
//! When [`crate::config::schema::AgentConfig::native_deferred_loading_enabled`] is enabled,
//! tools not in the keep-list are removed from the initial registry and surfaced
//! as stubs; the LLM loads full schemas via `tool_search` / `select:`.

use std::collections::HashSet;
use std::sync::Arc;

use crate::tools::traits::{Tool, ToolSpec};

/// Default built-in tool names that are active by default.
pub fn default_active_native_tool_names() -> HashSet<String> {
    [
        "file_read",
        "skill_read"
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Build final active native tool set from defaults + config.
pub fn build_active_native_tool_set(config_keep: &[String]) -> HashSet<String> {
    let mut s = default_active_native_tool_names();
    for n in config_keep {
        let t = n.trim();
        if !t.is_empty() {
            s.insert(t.to_string());
        }
    }
    s
}

/// A deferred built-in tool: full implementation is held behind activation.
#[derive(Clone)]
pub struct DeferredNativeToolStub {
    pub name: String,
    pub description: String,
    tool: Arc<dyn Tool>,
}

impl DeferredNativeToolStub {
    pub fn new(tool: Arc<dyn Tool>) -> Self {
        Self {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            tool,
        }
    }

    pub fn tool_arc(&self) -> Arc<dyn Tool> {
        Arc::clone(&self.tool)
    }

    pub fn spec(&self) -> ToolSpec {
        self.tool.spec()
    }
}

/// Catalog of built-in tools that are not yet in the live registry.
#[derive(Clone, Default)]
pub struct DeferredNativeToolSet {
    pub stubs: Vec<DeferredNativeToolStub>,
}

impl DeferredNativeToolSet {
    pub fn empty() -> Self {
        Self { stubs: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.stubs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.stubs.len()
    }

    pub fn get_by_name(&self, name: &str) -> Option<&DeferredNativeToolStub> {
        self.stubs.iter().find(|s| s.name == name)
    }

    /// Keyword search — same ranking as MCP deferred search.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<&DeferredNativeToolStub> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        if terms.is_empty() {
            return self.stubs.iter().take(max_results).collect();
        }

        let mut scored: Vec<(&DeferredNativeToolStub, usize)> = self
            .stubs
            .iter()
            .filter_map(|stub| {
                let haystack = format!(
                    "{} {}",
                    stub.name.to_ascii_lowercase(),
                    stub.description.to_ascii_lowercase()
                );
                let hits = terms
                    .iter()
                    .filter(|t| haystack.contains(t.as_str()))
                    .count();
                if hits > 0 {
                    Some((stub, hits))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored
            .into_iter()
            .take(max_results)
            .map(|(s, _)| s)
            .collect()
    }

    pub fn tool_spec(&self, name: &str) -> Option<ToolSpec> {
        self.get_by_name(name).map(|s| s.tool.spec())
    }

    pub fn activate_arc(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.get_by_name(name).map(DeferredNativeToolStub::tool_arc)
    }
}

/// Split registry: keep tools whose names are in `keep`, defer the rest into [`DeferredNativeToolSet`].
pub fn partition_tools_for_native_deferred(
    tools: Vec<Box<dyn Tool>>,
    keep: &HashSet<String>,
) -> (Vec<Box<dyn Tool>>, DeferredNativeToolSet) {
    let mut kept = Vec::with_capacity(tools.len());
    let mut stubs = Vec::new();

    for tool in tools {
        let name = tool.name().to_string();
        if keep.contains(&name) {
            kept.push(tool);
            continue;
        }
        let arc: Arc<dyn Tool> = Arc::from(tool);
        stubs.push(DeferredNativeToolStub::new(arc));
    }

    (
        kept,
        DeferredNativeToolSet { stubs },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::tools::ToolResult;

    struct NamedTool {
        n: &'static str,
    }

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.n
        }
        fn description(&self) -> &str {
            "desc"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        async fn execute(&self, _: serde_json::Value) -> anyhow::Result<ToolResult> {
            Ok(ToolResult {
                success: true,
                output: String::new(),
                error: None,
            })
        }
    }

    #[test]
    fn partition_respects_keep() {
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(NamedTool { n: "shell" }),
            Box::new(NamedTool { n: "extra_tool" }),
        ];
        let keep = build_active_native_tool_set(&[]);
        let (kept, def) = partition_tools_for_native_deferred(tools, &keep);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name(), "shell");
        assert_eq!(def.len(), 1);
        assert_eq!(def.stubs[0].name, "extra_tool");
    }
}
