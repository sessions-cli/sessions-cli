use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agents::adapter::{SessionSummary, TurnBoundary};
use crate::agents::parse_cache;
use crate::pty::naming::{is_confident_thread_title, is_weak_thread_name, shorten_prompt};

pub fn is_codex_env() -> bool {
    std::env::var_os("CODEX_THREAD_ID").is_some()
        || std::env::var_os("CODEX_HOME").is_some()
        || std::env::var_os("CODEX_CI").is_some()
}

#[derive(Debug, Default)]
pub struct CodexRolloutIndex {
    by_cwd: HashMap<String, Vec<CodexRolloutEntry>>,
}

#[derive(Debug, Clone)]
struct CodexRolloutEntry {
    modified: SystemTime,
    thread_id: String,
}

pub use super::paths::codex_home;

pub fn rollout_index(home: &Path) -> CodexRolloutIndex {
    let root = codex_home(home).join("sessions");
    let mut by_cwd: HashMap<String, Vec<CodexRolloutEntry>> = HashMap::new();
    collect_rollouts(&root, &mut by_cwd);
    for entries in by_cwd.values_mut() {
        entries.sort_by_key(|b| std::cmp::Reverse(b.modified));
    }
    CodexRolloutIndex { by_cwd }
}

pub fn assign_thread_for_cwd(
    index: &CodexRolloutIndex,
    cwd: &str,
    assigned: &mut HashSet<String>,
) -> Option<String> {
    let entries = index.by_cwd.get(normalize_cwd(cwd).as_str())?;
    entries
        .iter()
        .find(|entry| !assigned.contains(&entry.thread_id))
        .map(|entry| {
            assigned.insert(entry.thread_id.clone());
            entry.thread_id.clone()
        })
}

pub fn load_session_summary(home: &Path, _cwd: &str, session_id: &str) -> Option<SessionSummary> {
    let path = rollout_path_for_thread(&codex_home(home), session_id)?;
    let prompt = parsed_rollout(&path)?.first_user_prompt?;
    Some(SessionSummary {
        generated_title: Some(prompt),
        session_summary: None,
        agent_name: Some("codex".into()),
    })
}

pub fn session_messaged_at(home: &Path, session_id: &str) -> Option<DateTime<Utc>> {
    let path = rollout_path_for_thread(&codex_home(home), session_id)?;
    parsed_rollout(&path)?.latest_task_started_at
}

pub fn session_activity_at(home: &Path, session_id: &str) -> Option<DateTime<Utc>> {
    let path = rollout_path_for_thread(&codex_home(home), session_id)?;
    parsed_rollout(&path)?.started_at
}

pub fn thread_title_from_summary(summary: &SessionSummary) -> Option<String> {
    summary
        .generated_title
        .as_deref()
        .or(summary.session_summary.as_deref())
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(|title| {
            let thread = shorten_prompt(title);
            if thread.is_empty() || is_weak_thread_name(&thread) {
                title.to_string()
            } else {
                thread
            }
        })
        .filter(|thread| is_confident_thread_title(thread))
}

pub fn rollout_path_for_thread(codex_root: &Path, session_id: &str) -> Option<PathBuf> {
    let sessions = codex_root.join("sessions");
    find_rollout_by_id(&sessions, session_id)
}

pub fn session_cwd_for_id(home: &Path, session_id: &str) -> Option<String> {
    let path = rollout_path_for_thread(&codex_home(home), session_id)?;
    parsed_rollout(&path)?.cwd
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ParsedCodexRollout {
    cwd: Option<String>,
    started_at: Option<DateTime<Utc>>,
    latest_task_started_at: Option<DateTime<Utc>>,
    first_user_prompt: Option<String>,
    pub(super) boundary: TurnBoundary,
}

pub fn turn_boundary_from_rollout(path: &Path) -> Option<TurnBoundary> {
    parsed_rollout(path).map(|parsed| parsed.boundary)
}

pub(super) fn parsed_rollout(path: &Path) -> Option<ParsedCodexRollout> {
    parse_cache::cached_jsonl_parse(path, parse_codex_rollout_impl)
}

fn parse_codex_rollout_impl(path: &Path) -> Option<ParsedCodexRollout> {
    let data = std::fs::read_to_string(path).ok()?;
    let mut cwd = None;
    let mut started_at = None;
    let mut first_user_prompt = None;
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = value.get("type").and_then(|v| v.as_str());
        if kind == Some("session_meta") {
            if cwd.is_none() {
                cwd = value
                    .pointer("/payload/cwd")
                    .and_then(|v| v.as_str())
                    .map(normalize_cwd);
            }
            if started_at.is_none() {
                let ts = value
                    .pointer("/payload/timestamp")
                    .or_else(|| value.get("timestamp"));
                started_at = parse_rollout_ts(ts);
            }
        }
        if first_user_prompt.is_none() && kind == Some("event_msg") {
            let payload = value.get("payload")?;
            if payload.get("type").and_then(|v| v.as_str()) != Some("user_message") {
                continue;
            }
            let message = payload.get("message").and_then(|v| v.as_str())?.trim();
            if message.is_empty() || is_bootstrap_user_message(message) {
                continue;
            }
            let prompt = shorten_prompt(message);
            if is_confident_thread_title(&prompt) {
                first_user_prompt = Some(prompt);
            }
        }
    }

    let mut last_started = None;
    let mut last_completed = None;
    for line in data.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("event_msg") {
            continue;
        }
        let payload = value.get("payload")?;
        match payload.get("type").and_then(|v| v.as_str()) {
            Some("task_complete") if last_completed.is_none() => {
                last_completed = parse_rollout_ts(payload.get("completed_at"));
            }
            Some("task_started") if last_started.is_none() => {
                last_started = parse_rollout_ts(payload.get("started_at"));
            }
            _ => {}
        }
        if last_started.is_some() && last_completed.is_some() {
            break;
        }
    }
    let latest_task_started_at = last_started.or(started_at);
    Some(ParsedCodexRollout {
        cwd,
        started_at,
        latest_task_started_at,
        first_user_prompt,
        boundary: TurnBoundary {
            last_started,
            last_completed,
        },
    })
}

/// How many leading JSONL lines / bytes to scan when only cwd is needed.
/// `session_meta` (with `payload.cwd`) is written at session start, so it is
/// almost always in the first few lines — never load multi-hundred-MB rollouts.
const QUICK_CWD_MAX_LINES: usize = 64;
const QUICK_CWD_MAX_BYTES: u64 = 256 * 1024;

fn collect_rollouts(dir: &Path, by_cwd: &mut HashMap<String, Vec<CodexRolloutEntry>>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rollouts(&path, by_cwd);
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.starts_with("rollout-") || !file_name.ends_with(".jsonl") {
            continue;
        }
        let Some(thread_id) = parse_thread_id_from_rollout_name(file_name) else {
            continue;
        };
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        // Index build only needs cwd — never full-parse / cache huge rollouts.
        let Some(cwd) = quick_rollout_cwd(&path) else {
            continue;
        };
        by_cwd
            .entry(normalize_cwd(&cwd))
            .or_default()
            .push(CodexRolloutEntry {
                modified,
                thread_id,
            });
    }
}

/// Extract `cwd` from the leading `session_meta` line without reading the whole file.
/// Used by `collect_rollouts` so indexing stays cheap on large history trees.
fn quick_rollout_cwd(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader, Read};

    let file = std::fs::File::open(path).ok()?;
    // Cap the readable prefix so a pathological single-line file cannot blow memory.
    let limited = file.take(QUICK_CWD_MAX_BYTES);
    let reader = BufReader::new(limited);
    let mut lines_seen = 0usize;
    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        lines_seen += 1;
        if lines_seen > QUICK_CWD_MAX_LINES {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
            continue;
        }
        if let Some(cwd) = value
            .pointer("/payload/cwd")
            .and_then(|v| v.as_str())
            .map(normalize_cwd)
        {
            return Some(cwd);
        }
    }
    None
}

fn find_rollout_by_id(dir: &Path, session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_rollout_by_id(&path, session_id) {
                return Some(found);
            }
            continue;
        }
        let file_name = path.file_name()?.to_str()?;
        if file_name.contains(session_id) && file_name.ends_with(".jsonl") {
            return Some(path);
        }
    }
    None
}

fn parse_thread_id_from_rollout_name(file_name: &str) -> Option<String> {
    let stem = file_name.strip_suffix(".jsonl")?;
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 6 {
        return None;
    }
    let uuid = parts[parts.len() - 5..].join("-");
    (uuid.len() == 36).then_some(uuid)
}

fn is_bootstrap_user_message(message: &str) -> bool {
    message.starts_with("# AGENTS")
        || message.contains("<INSTRUCTIONS>")
        || message.starts_with("<environment_context>")
}

fn parse_rollout_ts(value: Option<&Value>) -> Option<DateTime<Utc>> {
    match value {
        Some(Value::String(ts)) => DateTime::parse_from_rfc3339(ts)
            .ok()
            .map(|ts| ts.with_timezone(&Utc)),
        Some(Value::Number(num)) => {
            let secs = num.as_i64()?;
            DateTime::from_timestamp(secs, 0)
        }
        _ => None,
    }
}

fn normalize_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::paths::test_lock::CodexHomeOverride;
    use super::*;
    use crate::agents::adapter::AgentAdapter;
    use crate::agents::codex::Codex;
    use std::fs;

    use tempfile::TempDir;

    #[test]
    fn codex_disk_respects_codex_home_env() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let custom_codex = PathBuf::from("/tmp/codex-test");
        let _ = fs::remove_dir_all(&custom_codex);
        fs::create_dir_all(&custom_codex.join("sessions/2026/06/10")).unwrap();
        let _guard = CodexHomeOverride::set(&custom_codex);

        let sid = "019eb088-9748-7ef2-86ba-4d7e20f5a576";
        let cwd = env!("CARGO_MANIFEST_DIR");
        fs::write(
            custom_codex.join(format!(
                "sessions/2026/06/10/rollout-2026-06-10T17-26-42-{sid}.jsonl"
            )),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{sid}","timestamp":"2026-06-10T07:56:42.222Z","cwd":"{cwd}"}}}}"#
            ),
        )
        .unwrap();

        assert_eq!(codex_home(home), custom_codex);
        assert!(session_activity_at(home, sid).is_some());
        assert!(rollout_path_for_thread(&codex_home(home), sid).is_some());

        let _ = fs::remove_dir_all(&custom_codex);
    }

    #[test]
    fn parse_thread_id_from_rollout_filename() {
        let id = parse_thread_id_from_rollout_name(
            "rollout-2026-06-10T17-26-42-019eb088-9748-7ef2-86ba-4d7e20f5a576.jsonl",
        );
        assert_eq!(id.as_deref(), Some("019eb088-9748-7ef2-86ba-4d7e20f5a576"));
    }

    #[test]
    fn session_activity_at_reads_rollout_timestamp() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let sid = "019eb088-9748-7ef2-86ba-4d7e20f5a576";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let rollout_dir = codex_home(home).join("sessions/2026/06/10");
        fs::create_dir_all(&rollout_dir).unwrap();
        fs::write(
            rollout_dir.join(format!("rollout-2026-06-10T17-26-42-{sid}.jsonl")),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"019eb088-9748-7ef2-86ba-4d7e20f5a576","timestamp":"2026-06-10T07:56:42.222Z","cwd":"{cwd}"}}}}"#
            ),
        )
        .unwrap();
        let at = session_activity_at(home, sid).unwrap();
        assert_eq!(
            at.timestamp_millis(),
            1_781_078_202_222 // 2026-06-10T07:56:42.222Z
        );
    }

    #[test]
    fn load_codex_summary_from_rollout_user_message() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let codex_root = codex_home(home);
        let rollout_dir = codex_root.join("sessions/2026/06/10");
        fs::create_dir_all(&rollout_dir).unwrap();
        let sid = "019eb088-9748-7ef2-86ba-4d7e20f5a576";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let path = rollout_dir.join(format!("rollout-2026-06-10T17-26-42-{sid}.jsonl"));
        fs::write(
            &path,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"019eb088-9748-7ef2-86ba-4d7e20f5a576","cwd":"{cwd}"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"fix codex sidebar titles"}}}}
"#
            ),
        )
        .unwrap();

        let summary = load_session_summary(home, env!("CARGO_MANIFEST_DIR"), sid).unwrap();
        assert_eq!(
            summary.generated_title.as_deref(),
            Some("fix codex sidebar titles")
        );
        assert_eq!(
            thread_title_from_summary(&summary).as_deref(),
            Some("fix codex sidebar titles")
        );
    }

    #[test]
    fn load_codex_summary_ignores_probe_prompts_like_testing() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let rollout_dir = codex_home(home).join("sessions/2026/06/10");
        fs::create_dir_all(&rollout_dir).unwrap();
        let sid = "019eb0bb-3711-72d2-a80c-15259d6349e4";
        let cwd = env!("CARGO_MANIFEST_DIR");
        fs::write(
            rollout_dir.join(format!("rollout-2026-06-10T18-21-59-{sid}.jsonl")),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"019eb0bb-3711-72d2-a80c-15259d6349e4","cwd":"{cwd}"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"testing"}}}}
"#
            ),
        )
        .unwrap();

        assert!(load_session_summary(home, env!("CARGO_MANIFEST_DIR"), sid).is_none());
    }

    #[test]
    fn rollout_index_assigns_threads_per_cwd() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let codex_root = codex_home(home);
        let rollout_dir = codex_root.join("sessions/2026/06/10");
        fs::create_dir_all(&rollout_dir).unwrap();
        let cwd = env!("CARGO_MANIFEST_DIR");
        for sid in [
            "019eb000-0000-7000-8000-000000000001",
            "019eb088-9748-7ef2-86ba-4d7e20f5a576",
        ] {
            let path = rollout_dir.join(format!("rollout-2026-06-10T17-26-42-{sid}.jsonl"));
            fs::write(
                &path,
                format!(r#"{{"type":"session_meta","payload":{{"id":"{sid}","cwd":"{cwd}"}}}}"#),
            )
            .unwrap();
        }

        let index = rollout_index(home);
        let mut assigned = HashSet::new();
        let first = assign_thread_for_cwd(&index, cwd, &mut assigned).unwrap();
        let second = assign_thread_for_cwd(&index, cwd, &mut assigned).unwrap();
        assert_ne!(first, second);
        assert_eq!(assigned.len(), 2);
    }

    #[test]
    fn quick_rollout_cwd_reads_session_meta_without_full_parse() {
        parse_cache::clear_parse_cache();
        let dir = TempDir::new().unwrap();
        let path = dir
            .path()
            .join("rollout-2026-06-10T17-26-42-019eb088-9748-7ef2-86ba-4d7e20f5a576.jsonl");
        let cwd = "/home/testuser/projects/acme";
        // Prefix noise + session_meta + a large trailing payload that must not be loaded fully.
        let mut body = String::new();
        body.push_str(r#"{"type":"response_item","payload":{"type":"noise"}}"#);
        body.push('\n');
        body.push_str(&format!(
            r#"{{"type":"session_meta","payload":{{"id":"019eb088-9748-7ef2-86ba-4d7e20f5a576","cwd":"{cwd}"}}}}"#
        ));
        body.push('\n');
        body.push_str(&format!(
            r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"{}"}}}}"#,
            "x".repeat(8_192)
        ));
        body.push('\n');
        fs::write(&path, body).unwrap();

        assert_eq!(quick_rollout_cwd(&path).as_deref(), Some(cwd));

        // Index path must not populate the heavy parse cache.
        let home = dir.path();
        let rollout_dir = codex_home(home).join("sessions/2026/06/10");
        fs::create_dir_all(&rollout_dir).unwrap();
        let indexed = rollout_dir
            .join("rollout-2026-06-10T17-26-42-019eb088-9748-7ef2-86ba-4d7e20f5a576.jsonl");
        fs::copy(&path, &indexed).unwrap();
        parse_cache::clear_parse_cache();
        let index = rollout_index(home);
        assert!(index.by_cwd.contains_key(cwd));
        // Full parse still works for a specific session.
        assert_eq!(
            parsed_rollout(&indexed).and_then(|p| p.cwd).as_deref(),
            Some(cwd)
        );
    }

    #[test]
    fn quick_rollout_cwd_returns_none_without_session_meta() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rollout-empty.jsonl");
        fs::write(
            &path,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}
"#,
        )
        .unwrap();
        assert!(quick_rollout_cwd(&path).is_none());
    }

    #[test]
    fn parsed_rollout_reuses_cache_for_multiple_fact_lookups() {
        parse_cache::clear_parse_cache();
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let rollout_dir = codex_home(home).join("sessions/2026/06/10");
        fs::create_dir_all(&rollout_dir).unwrap();
        let sid = "019eb088-9748-7ef2-86ba-4d7e20f5a576";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let path = rollout_dir.join(format!("rollout-2026-06-10T17-26-42-{sid}.jsonl"));
        fs::write(
            &path,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"019eb088-9748-7ef2-86ba-4d7e20f5a576","timestamp":"2026-06-10T07:56:42.222Z","cwd":"{cwd}"}}}}
{{"type":"event_msg","payload":{{"type":"task_started","started_at":"2026-06-10T08:00:00Z"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"cache parse once"}}}}
"#
            ),
        )
        .unwrap();

        let rollout = parsed_rollout(&path);
        assert!(rollout.is_some());
        assert_eq!(parsed_rollout(&path), rollout);

        assert!(load_session_summary(home, env!("CARGO_MANIFEST_DIR"), sid).is_some());
        assert!(session_messaged_at(home, sid).is_some());
        assert!(session_activity_at(home, sid).is_some());
        let boundary = Codex.turn_boundary(home, env!("CARGO_MANIFEST_DIR"), sid);
        assert!(boundary.is_some());
        assert_eq!(parsed_rollout(&path), rollout);
    }
}
