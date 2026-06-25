use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::agents::adapter::SessionSummary;
use crate::pty::naming::{is_sticky_thread_title, is_weak_thread_name, parse_description};
use crate::session::encode_session_cwd;

pub fn session_dir(home: &Path, cwd: &str, session_id: &str) -> PathBuf {
    crate::paths::provider_sessions_dir(home, "grok")
        .join(encode_session_cwd(cwd))
        .join(session_id)
}

/// Legacy alias retained for tests and gradual migration.
pub fn grok_session_dir(home: &Path, cwd: &str, session_id: &str) -> PathBuf {
    session_dir(home, cwd, session_id)
}

pub fn grok_session_summary_path(home: &Path, cwd: &str, session_id: &str) -> PathBuf {
    session_dir(home, cwd, session_id).join("summary.json")
}

pub fn grok_events_path(home: &Path, cwd: &str, session_id: &str) -> PathBuf {
    session_dir(home, cwd, session_id).join("events.jsonl")
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GrokSessionSummary {
    #[serde(default)]
    pub generated_title: Option<String>,
    #[serde(default)]
    pub session_summary: Option<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub last_active_at: Option<String>,
}

/// Resolve the parent Grok thread when `session_id` is a Task/subagent child.
pub fn parent_session_id_for_subagent(home: &Path, session_id: &str) -> Option<String> {
    let root = crate::paths::provider_sessions_dir(home, "grok");
    let encoded_dirs = std::fs::read_dir(root).ok()?;
    for encoded in encoded_dirs.flatten() {
        if !encoded.path().is_dir() {
            continue;
        }
        let parent_dirs = std::fs::read_dir(encoded.path()).ok()?;
        for parent in parent_dirs.flatten() {
            if !parent.path().is_dir() {
                continue;
            }
            let meta_path = parent
                .path()
                .join("subagents")
                .join(session_id)
                .join("meta.json");
            if !meta_path.is_file() {
                continue;
            }
            let data = std::fs::read_to_string(&meta_path).ok()?;
            let value: Value = serde_json::from_str(&data).ok()?;
            return value
                .get("parent_session_id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
    }
    None
}

pub fn is_subagent_of(home: &Path, child_id: &str, parent_id: &str) -> bool {
    parent_session_id_for_subagent(home, child_id).as_deref() == Some(parent_id)
}

/// Locate a Grok summary on disk by session id, regardless of pane cwd.
pub fn events_path_for_session_id(home: &Path, session_id: &str) -> Option<PathBuf> {
    let root = crate::paths::provider_sessions_dir(home, "grok");
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let encoded = entry.path();
        if !encoded.is_dir() {
            continue;
        }
        let candidate = encoded.join(session_id).join("events.jsonl");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn turn_started_from_events_text(
    lines: impl IntoIterator<Item = impl AsRef<str>>,
) -> Option<DateTime<Utc>> {
    for line in lines {
        let line = line.as_ref().trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("turn_started") {
            continue;
        }
        if let Some(at) = parse_event_ts(&value) {
            return Some(at);
        }
    }
    None
}

/// Latest user turn start — sidebar ordering, not tool-hook noise.
pub fn session_messaged_at(home: &Path, session_id: &str) -> Option<DateTime<Utc>> {
    let path = events_path_for_session_id(home, session_id)?;
    if let Some(tail) = read_tail_bytes(&path, 64 * 1024) {
        if let Some(at) = turn_started_from_events_text(tail.lines().rev()) {
            return Some(at);
        }
    }
    // Long threads keep the first `turn_started` at the file head — tail scans miss it.
    read_head_bytes(&path, 64 * 1024).and_then(|head| turn_started_from_events_text(head.lines()))
}

pub fn session_activity_at(home: &Path, session_id: &str) -> Option<DateTime<Utc>> {
    if let Some(path) = summary_path_for_session_id(home, session_id) {
        let data = std::fs::read_to_string(path).ok()?;
        let value: Value = serde_json::from_str(&data).ok()?;
        if let Some(ts) = value.get("last_active_at").and_then(|v| v.as_str()) {
            if let Ok(at) = DateTime::parse_from_rfc3339(ts) {
                return Some(at.with_timezone(&Utc));
            }
        }
    }
    let path = events_path_for_session_id(home, session_id)?;
    let tail = read_tail_bytes(&path, 16 * 1024)?;
    for line in tail.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(at) = parse_event_ts(&value) {
            return Some(at);
        }
    }
    None
}

pub fn summary_path_for_session_id(home: &Path, session_id: &str) -> Option<PathBuf> {
    let root = crate::paths::provider_sessions_dir(home, "grok");
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let encoded = entry.path();
        if !encoded.is_dir() {
            continue;
        }
        let candidate = encoded.join(session_id).join("summary.json");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn session_cwd_for_id(home: &Path, session_id: &str) -> Option<String> {
    let path = summary_path_for_session_id(home, session_id)?;
    let data = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&data).ok()?;
    value
        .pointer("/info/cwd")
        .and_then(|v| v.as_str())
        .map(normalize_summary_cwd)
}

fn normalize_summary_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.to_string()
    }
}

pub fn load_session_summary(home: &Path, cwd: &str, session_id: &str) -> Option<SessionSummary> {
    let direct = session_dir(home, cwd, session_id).join("summary.json");
    let path = if direct.is_file() {
        direct
    } else {
        summary_path_for_session_id(home, session_id)?
    };
    let data = std::fs::read_to_string(path).ok()?;
    let raw: GrokSessionSummary = serde_json::from_str(&data).ok()?;
    Some(SessionSummary {
        generated_title: raw.generated_title,
        session_summary: raw.session_summary,
        agent_name: raw.agent_name,
    })
}

pub fn thread_title_from_summary(summary: &SessionSummary) -> Option<String> {
    summary
        .generated_title
        .as_deref()
        .or(summary.session_summary.as_deref())
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(|title| {
            let thread = parse_description(title);
            if is_weak_thread_name(&thread) {
                title.to_string()
            } else {
                thread
            }
        })
        .filter(|thread| is_sticky_thread_title(thread))
}

pub(super) fn read_tail_bytes(path: &Path, max_bytes: usize) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    Some(buf)
}

fn read_head_bytes(path: &Path, max_bytes: usize) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; max_bytes];
    let read = file.read(&mut buf).ok()?;
    String::from_utf8(buf[..read].to_vec()).ok()
}

pub(super) fn parse_event_ts(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("ts")
        .and_then(|v| v.as_str())
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|ts| ts.with_timezone(&Utc))
}

pub(super) fn phase_is_active(phase: &str) -> bool {
    matches!(
        phase,
        "streaming_reasoning"
            | "streaming_text"
            | "tool_execution"
            | "waiting_for_model"
            | "permission_prompt"
    )
}

pub(super) fn phase_to_state(phase: &str) -> crate::model::AgentState {
    if phase == "permission_prompt" {
        crate::model::AgentState::Approval
    } else {
        crate::model::AgentState::Working
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrokTurnBoundary {
    pub last_started: Option<DateTime<Utc>>,
    pub last_completed: Option<DateTime<Utc>>,
}

/// Latest turn start/end markers from Grok's on-disk session log.
pub fn grok_turn_boundary(home: &Path, cwd: &str, session_id: &str) -> Option<GrokTurnBoundary> {
    let path = grok_events_path(home, cwd, session_id);
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
    Some(GrokTurnBoundary {
        last_started,
        last_completed,
    })
}

/// True when Grok recorded a completed turn after the latest `turn_started`.
pub fn grok_turn_is_complete(boundary: &GrokTurnBoundary) -> bool {
    match (boundary.last_started, boundary.last_completed) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(started), Some(ended)) => ended > started,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrokLiveActivity {
    pub state: crate::model::AgentState,
    pub at: DateTime<Utc>,
}

/// Latest in-flight turn activity from Grok's on-disk `events.jsonl`.
pub fn grok_live_activity(home: &Path, cwd: &str, session_id: &str) -> Option<GrokLiveActivity> {
    let path = grok_events_path(home, cwd, session_id);
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
    let boundary = GrokTurnBoundary {
        last_started,
        last_completed,
    };
    if grok_turn_is_complete(&boundary) {
        return None;
    }
    let (state, at) = match last_phase {
        Some((phase, ts)) => (phase_to_state(&phase), ts),
        None => (crate::model::AgentState::Working, started),
    };
    Some(GrokLiveActivity { state, at })
}

#[cfg(test)]
mod disk_tests {
    use super::*;
    use crate::model::AgentState;
    use chrono::Utc;

    #[test]
    fn grok_turn_is_complete_when_latest_event_is_turn_ended() {
        let started = Utc::now() - chrono::Duration::minutes(5);
        let completed = Utc::now() - chrono::Duration::minutes(1);
        assert!(grok_turn_is_complete(&GrokTurnBoundary {
            last_started: Some(started),
            last_completed: Some(completed),
        }));
        assert!(!grok_turn_is_complete(&GrokTurnBoundary {
            last_started: Some(completed),
            last_completed: Some(started),
        }));
    }

    #[test]
    fn grok_live_activity_reads_in_flight_phase() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");
        let sid = "019ea057-3abe-74e2-b130-2f01c3dd1988";
        let events_dir = grok_session_dir(home, cwd, sid);
        std::fs::create_dir_all(&events_dir).unwrap();
        std::fs::write(
            grok_events_path(home, cwd, sid),
            r#"{"ts":"2026-06-07T04:38:45.548Z","type":"turn_started"}
{"ts":"2026-06-07T04:40:09.787Z","type":"phase_changed","phase":"streaming_reasoning"}
"#,
        )
        .unwrap();
        let activity = grok_live_activity(home, cwd, sid).unwrap();
        assert_eq!(activity.state, AgentState::Working);
    }

    #[test]
    fn grok_live_activity_maps_permission_prompt_to_approval() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");
        let sid = "019ea057-3abe-74e2-b130-2f01c3dd1988";
        let events_dir = grok_session_dir(home, cwd, sid);
        std::fs::create_dir_all(&events_dir).unwrap();
        std::fs::write(
            grok_events_path(home, cwd, sid),
            r#"{"ts":"2026-06-07T04:38:45.548Z","type":"turn_started"}
{"ts":"2026-06-07T04:40:14.087Z","type":"phase_changed","phase":"permission_prompt"}
"#,
        )
        .unwrap();
        let activity = grok_live_activity(home, cwd, sid).unwrap();
        assert_eq!(activity.state, AgentState::Approval);
    }

    #[test]
    fn grok_live_activity_ignores_completed_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");
        let sid = "019ea057-3abe-74e2-b130-2f01c3dd1988";
        let events_dir = grok_session_dir(home, cwd, sid);
        std::fs::create_dir_all(&events_dir).unwrap();
        std::fs::write(
            grok_events_path(home, cwd, sid),
            r#"{"ts":"2026-06-07T04:38:45.548Z","type":"turn_started"}
{"ts":"2026-06-07T04:39:12.271Z","type":"turn_ended","outcome":"completed"}
"#,
        )
        .unwrap();
        assert!(grok_live_activity(home, cwd, sid).is_none());
    }

    #[test]
    fn parent_session_id_for_subagent_reads_meta_json() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let parent = "019efbed-parent-session-id";
        let subagent = "019efbee-subagent-session";
        let encoded = crate::session::env::encode_session_cwd("/tmp/project");
        let subagent_dir = crate::paths::provider_sessions_dir(home, "grok")
            .join(&encoded)
            .join(parent)
            .join("subagents")
            .join(subagent);
        std::fs::create_dir_all(&subagent_dir).unwrap();
        std::fs::write(
            subagent_dir.join("meta.json"),
            format!(
                r#"{{"parent_session_id":"{parent}","child_session_id":"{subagent}"}}"#
            ),
        )
        .unwrap();

        assert_eq!(
            parent_session_id_for_subagent(home, subagent).as_deref(),
            Some(parent)
        );
        assert!(is_subagent_of(home, subagent, parent));
    }

    #[test]
    fn grok_turn_boundary_reads_tail_of_events_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");
        let sid = "019ea009-b0b4-7e41-b767-43993c604b7f";
        let events_dir = grok_session_dir(home, cwd, sid);
        std::fs::create_dir_all(&events_dir).unwrap();
        std::fs::write(
            grok_events_path(home, cwd, sid),
            r#"{"ts":"2026-06-07T03:22:20.361Z","type":"turn_started"}
{"ts":"2026-06-07T03:22:46.584Z","type":"turn_ended","outcome":"completed"}
"#,
        )
        .unwrap();
        let boundary = grok_turn_boundary(home, cwd, sid).unwrap();
        assert!(grok_turn_is_complete(&boundary));
    }
}