use crate::config::Config;
use crate::model::Session;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Durable allowlist of managed sessions that were live while the daemon was healthy.
///
/// Cold-boot restore uses this so open-but-stale manifest rows (windows that died
/// without a clean close, or old workspace bootstrap slots) do not reappear as
/// phantom PWD groups after `sessions up`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LiveSessionSnapshot {
    pub sessions_session_ids: Vec<String>,
}

pub fn load(config: &Config) -> Option<HashSet<String>> {
    let path = config.live_session_snapshot_path();
    if !path.exists() {
        return None;
    }
    let data = fs::read_to_string(&path).ok()?;
    let snapshot: LiveSessionSnapshot = serde_json::from_str(&data).ok()?;
    if snapshot.sessions_session_ids.is_empty() {
        // Empty file means "intentionally nothing was live" — still authoritative.
        return Some(HashSet::new());
    }
    Some(snapshot.sessions_session_ids.into_iter().collect())
}

pub fn save(config: &Config, sessions_session_ids: impl IntoIterator<Item = String>) -> Result<()> {
    let mut ids: Vec<String> = sessions_session_ids.into_iter().collect();
    ids.sort();
    ids.dedup();
    let next = LiveSessionSnapshot {
        sessions_session_ids: ids,
    };
    // Avoid rewriting the file every poll when the live set is stable.
    if let Some(existing) = load_raw(config) {
        if existing == next {
            return Ok(());
        }
    }
    atomic_write_json(&config.live_session_snapshot_path(), &next)
}

fn load_raw(config: &Config) -> Option<LiveSessionSnapshot> {
    let path = config.live_session_snapshot_path();
    if !path.exists() {
        return None;
    }
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn save_from_sessions(config: &Config, sessions: &[Session]) -> Result<()> {
    save(config, ssns_from_sessions(sessions))
}

pub fn remember(config: &Config, sessions_session_id: &str) -> Result<()> {
    let mut ids = load(config).unwrap_or_default();
    if !ids.insert(sessions_session_id.to_string()) {
        return Ok(());
    }
    save(config, ids)
}

pub fn ssns_from_sessions(sessions: &[Session]) -> Vec<String> {
    sessions
        .iter()
        .filter_map(|session| session.sessions_session_id.clone())
        .collect()
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<()> {
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
    use crate::config::Config;
    use tempfile::TempDir;

    fn isolated_config(dir: &TempDir) -> Config {
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        config
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let config = isolated_config(&dir);
        save(&config, ["ssn_a".into(), "ssn_b".into(), "ssn_a".into()]).unwrap();
        let loaded = load(&config).expect("snapshot");
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains("ssn_a"));
        assert!(loaded.contains("ssn_b"));
    }

    #[test]
    fn empty_snapshot_is_authoritative() {
        let dir = TempDir::new().unwrap();
        let config = isolated_config(&dir);
        save(&config, std::iter::empty::<String>()).unwrap();
        let loaded = load(&config).expect("empty snapshot");
        assert!(loaded.is_empty());
    }

    #[test]
    fn remember_adds_without_dropping_existing() {
        let dir = TempDir::new().unwrap();
        let config = isolated_config(&dir);
        save(&config, ["ssn_a".into()]).unwrap();
        remember(&config, "ssn_b").unwrap();
        let loaded = load(&config).unwrap();
        assert!(loaded.contains("ssn_a"));
        assert!(loaded.contains("ssn_b"));
    }
}
