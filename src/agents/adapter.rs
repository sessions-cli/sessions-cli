use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::AgentState;

/// Latest turn start/end markers from an agent's on-disk session log.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TurnBoundary {
    pub last_started: Option<DateTime<Utc>>,
    pub last_completed: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveActivity {
    pub state: AgentState,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionSummary {
    pub generated_title: Option<String>,
    pub session_summary: Option<String>,
    pub agent_name: Option<String>,
}

pub trait AgentAdapter: Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn binary_matches(&self, binary: &str) -> bool;
    fn extract_thread(&self, command: &str) -> Option<String>;
    fn session_id_env_var(&self) -> Option<&'static str>;
    fn resolve_session_id_from_env(&self) -> Option<String>;
    fn state_dir(&self, home: &Path) -> PathBuf;
    fn summary_path(&self, home: &Path, cwd: &str, sid: &str) -> Option<PathBuf>;
    fn events_path(&self, home: &Path, cwd: &str, sid: &str) -> Option<PathBuf>;
    fn live_activity(&self, home: &Path, cwd: &str, sid: &str) -> Option<LiveActivity>;
    fn turn_boundary(&self, home: &Path, cwd: &str, sid: &str) -> Option<TurnBoundary>;
    fn hook_config_paths(&self, home: &Path) -> Vec<PathBuf>;

    /// True when this agent has on-disk thread data for `sid` (detection probe).
    fn detect_session_on_disk(&self, home: &Path, sid: &str) -> bool;

    fn load_summary(&self, home: &Path, cwd: &str, sid: &str) -> Option<SessionSummary>;

    fn session_cwd(&self, home: &Path, sid: &str) -> Option<String>;

    fn messaged_at(&self, home: &Path, sid: &str) -> Option<DateTime<Utc>>;

    fn activity_at(&self, home: &Path, sid: &str) -> Option<DateTime<Utc>>;

    fn thread_title_from_summary(&self, summary: &SessionSummary) -> Option<String>;

    /// Assign an unbound session id for a pane cwd during poll merge. Default: none.
    fn assign_session_for_cwd(
        &self,
        _home: &Path,
        _cwd: &str,
        _assigned: &mut HashSet<String>,
    ) -> Option<String> {
        None
    }
}

pub fn turn_is_complete(boundary: &TurnBoundary) -> bool {
    match (boundary.last_started, boundary.last_completed) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(started), Some(ended)) => ended > started,
    }
}
