use anyhow::Result;
use std::path::Path;

use crate::agents::{AgentHookReport, HookProvider};
use crate::agents::{
    claude::hooks as claude_hooks,
    codex::hooks as codex_hooks,
    grok::hooks as grok_hooks,
    opencode::hooks as opencode_hooks,
};

pub struct GrokHooks;

impl HookProvider for GrokHooks {
    fn id(&self) -> &'static str {
        "grok"
    }

    fn present(&self, home: &Path) -> bool {
        grok_hooks::present(home)
    }

    fn hook_report(&self, home: &Path) -> AgentHookReport {
        let present = self.present(home);
        let status = grok_hooks::status(home);
        AgentHookReport {
            id: "grok",
            present,
            detail: if present {
                status.detail_label()
            } else {
                "not installed".into()
            },
            needs_setup: present && status.needs_setup(),
        }
    }

    fn setup(&self, home: &Path) -> Result<()> {
        grok_hooks::setup(home)?;
        Ok(())
    }
}

pub struct CodexHooks;

impl HookProvider for CodexHooks {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn present(&self, home: &Path) -> bool {
        codex_hooks::present(home)
    }

    fn hook_report(&self, home: &Path) -> AgentHookReport {
        let present = self.present(home);
        let status = codex_hooks::status(home);
        AgentHookReport {
            id: "codex",
            present,
            detail: codex_hooks::detail_label(&status),
            needs_setup: present
                && matches!(
                    status.health,
                    codex_hooks::CodexHookHealth::NotFound
                        | codex_hooks::CodexHookHealth::OutOfDate
                ),
        }
    }

    fn setup(&self, home: &Path) -> Result<()> {
        codex_hooks::setup(home)?;
        Ok(())
    }
}

pub struct ClaudeHooks;

impl HookProvider for ClaudeHooks {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn present(&self, home: &Path) -> bool {
        claude_hooks::present(home)
    }

    fn hook_report(&self, home: &Path) -> AgentHookReport {
        let present = self.present(home);
        let status = claude_hooks::status(home);
        AgentHookReport {
            id: "claude",
            present,
            detail: claude_hooks::detail_label(&status),
            needs_setup: present
                && matches!(
                    status.health,
                    claude_hooks::ClaudeHookHealth::NotFound
                        | claude_hooks::ClaudeHookHealth::OutOfDate
                ),
        }
    }

    fn setup(&self, home: &Path) -> Result<()> {
        claude_hooks::setup(home)?;
        Ok(())
    }
}

pub struct OpenCodeHooks;

impl HookProvider for OpenCodeHooks {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn present(&self, home: &Path) -> bool {
        opencode_hooks::present(home)
    }

    fn hook_report(&self, home: &Path) -> AgentHookReport {
        let present = self.present(home);
        let status = opencode_hooks::status(home);
        AgentHookReport {
            id: "opencode",
            present,
            detail: opencode_hooks::detail_label(&status),
            needs_setup: present
                && matches!(
                    status.health,
                    opencode_hooks::OpenCodeHookHealth::NotFound
                        | opencode_hooks::OpenCodeHookHealth::OutOfDate
                ),
        }
    }

    fn setup(&self, home: &Path) -> Result<()> {
        opencode_hooks::setup(home)?;
        Ok(())
    }
}

pub const GROK_HOOKS: GrokHooks = GrokHooks;
pub const CODEX_HOOKS: CodexHooks = CodexHooks;
pub const CLAUDE_HOOKS: ClaudeHooks = ClaudeHooks;
pub const OPENCODE_HOOKS: OpenCodeHooks = OpenCodeHooks;