use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::disk::{
    assign_session_for_cwd, is_opencode_session_id, load_session_summary, millis_to_utc,
    open_readonly, opencode_data_dir, opencode_db_path, opencode_session_index,
    session_activity_at, session_cwd_for_id, session_exists, session_messaged_at,
    thread_title_from_summary,
};
use crate::agents::adapter::{AgentAdapter, LiveActivity, SessionSummary, TurnBoundary};
use crate::model::AgentState;

pub struct OpenCode;

impl AgentAdapter for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn display_name(&self) -> &'static str {
        "opencode"
    }

    fn binary_matches(&self, binary: &str) -> bool {
        let base = binary
            .rsplit('/')
            .next()
            .unwrap_or(binary)
            .to_ascii_lowercase();
        base == "opencode" || base.starts_with("opencode-")
    }

    fn extract_thread(&self, command: &str) -> Option<String> {
        crate::pty::extract_natural_language_arg(command)
    }

    fn session_id_env_var(&self) -> Option<&'static str> {
        Some("OPENCODE_SESSION_ID")
    }

    fn resolve_session_id_from_env(&self) -> Option<String> {
        std::env::var("OPENCODE_SESSION_ID").ok()
    }

    fn state_dir(&self, _home: &Path) -> PathBuf {
        opencode_data_dir(_home)
    }

    fn summary_path(&self, _home: &Path, _cwd: &str, _sid: &str) -> Option<PathBuf> {
        None
    }

    fn events_path(&self, _home: &Path, _cwd: &str, _sid: &str) -> Option<PathBuf> {
        None
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

    fn turn_boundary(&self, home: &Path, _cwd: &str, sid: &str) -> Option<TurnBoundary> {
        let conn = open_readonly(&opencode_db_path(home)).ok()?;
        let last_started = {
            let mut stmt = conn
                .prepare(
                    "SELECT MAX(m.time_created) FROM message m
                     WHERE m.session_id = ?1
                       AND json_extract(m.data, '$.role') = 'user'",
                )
                .ok()?;
            let ms: Option<i64> = stmt.query_row([sid], |row| row.get(0)).ok()?;
            ms.and_then(millis_to_utc)
        };
        let last_completed = {
            let mut stmt = conn
                .prepare(
                    "SELECT MAX(m.time_updated) FROM message m
                     WHERE m.session_id = ?1
                       AND json_extract(m.data, '$.role') = 'assistant'
                       AND json_extract(m.data, '$.finish') IN ('stop', 'end_turn')",
                )
                .ok()?;
            let ms: Option<i64> = stmt.query_row([sid], |row| row.get(0)).ok()?;
            ms.and_then(millis_to_utc)
        };
        Some(TurnBoundary {
            last_started,
            last_completed,
        })
    }

    fn hook_config_paths(&self, home: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for dir in [
            home.join(".config/opencode/plugins"),
            home.join(".opencode/plugins"),
        ] {
            let candidate = dir.join("sessions.ts");
            if candidate.is_file() {
                paths.push(candidate);
            }
        }
        paths
    }

    fn detect_session_on_disk(&self, home: &Path, sid: &str) -> bool {
        if !is_opencode_session_id(sid) {
            return false;
        }
        // Validate session exists in the opencode SQLite DB.
        // The ses_ prefix alone is insufficient — other systems could
        // generate IDs with that prefix. A DB hit confirms this is a
        // genuine opencode session.
        session_exists(home, sid)
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
        let index = opencode_session_index(home);
        assign_session_for_cwd(&index, cwd, assigned)
    }
}
