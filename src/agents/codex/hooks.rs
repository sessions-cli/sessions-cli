use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::agents::common::notify_binary::{command_uses_binary, hook_binary};

use super::paths::codex_home;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexHookHealth {
    Absent,
    NotFound,
    Configured,
    OutOfDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexHookStatus {
    pub health: CodexHookHealth,
    pub hook_files: usize,
    pub configured_files: usize,
}

const HOOK_SPECS: &[(&str, &str, &str)] = &[
    ("sessions-prompt", "prompt", " --stdin"),
    ("sessions-pre-tool", "pre_tool", ""),
    ("sessions-post-tool", "post_tool", ""),
    ("sessions-tool-fail", "tool_fail", ""),
    ("sessions-session-start", "session_start", " --stdin"),
];

pub fn present(home: &Path) -> bool {
    codex_home(home).is_dir() || crate::hooks::command_on_path("codex")
}

pub fn hooks_dir(home: &Path) -> PathBuf {
    codex_home(home).join("hooks")
}

pub fn status(home: &Path) -> CodexHookStatus {
    if !present(home) {
        return CodexHookStatus {
            health: CodexHookHealth::Absent,
            hook_files: 0,
            configured_files: 0,
        };
    }

    let expected = hook_binary(home).to_string_lossy().to_string();
    let dir = hooks_dir(home);
    let mut hook_files = 0usize;
    let mut configured_files = 0usize;

    for (name, _, _) in HOOK_SPECS {
        let path = dir.join(format!("{name}.json"));
        if !path.is_file() {
            continue;
        }
        hook_files += 1;
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(value) = serde_json::from_str::<Value>(&data) {
                if let Some(command) = value.get("command").and_then(|v| v.as_str()) {
                    if command_uses_binary(command, &expected) {
                        configured_files += 1;
                    }
                }
            }
        }
    }

    let health = if hook_files == 0 {
        CodexHookHealth::NotFound
    } else if configured_files == hook_files {
        CodexHookHealth::Configured
    } else {
        CodexHookHealth::OutOfDate
    };

    CodexHookStatus {
        health,
        hook_files,
        configured_files,
    }
}

pub fn setup(home: &Path) -> Result<usize> {
    if !present(home) {
        anyhow::bail!("codex not detected");
    }
    let binary = hook_binary(home);
    let dir = hooks_dir(home);
    std::fs::create_dir_all(&dir)?;
    let mut written = 0usize;
    for (name, event, extra) in HOOK_SPECS {
        let path = dir.join(format!("{name}.json"));
        let command = format!("{} notify --event {event}{extra}", binary.display());
        let value = json!({ "command": command });
        std::fs::write(&path, serde_json::to_string_pretty(&value)? + "\n")
            .with_context(|| format!("write {}", path.display()))?;
        written += 1;
    }
    Ok(written)
}

pub fn detail_label(status: &CodexHookStatus) -> String {
    match status.health {
        CodexHookHealth::Absent => "not installed".into(),
        CodexHookHealth::NotFound => "not configured".into(),
        CodexHookHealth::Configured => "configured".into(),
        CodexHookHealth::OutOfDate => format!(
            "out of date ({}/{})",
            status.configured_files, status.hook_files
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
    fn setup_writes_codex_hook_files() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(codex_home(home)).unwrap();
        install_fake_binary(home);
        let written = setup(home).unwrap();
        assert_eq!(written, HOOK_SPECS.len());
        let current = status(home);
        assert_eq!(current.health, CodexHookHealth::Configured);
    }
}
