use crate::config::Config;
use crate::model::{PersistedState, Session};
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn load_state(config: &Config) -> Result<PersistedState> {
    if !config.state_path.exists() {
        return Ok(empty_state());
    }

    let data = fs::read_to_string(&config.state_path)
        .with_context(|| format!("read {}", config.state_path.display()))?;
    match serde_json::from_str(&data) {
        Ok(state) => Ok(state),
        Err(err) => {
            quarantine_invalid_state(&config.state_path)?;
            Err(anyhow::anyhow!(
                "invalid persisted state at {}: {err}",
                config.state_path.display()
            ))
        }
    }
}

pub fn load_state_or_empty(config: &Config) -> PersistedState {
    load_state(config).unwrap_or_else(|_| empty_state())
}

pub fn save_state(config: &Config, sessions: &[Session], version: u64) -> Result<()> {
    let state = PersistedState {
        sessions: sessions.to_vec(),
        version,
    };
    atomic_write_json(&config.state_path, &state)
}

fn empty_state() -> PersistedState {
    PersistedState {
        sessions: Vec::new(),
        version: 0,
    }
}

fn quarantine_invalid_state(path: &Path) -> Result<()> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let corrupt = path.with_extension(format!("corrupt.{ts}.json"));
    fs::rename(path, &corrupt)
        .with_context(|| format!("rename {} -> {}", path.display(), corrupt.display()))
}

fn atomic_write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let data = serde_json::to_string(value)?;
    {
        let mut file = File::create(&tmp)?;
        file.write_all(data.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentState, Session};
    use chrono::Utc;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.state_path = dir.path().join("sessionsd.json");
        let session = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: "agents".into(),
            tmux_pane_id: "%0".into(),
            pane_pid: 0,
            agent_session_id: None,
            title: "test".into(),
            description: "test".into(),
            cwd: "/tmp".into(),
            cwd_label: "/tmp".into(),
            project: "other".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now()),
            prompt_submitted: true,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        save_state(&config, &[session], 1).unwrap();
        let loaded = load_state(&config).unwrap();
        assert_eq!(loaded.sessions.len(), 1);
        assert_eq!(loaded.version, 1);
    }

    #[test]
    fn invalid_state_is_quarantined() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.state_path = dir.path().join("sessionsd.json");
        fs::write(&config.state_path, "{not-json").unwrap();
        let loaded = load_state_or_empty(&config);
        assert!(loaded.sessions.is_empty());
        assert!(fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains("corrupt")));
    }
}
