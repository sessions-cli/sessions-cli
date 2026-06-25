use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::disk::{
    assign_thread_for_cwd, codex_home, load_session_summary, rollout_index,
    turn_boundary_from_rollout,
    rollout_path_for_thread, session_activity_at, session_cwd_for_id, session_messaged_at,
    thread_title_from_summary,
};
use crate::agents::adapter::{AgentAdapter, LiveActivity, SessionSummary, TurnBoundary};
use crate::model::AgentState;

pub struct Codex;

impl AgentAdapter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        "codex"
    }

    fn binary_matches(&self, binary: &str) -> bool {
        let base = binary
            .rsplit('/')
            .next()
            .unwrap_or(binary)
            .to_ascii_lowercase();
        base == "codex" || base.starts_with("codex-")
    }

    fn extract_thread(&self, command: &str) -> Option<String> {
        crate::pty::extract_natural_language_arg(command)
    }

    fn session_id_env_var(&self) -> Option<&'static str> {
        Some("CODEX_THREAD_ID")
    }

    fn resolve_session_id_from_env(&self) -> Option<String> {
        std::env::var("CODEX_THREAD_ID").ok()
    }

    fn state_dir(&self, home: &Path) -> PathBuf {
        crate::paths::state_dir(home)
    }

    fn summary_path(&self, home: &Path, _cwd: &str, sid: &str) -> Option<PathBuf> {
        rollout_path_for_thread(&codex_home(home), sid)
    }

    fn events_path(&self, home: &Path, cwd: &str, sid: &str) -> Option<PathBuf> {
        self.summary_path(home, cwd, sid)
    }

    fn live_activity(&self, home: &Path, cwd: &str, sid: &str) -> Option<LiveActivity> {
        let boundary = self.turn_boundary(home, cwd, sid)?;
        if crate::agents::adapter::turn_is_complete(&boundary) {
            return None;
        }
        let started = boundary.last_started?;
        Some(LiveActivity {
            state: AgentState::Working,
            at: started,
        })
    }

    fn turn_boundary(&self, home: &Path, cwd: &str, sid: &str) -> Option<TurnBoundary> {
        let path = self.events_path(home, cwd, sid)?;
        turn_boundary_from_rollout(&path)
    }

    fn hook_config_paths(&self, home: &Path) -> Vec<PathBuf> {
        let hooks = codex_home(home).join("hooks");
        std::fs::read_dir(&hooks)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn detect_session_on_disk(&self, home: &Path, sid: &str) -> bool {
        rollout_path_for_thread(&codex_home(home), sid).is_some()
    }

    fn load_summary(&self, home: &Path, cwd: &str, sid: &str) -> Option<SessionSummary> {
        load_session_summary(home, cwd, sid)
    }

    fn session_cwd(&self, home: &Path, sid: &str) -> Option<String> {
        session_cwd_for_id(home, sid)
    }

    fn messaged_at(&self, home: &Path, sid: &str) -> Option<DateTime<Utc>> {
        session_messaged_at(home, sid)
    }

    fn activity_at(&self, home: &Path, sid: &str) -> Option<DateTime<Utc>> {
        session_activity_at(home, sid)
    }

    fn thread_title_from_summary(&self, summary: &SessionSummary) -> Option<String> {
        thread_title_from_summary(summary)
    }

    fn assign_session_for_cwd(
        &self,
        home: &Path,
        cwd: &str,
        assigned: &mut HashSet<String>,
    ) -> Option<String> {
        let index = rollout_index(home);
        assign_thread_for_cwd(&index, cwd, assigned)
    }
}