use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::agents::adapter::{SessionSummary, TurnBoundary};
use crate::pty::naming::{is_confident_thread_title, is_weak_thread_name, shorten_prompt};

pub fn is_claude_env() -> bool {
    std::env::var_os("CLAUDE_SESSION_ID").is_some()
}

#[derive(Debug, Default)]
pub struct ClaudeSessionIndex {
    by_cwd: HashMap<String, Vec<ClaudeSessionEntry>>,
}

#[derive(Debug, Clone)]
struct ClaudeSessionEntry {
    modified: SystemTime,
    session_id: String,
}

pub fn claude_home(home: &Path) -> PathBuf {
    home.join(".claude")
}

pub fn claude_session_index(home: &Path) -> ClaudeSessionIndex {
    let root = claude_home(home).join("projects");
    let mut by_cwd: HashMap<String, Vec<ClaudeSessionEntry>> = HashMap::new();
    collect_sessions(&root, &mut by_cwd);
    for entries in by_cwd.values_mut() {
        entries.sort_by_key(|b| std::cmp::Reverse(b.modified));
    }
    ClaudeSessionIndex { by_cwd }
}

pub fn assign_session_for_cwd(
    index: &ClaudeSessionIndex,
    cwd: &str,
    assigned: &mut HashSet<String>,
) -> Option<String> {
    let entries = index.by_cwd.get(normalize_cwd(cwd).as_str())?;
    entries
        .iter()
        .find(|entry| !assigned.contains(&entry.session_id))
        .map(|entry| {
            assigned.insert(entry.session_id.clone());
            entry.session_id.clone()
        })
}

pub fn session_path_for_id(home: &Path, cwd: &str, session_id: &str) -> Option<PathBuf> {
    let direct = claude_home(home)
        .join("projects")
        .join(encode_claude_project_dir(cwd))
        .join(format!("{session_id}.jsonl"));
    if direct.is_file() {
        return Some(direct);
    }
    find_session_by_id(&claude_home(home).join("projects"), session_id)
}

pub fn session_exists(home: &Path, session_id: &str) -> bool {
    find_session_by_id(&claude_home(home).join("projects"), session_id).is_some()
}

pub fn load_session_summary(home: &Path, cwd: &str, session_id: &str) -> Option<SessionSummary> {
    let path = session_path_for_id(home, cwd, session_id)?;
    let title = ai_title(&path).or_else(|| first_user_prompt(&path))?;
    Some(SessionSummary {
        generated_title: Some(title),
        session_summary: None,
        agent_name: Some("claude".into()),
    })
}

pub fn session_messaged_at(home: &Path, session_id: &str) -> Option<DateTime<Utc>> {
    let path = find_session_by_id(&claude_home(home).join("projects"), session_id)?;
    latest_user_prompt_at(&path)
}

pub fn session_activity_at(home: &Path, session_id: &str) -> Option<DateTime<Utc>> {
    let path = find_session_by_id(&claude_home(home).join("projects"), session_id)?;
    latest_event_at(&path).or_else(|| file_modified_at(&path))
}

pub fn session_cwd_for_id(home: &Path, session_id: &str) -> Option<String> {
    let path = find_session_by_id(&claude_home(home).join("projects"), session_id)?;
    session_cwd(&path)
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

fn collect_sessions(dir: &Path, by_cwd: &mut HashMap<String, Vec<ClaudeSessionEntry>>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sessions(&path, by_cwd);
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.ends_with(".jsonl") {
            continue;
        }
        let Some(session_id) = parse_session_id_from_name(file_name) else {
            continue;
        };
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        let Some(cwd) = session_cwd(&path) else {
            continue;
        };
        by_cwd
            .entry(normalize_cwd(&cwd))
            .or_default()
            .push(ClaudeSessionEntry {
                modified,
                session_id,
            });
    }
}

fn find_session_by_id(dir: &Path, session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_session_by_id(&path, session_id) {
                return Some(found);
            }
            continue;
        }
        let file_name = path.file_name()?.to_str()?;
        if file_name == format!("{session_id}.jsonl") {
            return Some(path);
        }
    }
    None
}

fn parse_session_id_from_name(file_name: &str) -> Option<String> {
    let stem = file_name.strip_suffix(".jsonl")?;
    (!stem.is_empty()).then(|| stem.to_string())
}

pub fn encode_claude_project_dir(cwd: &str) -> String {
    let cwd = normalize_cwd(cwd);
    if cwd == "/" {
        return "-".into();
    }
    format!("-{}", cwd.trim_start_matches('/').replace('/', "-"))
}

fn ai_title(path: &Path) -> Option<String> {
    let data = std::fs::read_to_string(path).ok()?;
    let mut title = None;
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("ai-title") {
            continue;
        }
        if let Some(next) = value.get("aiTitle").and_then(|v| v.as_str()) {
            let next = normalize_user_text(next);
            if !next.is_empty() {
                title = Some(next);
            }
        }
    }
    title
}

fn first_user_prompt(path: &Path) -> Option<String> {
    let data = std::fs::read_to_string(path).ok()?;
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(prompt) = user_prompt_text(&value) else {
            continue;
        };
        let prompt = shorten_prompt(&prompt);
        if is_confident_thread_title(&prompt) {
            return Some(prompt);
        }
    }
    None
}

fn latest_user_prompt_at(path: &Path) -> Option<DateTime<Utc>> {
    let data = std::fs::read_to_string(path).ok()?;
    let mut latest = None;
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if user_prompt_text(&value).is_none() {
            continue;
        }
        if let Some(at) = parse_claude_ts(value.get("timestamp")) {
            latest = Some(at);
        }
    }
    latest
}

fn latest_event_at(path: &Path) -> Option<DateTime<Utc>> {
    let data = std::fs::read_to_string(path).ok()?;
    let mut latest = None;
    for line in data.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(at) = parse_claude_ts(value.get("timestamp")) {
            return Some(at);
        }
        if let Some(at) = value
            .pointer("/snapshot/timestamp")
            .and_then(|ts| parse_claude_ts(Some(ts)))
        {
            latest = Some(at);
        }
    }
    latest
}

pub(super) fn parse_turn_boundary(path: &Path) -> Option<TurnBoundary> {
    let data = std::fs::read_to_string(path).ok()?;
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
        let kind = value.get("type").and_then(|v| v.as_str());
        if kind == Some("assistant") && last_completed.is_none() {
            let stop_reason = value
                .pointer("/message/stop_reason")
                .and_then(|v| v.as_str());
            if stop_reason == Some("end_turn") {
                last_completed = parse_claude_ts(value.get("timestamp"));
            }
        } else if kind == Some("user")
            && last_started.is_none()
            && user_prompt_text(&value).is_some()
        {
            last_started = parse_claude_ts(value.get("timestamp"));
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

fn session_cwd(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    use std::io::{BufRead, BufReader};
    let reader = BufReader::new(&mut file);
    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
            return Some(normalize_cwd(cwd));
        }
    }
    None
}

fn user_prompt_text(value: &Value) -> Option<String> {
    if value.get("type").and_then(|v| v.as_str()) != Some("user") {
        return None;
    }
    let message = value.get("message")?;
    if message.get("role").and_then(|v| v.as_str()) != Some("user") {
        return None;
    }
    let content = message.get("content")?;
    let text = match content {
        Value::String(text) => normalize_user_text(text),
        _ => return None,
    };
    if text.is_empty()
        || text.starts_with('/')
        || is_bootstrap_user_message(&text)
        || is_probe_prompt(&text)
    {
        return None;
    }
    Some(text)
}

fn normalize_user_text(text: &str) -> String {
    text.trim().trim_start_matches('❯').trim().to_string()
}

fn is_bootstrap_user_message(message: &str) -> bool {
    message.starts_with("# AGENTS")
        || message.contains("<INSTRUCTIONS>")
        || message.starts_with("<environment_context>")
}

fn is_probe_prompt(message: &str) -> bool {
    matches!(
        message.to_ascii_lowercase().as_str(),
        "testing" | "test" | "hi" | "hello" | "hey" | "ping" | "pong"
    )
}

fn parse_claude_ts(value: Option<&Value>) -> Option<DateTime<Utc>> {
    match value {
        Some(Value::String(ts)) => DateTime::parse_from_rfc3339(ts)
            .ok()
            .map(|ts| ts.with_timezone(&Utc)),
        _ => None,
    }
}

fn file_modified_at(path: &Path) -> Option<DateTime<Utc>> {
    let modified = path.metadata().ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| DateTime::from_timestamp(d.as_secs() as i64, 0))
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
    use super::*;
    use std::fs;

    use tempfile::TempDir;

    #[test]
    fn encode_claude_project_dir_matches_on_disk_layout() {
        let cwd = env!("CARGO_MANIFEST_DIR");
        let expected = format!("-{}", cwd.trim_start_matches('/').replace('/', "-"));
        assert_eq!(encode_claude_project_dir(cwd), expected);
    }

    #[test]
    fn load_claude_summary_prefers_ai_title() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let sid = "06b67c89-bd76-4922-b6ec-518172be4267";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let project_dir = claude_home(home)
            .join("projects")
            .join(encode_claude_project_dir(cwd));
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join(format!("{sid}.jsonl")),
            format!(
                r#"{{"type":"user","message":{{"role":"user","content":"fix scrolling"}},"cwd":"{cwd}","timestamp":"2026-06-11T06:38:24.372Z"}}
{{"type":"ai-title","aiTitle":"Implement scrolling for session CLI","sessionId":"06b67c89-bd76-4922-b6ec-518172be4267"}}"#
            ),
        )
        .unwrap();

        let summary = load_session_summary(home, cwd, sid).unwrap();
        assert_eq!(
            summary.generated_title.as_deref(),
            Some("Implement scrolling for session CLI")
        );
        assert_eq!(
            thread_title_from_summary(&summary).as_deref(),
            Some("Implement scrolling for session CLI")
        );
    }

    #[test]
    fn load_claude_summary_ignores_probe_prompts() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let sid = "bc4684da-6c2d-4a29-b79c-a8a9890dbd3d";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let project_dir = claude_home(home)
            .join("projects")
            .join(encode_claude_project_dir(cwd));
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join(format!("{sid}.jsonl")),
            format!(
                r#"{{"type":"user","message":{{"role":"user","content":"testing"}},"cwd":"{cwd}","timestamp":"2026-06-11T06:38:24.372Z"}}"#
            ),
        )
        .unwrap();

        assert!(load_session_summary(home, cwd, sid).is_none());
    }

    #[test]
    fn claude_session_index_assigns_sessions_per_cwd() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");
        let project_dir = claude_home(home)
            .join("projects")
            .join(encode_claude_project_dir(cwd));
        fs::create_dir_all(&project_dir).unwrap();
        for sid in [
            "06b67c89-bd76-4922-b6ec-518172be4267",
            "bc4684da-6c2d-4a29-b79c-a8a9890dbd3d",
        ] {
            fs::write(
                project_dir.join(format!("{sid}.jsonl")),
                format!(
                    r#"{{"type":"user","message":{{"role":"user","content":"task {sid}"}},"cwd":"{cwd}","timestamp":"2026-06-11T06:38:24.372Z"}}"#
                ),
            )
            .unwrap();
        }

        let index = claude_session_index(home);
        let mut assigned = HashSet::new();
        let first = assign_session_for_cwd(&index, cwd, &mut assigned).unwrap();
        let second = assign_session_for_cwd(&index, cwd, &mut assigned).unwrap();
        assert_ne!(first, second);
        assert_eq!(assigned.len(), 2);
    }

    #[test]
    fn parse_turn_boundary_reads_user_and_end_turn() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let sid = "06b67c89-bd76-4922-b6ec-518172be4267";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let project_dir = claude_home(home)
            .join("projects")
            .join(encode_claude_project_dir(cwd));
        fs::create_dir_all(&project_dir).unwrap();
        let path = project_dir.join(format!("{sid}.jsonl"));
        fs::write(
            &path,
            r#"{"type":"user","message":{"role":"user","content":"fix scrolling"},"timestamp":"2026-06-11T06:38:24.372Z"}
{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"},"timestamp":"2026-06-11T06:46:57.188Z"}
"#,
        )
        .unwrap();

        let boundary = parse_turn_boundary(&path).unwrap();
        assert_eq!(
            boundary.last_started.map(|ts| ts.to_rfc3339()),
            Some("2026-06-11T06:38:24.372+00:00".into())
        );
        assert_eq!(
            boundary.last_completed.map(|ts| ts.to_rfc3339()),
            Some("2026-06-11T06:46:57.188+00:00".into())
        );
        assert!(crate::agents::turn_is_complete(&boundary));
    }
}
