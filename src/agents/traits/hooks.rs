use anyhow::Result;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHookReport {
    pub id: &'static str,
    pub present: bool,
    pub detail: String,
    pub needs_setup: bool,
}

pub trait HookProvider: Sync {
    fn id(&self) -> &'static str;
    fn present(&self, home: &Path) -> bool;
    fn hook_report(&self, home: &Path) -> AgentHookReport;
    fn setup(&self, home: &Path) -> Result<()>;
}