use super::adapter::AgentAdapter;
use super::claude::CLAUDE_LAUNCH;
use super::codex::CODEX_LAUNCH;
use super::grok::GROK_LAUNCH;
use super::opencode::OPENCODE_LAUNCH;
use super::traits::LaunchProvider;
use super::traits::hooks::HookProvider;
use super::{Claude, Codex, Grok, OpenCode};
use crate::hooks::providers::{
    CLAUDE_HOOKS, CODEX_HOOKS, GROK_HOOKS, OPENCODE_HOOKS,
};

pub struct ProviderDescriptor {
    pub id: &'static str,
    pub adapter: &'static dyn AgentAdapter,
    pub hooks: &'static dyn HookProvider,
    pub launch: &'static dyn LaunchProvider,
    /// Lower value = higher priority when probing on-disk session identity.
    pub detection_priority: u8,
}

pub const PROVIDERS: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        id: "opencode",
        adapter: &OpenCode,
        hooks: &OPENCODE_HOOKS,
        launch: &OPENCODE_LAUNCH,
        detection_priority: 0,
    },
    ProviderDescriptor {
        id: "codex",
        adapter: &Codex,
        hooks: &CODEX_HOOKS,
        launch: &CODEX_LAUNCH,
        detection_priority: 1,
    },
    ProviderDescriptor {
        id: "claude",
        adapter: &Claude,
        hooks: &CLAUDE_HOOKS,
        launch: &CLAUDE_LAUNCH,
        detection_priority: 2,
    },
    ProviderDescriptor {
        id: "grok",
        adapter: &Grok,
        hooks: &GROK_HOOKS,
        launch: &GROK_LAUNCH,
        detection_priority: 3,
    },
];

pub const AGENT_IDS: &[&str] = &["grok", "codex", "claude", "opencode"];

pub fn provider_by_id(id: &str) -> Option<&'static ProviderDescriptor> {
    let id = id.trim().to_ascii_lowercase();
    PROVIDERS.iter().find(|provider| provider.id == id)
}

pub fn hook_provider_by_id(id: &str) -> Option<&'static dyn HookProvider> {
    provider_by_id(id).map(|provider| provider.hooks)
}

pub fn launch_provider_by_id(id: &str) -> Option<&'static dyn LaunchProvider> {
    provider_by_id(id).map(|provider| provider.launch)
}

pub fn providers_by_detection_priority() -> impl Iterator<Item = &'static ProviderDescriptor> {
    let mut ordered: Vec<&ProviderDescriptor> = PROVIDERS.iter().collect();
    ordered.sort_by_key(|provider| provider.detection_priority);
    ordered.into_iter()
}