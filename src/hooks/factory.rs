//! HookRunner 工厂模块
//!
//! 集中管理所有 builtin hooks 的注册逻辑，减少核心模块的模板代码。

use crate::config::Config;
use crate::hooks::{builtin, HookRunner};

/// 构建 HookRunner 实例。
///
/// 根据配置决定注册哪些 builtin hooks。
/// 如果 hooks 系统未启用，返回 `None`。
pub fn build_hooks_runner(config: &Config) -> Option<HookRunner> {
    if !config.hooks.enabled {
        return None;
    }

    let mut runner = HookRunner::new();

    // Command Logger Hook
    if config.hooks.builtin.command_logger {
        runner.register(Box::new(builtin::CommandLoggerHook::new()));
    }

    // Webhook Audit Hook
    if config.hooks.builtin.webhook_audit.enabled {
        runner.register(Box::new(builtin::WebhookAuditHook::new(
            config.hooks.builtin.webhook_audit.clone(),
        )));
    }

    // Guardrail Hook
    if config.hooks.builtin.guardrail.enabled {
        runner.register(Box::new(builtin::GuardrailHook::new(
            config.hooks.builtin.guardrail.clone(),
        )));
    }

    Some(runner)
}
