use anyhow::{bail, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub use crate::agents::common::notify_binary::{command_uses_binary, hook_binary};

use crate::agents::common::notify_binary::{is_sessions_notify_command, rewrite_notify_command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrokHookHealth {
    /// ~/.grok/hooks is missing — Grok may not be installed.
    Absent,
    /// No sessions notify commands found in hook files or config.
    NotFound,
    /// Every notify command points at the installed sessions binary.
    Configured,
    /// Some notify commands point at a stale or missing binary.
    OutOfDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokHookStatus {
    pub health: GrokHookHealth,
    pub notify_commands: usize,
    pub configured_commands: usize,
    pub hooks_dir: PathBuf,
    pub config_path: PathBuf,
    pub sessions_binary: PathBuf,
}

impl GrokHookStatus {
    pub fn detail_label(&self) -> String {
        match self.health {
            GrokHookHealth::Absent => "Grok not installed".into(),
            GrokHookHealth::NotFound => "No hooks found · ↵ install".into(),
            GrokHookHealth::Configured => "configured".into(),
            GrokHookHealth::OutOfDate => format!(
                "Out of date ({}/{}) · ↵ repair",
                self.configured_commands, self.notify_commands
            ),
        }
    }

    pub fn needs_setup(&self) -> bool {
        matches!(
            self.health,
            GrokHookHealth::NotFound | GrokHookHealth::OutOfDate
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokHookSetupResult {
    pub hook_file_changes: usize,
    pub config_changes: usize,
    pub sessions_binary: PathBuf,
}

const SESSIONS_HOOK_FILE: &str = "sessions-cli.json";

/// Grok lifecycle hook name, sessions notify event, read prompt JSON from stdin.
const LIFECYCLE_HOOKS: &[(&str, &str, bool)] = &[
    ("SessionStart", "session_start", true),
    ("UserPromptSubmit", "prompt", true),
    ("PreToolUse", "pre_tool", false),
    ("PostToolUse", "post_tool", false),
    ("PostToolUseFailure", "tool_fail", false),
];

pub fn present(home: &Path) -> bool {
    grok_home(home).is_dir() || crate::hooks::command_on_path("grok")
}

pub fn grok_home(home: &Path) -> PathBuf {
    std::env::var_os("GROK_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".grok"))
}

pub fn hooks_dir(home: &Path) -> PathBuf {
    grok_home(home).join("hooks")
}

pub fn config_path(home: &Path) -> PathBuf {
    grok_home(home).join("config.toml")
}

pub fn status(home: &Path) -> GrokHookStatus {
    let hooks_dir = hooks_dir(home);
    let config_path = config_path(home);
    let sessions_binary = hook_binary(home);
    let expected = sessions_binary.to_string_lossy();

    if !present(home) {
        return GrokHookStatus {
            health: GrokHookHealth::Absent,
            notify_commands: 0,
            configured_commands: 0,
            hooks_dir,
            config_path,
            sessions_binary,
        };
    }

    let mut notify_commands = 0usize;
    let mut configured_commands = 0usize;

    if let Ok(entries) = std::fs::read_dir(&hooks_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(data) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            scan_notify_commands(
                &value,
                &expected,
                &mut notify_commands,
                &mut configured_commands,
            );
        }
    }

    if let Ok(text) = std::fs::read_to_string(&config_path) {
        for line in text.lines() {
            let Some(command) = extract_toml_command_value(line) else {
                continue;
            };
            if !is_sessions_notify_command(command) {
                continue;
            }
            notify_commands += 1;
            if command_uses_binary(command, &expected) {
                configured_commands += 1;
            }
        }
    }

    let health = if notify_commands == 0 {
        GrokHookHealth::NotFound
    } else if configured_commands == notify_commands {
        GrokHookHealth::Configured
    } else {
        GrokHookHealth::OutOfDate
    };

    GrokHookStatus {
        health,
        notify_commands,
        configured_commands,
        hooks_dir,
        config_path,
        sessions_binary,
    }
}

pub fn setup(home: &Path) -> Result<GrokHookSetupResult> {
    let current = status(home);
    if !present(home) {
        bail!("grok not detected");
    }
    if !current.hooks_dir.is_dir() {
        std::fs::create_dir_all(&current.hooks_dir)?;
    }

    let sessions_binary = current.sessions_binary.clone();
    let expected = sessions_binary.to_string_lossy().to_string();
    let mut hook_file_changes = 0usize;
    let mut config_changes = 0usize;

    if let Ok(entries) = std::fs::read_dir(&current.hooks_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let mut value: Value =
                serde_json::from_str(&data).with_context(|| format!("parse {}", path.display()))?;
            let changes = rewrite_hook_json(&mut value, &expected);
            if changes > 0 {
                let out = serde_json::to_string_pretty(&value)? + "\n";
                std::fs::write(&path, out).with_context(|| format!("write {}", path.display()))?;
                hook_file_changes += changes;
            }
        }
    }

    if write_lifecycle_hook_file(&current.hooks_dir, &sessions_binary)? {
        hook_file_changes += 1;
    }

    let config_text = if current.config_path.is_file() {
        std::fs::read_to_string(&current.config_path)?
    } else {
        String::new()
    };
    let (rewritten, rewrite_changes) = rewrite_config_toml(&config_text, &expected);
    let (updated, ensure_changes) = ensure_config_notifications(&rewritten, &expected);
    if rewrite_changes > 0 || ensure_changes > 0 {
        if let Some(parent) = current.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&current.config_path, updated)?;
        config_changes += rewrite_changes + ensure_changes;
    }

    Ok(GrokHookSetupResult {
        hook_file_changes,
        config_changes,
        sessions_binary,
    })
}

fn scan_notify_commands(
    node: &Value,
    expected: &str,
    notify_commands: &mut usize,
    configured_commands: &mut usize,
) {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(command)) = map.get("command") {
                if is_sessions_notify_command(command) {
                    *notify_commands += 1;
                    if command_uses_binary(command, expected) {
                        *configured_commands += 1;
                    }
                }
            }
            for value in map.values() {
                scan_notify_commands(value, expected, notify_commands, configured_commands);
            }
        }
        Value::Array(items) => {
            for item in items {
                scan_notify_commands(item, expected, notify_commands, configured_commands);
            }
        }
        _ => {}
    }
}

fn rewrite_hook_json(node: &mut Value, expected: &str) -> usize {
    match node {
        Value::Object(map) => {
            let mut changes = 0usize;
            if let Some(Value::String(command)) = map.get_mut("command") {
                if let Some(updated) = rewrite_notify_command(command, expected) {
                    *command = updated;
                    changes += 1;
                }
            }
            for value in map.values_mut() {
                changes += rewrite_hook_json(value, expected);
            }
            changes
        }
        Value::Array(items) => items
            .iter_mut()
            .map(|item| rewrite_hook_json(item, expected))
            .sum(),
        _ => 0,
    }
}

fn write_lifecycle_hook_file(dir: &Path, binary: &Path) -> Result<bool> {
    let path = dir.join(SESSIONS_HOOK_FILE);
    let mut hooks_obj = serde_json::Map::new();
    for (grok_event, sessions_event, stdin) in LIFECYCLE_HOOKS {
        let suffix = if *stdin { " --stdin" } else { "" };
        let command = format!(
            "{} notify --event {sessions_event}{suffix}",
            binary.display()
        );
        hooks_obj.insert(
            (*grok_event).into(),
            json!([{
                "hooks": [{
                    "type": "command",
                    "command": command,
                }]
            }]),
        );
    }
    let value = json!({ "hooks": hooks_obj });
    let new_text = serde_json::to_string_pretty(&value)? + "\n";
    let changed = std::fs::read_to_string(&path)
        .ok()
        .is_none_or(|existing| existing != new_text);
    std::fs::write(&path, new_text).with_context(|| format!("write {}", path.display()))?;
    Ok(changed)
}

fn has_turn_complete_notify(text: &str) -> bool {
    text.contains("notify --event turn_complete")
}

fn has_approval_required_notify(text: &str) -> bool {
    text.contains("notify --event approval_required")
}

fn ensure_config_notifications(text: &str, expected: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut changes = 0usize;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.contains("[ui.notifications]") {
        out.push_str("\n[ui.notifications]\ncondition = \"always\"\n");
    }
    if !has_turn_complete_notify(&out) {
        out.push_str(&format!(
            "\n[[ui.notifications.hooks]]\ncommand = \"{expected} notify --event turn_complete\"\nevents = [\"turn_complete\"]\nonly_unfocused = false\n"
        ));
        changes += 1;
    }
    // True "needs assistance" — Grok's approval_required notification (not PreToolUse).
    if !has_approval_required_notify(&out) {
        out.push_str(&format!(
            "\n[[ui.notifications.hooks]]\ncommand = \"{expected} notify --event approval_required\"\nevents = [\"approval_required\"]\nonly_unfocused = false\n"
        ));
        changes += 1;
    }
    (out, changes)
}

fn rewrite_config_toml(text: &str, expected: &str) -> (String, usize) {
    let re = Regex::new(r#"^(command\s*=\s*")[^"]*sessions notify"#).expect("valid regex");
    let mut changes = 0usize;
    let mut out = String::new();
    for line in text.lines() {
        if line.contains("sessions notify") && line.trim_start().starts_with("command") {
            let replaced = re.replace(line, format!(r#"$1{expected} notify"#));
            if replaced != line {
                changes += 1;
            }
            out.push_str(&replaced);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    (out, changes)
}

fn extract_toml_command_value(line: &str) -> Option<&str> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"^command\s*=\s*"(.+)"\s*$"#).expect("valid regex"));
    re.captures(line.trim())
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn setup_updates_hook_json_and_config() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let grok = home.join(".grok");
        let hooks = grok.join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(
            hooks.join("kitty-tab.json"),
            r#"{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/old/.grok/scripts/sessions notify --event prompt"
          }
        ]
      }
    ]
  }
}"#,
        )
        .unwrap();
        fs::write(
            grok.join("config.toml"),
            r#"[[ui.notifications.hooks]]
command = "/old/.grok/scripts/sessions notify --event turn_complete"
events = ["turn_complete"]
"#,
        )
        .unwrap();
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

        let result = setup(home).unwrap();
        assert_eq!(result.hook_file_changes, 2);
        // rewrite turn_complete binary path + append approval_required hook
        assert!(result.config_changes >= 2);

        let hook_text = fs::read_to_string(hooks.join("kitty-tab.json")).unwrap();
        assert!(hook_text.contains(&format!(
            "{} notify --event prompt",
            result.sessions_binary.display()
        )));
        let config_text = fs::read_to_string(grok.join("config.toml")).unwrap();
        assert!(config_text.contains(&format!(
            "command = \"{} notify --event turn_complete\"",
            result.sessions_binary.display()
        )));
        assert!(config_text.contains("notify --event approval_required"));

        let current = status(home);
        assert_eq!(
            current.health,
            GrokHookHealth::Configured,
            "notify={} configured={} binary={}",
            current.notify_commands,
            current.configured_commands,
            current.sessions_binary.display()
        );
    }

    #[test]
    fn setup_bootstraps_hooks_on_fresh_grok_install() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".grok")).unwrap();
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

        let result = setup(home).unwrap();
        assert!(result.hook_file_changes >= 1);
        assert!(result.config_changes >= 1);

        let hook_path = home.join(".grok/hooks/sessions-cli.json");
        assert!(hook_path.is_file());
        let hook_text = fs::read_to_string(&hook_path).unwrap();
        assert!(hook_text.contains("UserPromptSubmit"));
        assert!(hook_text.contains("notify --event prompt --stdin"));

        let config_text = fs::read_to_string(home.join(".grok/config.toml")).unwrap();
        assert!(config_text.contains("notify --event turn_complete"));
        assert!(config_text.contains("notify --event approval_required"));
        assert!(config_text.contains("only_unfocused = false"));

        let current = status(home);
        assert_eq!(current.health, GrokHookHealth::Configured);
    }

    #[test]
    fn setup_adds_approval_required_when_only_turn_complete_exists() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let grok = home.join(".grok");
        fs::create_dir_all(grok.join("hooks")).unwrap();
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
        fs::write(
            grok.join("config.toml"),
            format!(
                r#"[ui.notifications]
condition = "always"

[[ui.notifications.hooks]]
command = "{} notify --event turn_complete"
events = ["turn_complete"]
only_unfocused = false
"#,
                installed.display()
            ),
        )
        .unwrap();

        let result = setup(home).unwrap();
        assert!(result.config_changes >= 1);
        let config_text = fs::read_to_string(grok.join("config.toml")).unwrap();
        assert!(config_text.contains("notify --event approval_required"));
        assert!(config_text.contains("events = [\"approval_required\"]"));
    }

    #[test]
    fn status_reports_out_of_date_when_binary_differs() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let hooks = home.join(".grok/hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(
            hooks.join("cmux-session.json"),
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"command":"/stale/sessions notify --event post_tool"}]}]}}"#,
        )
        .unwrap();
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

        let current = status(home);
        assert_eq!(current.health, GrokHookHealth::OutOfDate);
        assert_eq!(current.notify_commands, 1);
        assert_eq!(current.configured_commands, 0);
    }
}
