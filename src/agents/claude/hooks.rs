use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::agents::common::notify_binary::{command_uses_binary, hook_binary};
fn claude_home(home: &Path) -> PathBuf {
    home.join(".claude")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeHookHealth {
    Absent,
    NotFound,
    Configured,
    OutOfDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeHookStatus {
    pub health: ClaudeHookHealth,
    pub configured_events: usize,
    pub expected_events: usize,
}

const HOOK_EVENTS: &[(&str, &str, bool)] = &[
    ("SessionStart", "session_start", true),
    ("UserPromptSubmit", "prompt", true),
    ("PreToolUse", "pre_tool", false),
    ("PostToolUse", "post_tool", false),
    ("Stop", "turn_complete", false),
];

pub fn present(home: &Path) -> bool {
    claude_home(home).is_dir() || crate::hooks::command_on_path("claude")
}

pub fn settings_path(home: &Path) -> PathBuf {
    claude_home(home).join("settings.json")
}

pub fn status(home: &Path) -> ClaudeHookStatus {
    if !present(home) {
        return ClaudeHookStatus {
            health: ClaudeHookHealth::Absent,
            configured_events: 0,
            expected_events: HOOK_EVENTS.len(),
        };
    }

    let expected = hook_binary(home).to_string_lossy().to_string();
    let path = settings_path(home);
    if !path.is_file() {
        return ClaudeHookStatus {
            health: ClaudeHookHealth::NotFound,
            configured_events: 0,
            expected_events: HOOK_EVENTS.len(),
        };
    }

    let Ok(data) = std::fs::read_to_string(&path) else {
        return ClaudeHookStatus {
            health: ClaudeHookHealth::NotFound,
            configured_events: 0,
            expected_events: HOOK_EVENTS.len(),
        };
    };
    let Ok(value) = serde_json::from_str::<Value>(&data) else {
        return ClaudeHookStatus {
            health: ClaudeHookHealth::NotFound,
            configured_events: 0,
            expected_events: HOOK_EVENTS.len(),
        };
    };

    let mut configured = 0usize;
    for (event, _, _) in HOOK_EVENTS {
        if hook_event_uses_binary(&value, event, &expected) {
            configured += 1;
        }
    }

    let health = if configured == 0 {
        ClaudeHookHealth::NotFound
    } else if configured == HOOK_EVENTS.len() {
        ClaudeHookHealth::Configured
    } else {
        ClaudeHookHealth::OutOfDate
    };

    ClaudeHookStatus {
        health,
        configured_events: configured,
        expected_events: HOOK_EVENTS.len(),
    }
}

pub fn setup(home: &Path) -> Result<usize> {
    if !present(home) {
        anyhow::bail!("claude not detected");
    }
    let binary = hook_binary(home);
    let path = settings_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut data: Value = if path.is_file() {
        let raw = std::fs::read_to_string(&path)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    let hooks = data
        .as_object_mut()
        .context("settings root must be object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));

    let hooks_obj = hooks.as_object_mut().context("hooks must be object")?;

    for (event, notify_event, stdin) in HOOK_EVENTS {
        let suffix = if *stdin { " --stdin" } else { "" };
        let command = format!("{} notify --event {notify_event}{suffix}", binary.display());
        let entry = if *event == "SessionStart" {
            // SessionStart fires on resume; matcher limits hook to resume events only.
            json!([{
                "matcher": "resume",
                "hooks": [{
                    "type": "command",
                    "command": command,
                }]
            }])
        } else {
            json!([{
                "hooks": [{
                    "type": "command",
                    "command": command,
                }]
            }])
        };
        hooks_obj.insert((*event).into(), entry);
    }

    std::fs::write(&path, serde_json::to_string_pretty(&data)? + "\n")
        .with_context(|| format!("write {}", path.display()))?;
    Ok(HOOK_EVENTS.len())
}

fn hook_event_uses_binary(data: &Value, event: &str, expected: &str) -> bool {
    let Some(entries) = data
        .get("hooks")
        .and_then(|v| v.get(event))
        .and_then(|v| v.as_array())
    else {
        return false;
    };
    for entry in entries {
        let Some(inner) = entry.get("hooks").and_then(|v| v.as_array()) else {
            continue;
        };
        for hook in inner {
            let Some(command) = hook.get("command").and_then(|v| v.as_str()) else {
                continue;
            };
            if command_uses_binary(command, expected) {
                return true;
            }
        }
    }
    false
}

pub fn detail_label(status: &ClaudeHookStatus) -> String {
    match status.health {
        ClaudeHookHealth::Absent => "not installed".into(),
        ClaudeHookHealth::NotFound => "not configured".into(),
        ClaudeHookHealth::Configured => "configured".into(),
        ClaudeHookHealth::OutOfDate => format!(
            "out of date ({}/{})",
            status.configured_events, status.expected_events
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn install_fake_binary(home: &Path) {
        let installed = home.join(".local/share/sessions/bin/sessions");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::write(&installed, b"#!/bin/sh\ntrue\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&installed).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&installed, perms).unwrap();
        }
    }

    #[test]
    fn setup_merges_claude_settings_hooks() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(claude_home(home)).unwrap();
        install_fake_binary(home);
        let written = setup(home).unwrap();
        assert_eq!(written, HOOK_EVENTS.len());
        assert_eq!(written, 5);

        let current = status(home);
        assert_eq!(current.health, ClaudeHookHealth::Configured);
        assert_eq!(current.configured_events, 5);
        assert_eq!(current.expected_events, 5);

        let settings_text = fs::read_to_string(settings_path(home)).unwrap();
        let settings: Value = serde_json::from_str(&settings_text).unwrap();
        let session_start = settings
            .get("hooks")
            .and_then(|h| h.get("SessionStart"))
            .and_then(|v| v.as_array())
            .expect("SessionStart hook");
        assert_eq!(
            session_start[0].get("matcher").and_then(|v| v.as_str()),
            Some("resume")
        );
        let command = session_start[0]
            .get("hooks")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|h| h.get("command"))
            .and_then(|v| v.as_str())
            .expect("SessionStart command");
        assert!(command.contains("notify --event session_start --stdin"));
    }
}
