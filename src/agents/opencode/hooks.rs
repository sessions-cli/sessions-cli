use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const SESSIONS_TS: &str = include_str!("../../../integrations/opencode/sessions.ts");
const PACKAGE_JSON: &str = include_str!("../../../integrations/opencode/package.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeHookHealth {
    Absent,
    NotFound,
    Configured,
    OutOfDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeHookStatus {
    pub health: OpenCodeHookHealth,
    pub plugin_installed: bool,
    pub plugin_enabled: bool,
}

fn opencode_data_dir(home: &Path) -> PathBuf {
    home.join(".local/share/opencode")
}

pub fn present(home: &Path) -> bool {
    config_dir(home).is_dir()
        || opencode_data_dir(home).is_dir()
        || crate::hooks::command_on_path("opencode")
}

pub fn config_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("opencode")
}

pub fn plugin_dir(home: &Path) -> PathBuf {
    config_dir(home).join("plugins")
}

pub fn config_path(home: &Path) -> PathBuf {
    config_dir(home).join("opencode.json")
}

pub fn status(home: &Path) -> OpenCodeHookStatus {
    if !present(home) {
        return OpenCodeHookStatus {
            health: OpenCodeHookHealth::Absent,
            plugin_installed: false,
            plugin_enabled: false,
        };
    }

    let plugin = plugin_dir(home).join("sessions.ts");
    let plugin_installed = plugin.is_file() && plugin_dir(home).join("package.json").is_file();
    let plugin_enabled = config_path(home).is_file()
        && plugin_enabled_in_config(&config_path(home));

    let health = if !plugin_installed {
        OpenCodeHookHealth::NotFound
    } else if plugin_enabled {
        OpenCodeHookHealth::Configured
    } else {
        OpenCodeHookHealth::OutOfDate
    };

    OpenCodeHookStatus {
        health,
        plugin_installed,
        plugin_enabled,
    }
}

pub fn setup(home: &Path) -> Result<usize> {
    if !present(home) {
        anyhow::bail!("opencode not detected");
    }
    let dir = plugin_dir(home);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("sessions.ts"), SESSIONS_TS)
        .with_context(|| format!("write {}", dir.join("sessions.ts").display()))?;
    std::fs::write(dir.join("package.json"), PACKAGE_JSON)
        .with_context(|| format!("write {}", dir.join("package.json").display()))?;

    let mut changes = 2usize;

    // Write the installed sessions binary path so the plugin can find it
    // without relying on PATH resolution in the opencode process environment.
    let binary = crate::agents::common::notify_binary::hook_binary(home);
    let bin_path = dir.join("sessions-bin");
    std::fs::write(&bin_path, binary.to_string_lossy().as_bytes())
        .with_context(|| format!("write {}", bin_path.display()))?;
    changes += 1;

    // Ensure opencode.json exists with the "sessions" plugin enabled.
    let config = config_path(home);
    let raw = if config.is_file() {
        std::fs::read_to_string(&config)?
    } else {
        String::new()
    };
    let mut data: Value = if raw.is_empty() {
        json!({})
    } else {
        serde_json::from_str(&raw)?
    };
    let plugins = data
        .as_object_mut()
        .context("opencode config root must be object")?
        .entry("plugin")
        .or_insert_with(|| json!([]));
    let list = plugins.as_array_mut().context("plugin must be array")?;
    if !list.iter().any(|entry| entry.as_str() == Some("sessions")) {
        list.push(json!("sessions"));
        if let Some(parent) = config.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&config, serde_json::to_string_pretty(&data)? + "\n")?;
        changes += 1;
    }
    Ok(changes)
}

fn plugin_enabled_in_config(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(data) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    data.get("plugin")
        .and_then(|v| v.as_array())
        .is_some_and(|items| items.iter().any(|entry| entry.as_str() == Some("sessions")))
}

pub fn detail_label(status: &OpenCodeHookStatus) -> String {
    match status.health {
        OpenCodeHookHealth::Absent => "not installed".into(),
        OpenCodeHookHealth::NotFound => "plugin not installed".into(),
        OpenCodeHookHealth::Configured => "configured".into(),
        OpenCodeHookHealth::OutOfDate => "plugin not enabled in opencode.json".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn setup_installs_opencode_plugin() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(config_dir(home)).unwrap();
        fs::write(config_path(home), r#"{"plugin":["other"]}"#).unwrap();
        let changes = setup(home).unwrap();
        assert!(changes >= 2);
        let current = status(home);
        assert_eq!(current.health, OpenCodeHookHealth::Configured);
        assert!(current.plugin_enabled);
    }
}
