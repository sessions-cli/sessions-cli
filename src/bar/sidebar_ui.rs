use crate::config::Config;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SidebarUi {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded_groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_sessions_session_id: Option<String>,
}

pub fn load(config: &Config) -> SidebarUi {
    let path = config.sidebar_ui_path();
    if !path.exists() {
        return SidebarUi::default();
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

pub fn save(
    config: &Config,
    expanded_groups: &HashSet<String>,
    selected_sessions_session_id: Option<&str>,
) -> Result<()> {
    let mut groups: Vec<String> = expanded_groups.iter().cloned().collect();
    groups.sort();
    atomic_write_json(
        &config.sidebar_ui_path(),
        &SidebarUi {
            expanded_groups: groups,
            selected_sessions_session_id: selected_sessions_session_id.map(str::to_string),
        },
    )
}

fn atomic_write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let data = serde_json::to_string_pretty(value)?;
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut expanded = HashSet::new();
        expanded.insert("~/projects/a".into());
        expanded.insert("~/projects/b".into());
        save(&config, &expanded, Some("ssn_test_123")).unwrap();
        let loaded = load(&config);
        assert_eq!(
            loaded,
            SidebarUi {
                expanded_groups: vec!["~/projects/a".into(), "~/projects/b".into()],
                selected_sessions_session_id: Some("ssn_test_123".into()),
            }
        );
    }
}