use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::disk::{
    events_path_for_session_id, load_session_summary, parse_event_ts, phase_is_active,
    phase_to_state, read_tail_bytes, session_activity_at, session_cwd_for_id, session_dir,
    session_messaged_at, summary_path_for_session_id, thread_title_from_summary,
};
use crate::agents::adapter::{AgentAdapter, LiveActivity, SessionSummary, TurnBoundary};
use crate::model::AgentState;

pub struct Grok;

impl AgentAdapter for Grok {
    fn id(&self) -> &'static str {
        "grok"
    }

    fn display_name(&self) -> &'static str {
        "grok"
    }

    fn binary_matches(&self, binary: &str) -> bool {
        let base = binary
            .rsplit('/')
            .next()
            .unwrap_or(binary)
            .to_ascii_lowercase();
        base == "grok" || base.starts_with("grok-")
    }

    fn extract_thread(&self, command: &str) -> Option<String> {
        crate::pty::extract_natural_language_arg(command)
    }

    fn session_id_env_var(&self) -> Option<&'static str> {
        Some("GROK_SESSION_ID")
    }

    fn resolve_session_id_from_env(&self) -> Option<String> {
        std::env::var("GROK_SESSION_ID").ok()
    }

    fn state_dir(&self, home: &Path) -> PathBuf {
        crate::paths::state_dir(home)
    }

    fn summary_path(&self, home: &Path, cwd: &str, sid: &str) -> Option<PathBuf> {
        Some(session_dir(home, cwd, sid).join("summary.json"))
    }

    fn events_path(&self, home: &Path, cwd: &str, sid: &str) -> Option<PathBuf> {
        Some(session_dir(home, cwd, sid).join("events.jsonl"))
    }

    fn live_activity(&self, home: &Path, cwd: &str, sid: &str) -> Option<LiveActivity> {
        let path = self.events_path(home, cwd, sid)?;
        let tail = read_tail_bytes(&path, 64 * 1024)?;
        let mut last_started = None;
        let mut last_completed = None;
        let mut last_phase: Option<(String, DateTime<Utc>)> = None;
        for line in tail.lines().rev() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(kind) = value.get("type").and_then(|v| v.as_str()) else {
                continue;
            };
            match kind {
                "turn_started" if last_started.is_none() => {
                    last_started = parse_event_ts(&value);
                }
                "turn_ended" if last_completed.is_none() => {
                    let outcome = value.get("outcome").and_then(|v| v.as_str()).unwrap_or("");
                    if outcome == "completed" {
                        last_completed = parse_event_ts(&value);
                    }
                }
                "phase_changed" if last_phase.is_none() => {
                    let Some(phase) = value
                        .get("phase")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                    else {
                        continue;
                    };
                    if !phase_is_active(&phase) {
                        continue;
                    }
                    let Some(at) = parse_event_ts(&value) else {
                        continue;
                    };
                    last_phase = Some((phase, at));
                }
                _ => {}
            }
            if last_started.is_some() && last_phase.is_some() {
                break;
            }
        }
        let started = last_started?;
        let boundary = TurnBoundary {
            last_started,
            last_completed,
        };
        if crate::agents::adapter::turn_is_complete(&boundary) {
            return None;
        }
        let (state, at) = match last_phase {
            Some((phase, ts)) => (phase_to_state(&phase), ts),
            None => (AgentState::Working, started),
        };
        Some(LiveActivity { state, at })
    }

    fn turn_boundary(&self, home: &Path, cwd: &str, sid: &str) -> Option<TurnBoundary> {
        let path = self.events_path(home, cwd, sid)?;
        let tail = read_tail_bytes(&path, 64 * 1024)?;
        let mut last_started = None;
        let mut last_completed = None;
        for line in tail.lines().rev() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(kind) = value.get("type").and_then(|v| v.as_str()) else {
                continue;
            };
            if kind == "turn_started" && last_started.is_none() {
                last_started = parse_event_ts(&value);
            } else if kind == "turn_ended" && last_completed.is_none() {
                let outcome = value.get("outcome").and_then(|v| v.as_str()).unwrap_or("");
                if outcome == "completed" {
                    last_completed = parse_event_ts(&value);
                }
            }
            if last_started.is_some() && last_completed.is_some() {
                break;
            }
        }
        Some(TurnBoundary {
            last_started,
            last_completed,
        })
    }

    fn hook_config_paths(&self, home: &Path) -> Vec<PathBuf> {
        let hooks = home.join(".grok/hooks");
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
        summary_path_for_session_id(home, sid).is_some()
            || events_path_for_session_id(home, sid).is_some()
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
}