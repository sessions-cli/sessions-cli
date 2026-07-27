#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolbarAction {
    NewSession,
    Search,
    Automations,
    Mcps,
    Skills,
    Settings,
    Leave,
}

pub fn toolbar_action_coming_soon(action: ToolbarAction) -> bool {
    matches!(action, ToolbarAction::Search)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateBannerAction {
    Upgrade,
    Dismiss,
}

#[derive(Debug, Clone)]
pub struct UpdateBannerView {
    pub version: String,
    pub label: String,
    pub critical: bool,
}
