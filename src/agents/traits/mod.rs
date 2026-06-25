pub mod hooks;
pub mod launch;

pub use hooks::{AgentHookReport, HookProvider};
pub use launch::{LaunchProvider, ModelOption, shell_quote};
pub use crate::agents::claude::launch::{ClaudeLaunch, CLAUDE_LAUNCH};
pub use crate::agents::codex::launch::{CodexLaunch, CODEX_LAUNCH};
pub use crate::agents::grok::launch::{GrokLaunch, GROK_LAUNCH};
pub use crate::agents::opencode::launch::{OpenCodeLaunch, OPENCODE_LAUNCH};