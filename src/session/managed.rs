use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static ID_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedLaunchRecord {
    pub sessions_session_id: String,
    pub launch_id: String,
    pub agent: String,
    pub tmux_session: String,
    pub window_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    pub initial_cwd: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    /// Detached pre-hydrated spare — hidden from the sidebar until claimed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pool: bool,
}

pub fn managed_state_dir(home: &Path) -> PathBuf {
    crate::paths::state_dir(home).join("managed")
}

pub fn managed_record_path(home: &Path, sessions_session_id: &str) -> PathBuf {
    managed_state_dir(home).join(format!("{sessions_session_id}.json"))
}

pub fn new_sessions_session_id() -> String {
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    let ms = Utc::now().timestamp_millis();
    format!("ssn_{ms}_{seq:04x}")
}

pub fn new_launch_id() -> String {
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    let ms = Utc::now().timestamp_millis();
    format!("lch_{ms}_{seq:04x}")
}

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Wrap an agent launch command with stable sessions identity env.
pub fn wrap_managed_launch_command(
    agent: &str,
    initial_cwd: &str,
    sessions_session_id: &str,
    inner_command: &str,
) -> String {
    let cwd = shell_quote(initial_cwd);
    let id = shell_quote(sessions_session_id);
    let agent_env = shell_quote(agent);
    // Color first: agent shells often inherit NO_COLOR from the tmux server.
    let color = crate::color_env::shell_exports();
    format!(
        "{color}; export SESSIONS_SESSION_ID={id} SESSIONS_AGENT={agent_env} SESSIONS_INITIAL_CWD={cwd}; {inner_command}"
    )
}

pub fn save_managed_record(home: &Path, record: &ManagedLaunchRecord) -> std::io::Result<()> {
    let dir = managed_state_dir(home);
    fs::create_dir_all(&dir)?;
    let path = managed_record_path(home, &record.sessions_session_id);
    let data = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
    fs::write(path, data)
}

pub fn load_managed_record(home: &Path, sessions_session_id: &str) -> Option<ManagedLaunchRecord> {
    let path = managed_record_path(home, sessions_session_id);
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn remove_managed_record(home: &Path, sessions_session_id: &str) -> std::io::Result<()> {
    let path = managed_record_path(home, sessions_session_id);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Drop managed launch records pinned to a window index that now hosts another `@sessions.id`.
pub fn gc_managed_records_superseded_at_window(
    home: &Path,
    tmux_session: &str,
    live_by_window: &HashMap<u32, String>,
) {
    let dir = managed_state_dir(home);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<ManagedLaunchRecord>(&data) else {
            continue;
        };
        if record.tmux_session != tmux_session {
            continue;
        }
        if let Some(live_ssn) = live_by_window.get(&record.window_index) {
            if live_ssn != &record.sessions_session_id {
                let _ = remove_managed_record(home, &record.sessions_session_id);
            }
        }
    }
}

pub fn update_managed_agent_session_id(
    home: &Path,
    sessions_session_id: &str,
    agent_session_id: &str,
) -> std::io::Result<()> {
    if crate::agents::parent_session_id_for_subagent(home, agent_session_id).is_some() {
        return Ok(());
    }
    let Some(mut record) = load_managed_record(home, sessions_session_id) else {
        return Ok(());
    };
    // Refuse cross-agent bindings (e.g. a Grok UUID on an OpenCode launch).
    // A poisoned managed record rehydrates every poll and groups the sidebar
    // row under the wrong project via group_cwd_for_session.
    if !crate::agents::agent_session_matches_expected_agent(home, agent_session_id, &record.agent) {
        return Ok(());
    }
    if record.agent_session_id.as_deref() == Some(agent_session_id) {
        return Ok(());
    }
    record.agent_session_id = Some(agent_session_id.to_string());
    save_managed_record(home, &record)
}

/// Clear a durable managed binding when it is known to be wrong (cross-agent
/// or otherwise rejected). No-op when the record is missing or already clear.
pub fn clear_managed_agent_session_id(
    home: &Path,
    sessions_session_id: &str,
    only_if_sid: Option<&str>,
) -> std::io::Result<()> {
    let Some(mut record) = load_managed_record(home, sessions_session_id) else {
        return Ok(());
    };
    let Some(current) = record.agent_session_id.as_deref() else {
        return Ok(());
    };
    if let Some(expected) = only_if_sid {
        if current != expected {
            return Ok(());
        }
    }
    record.agent_session_id = None;
    save_managed_record(home, &record)
}

#[derive(Debug, Default)]
pub struct ManagedLaunchIndex {
    by_window: HashMap<(String, u32), ManagedLaunchRecord>,
    by_pane: HashMap<String, ManagedLaunchRecord>,
    by_ssn: HashMap<String, ManagedLaunchRecord>,
}

pub fn load_managed_index(home: &Path) -> ManagedLaunchIndex {
    let dir = managed_state_dir(home);
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return ManagedLaunchIndex::default(),
    };
    let mut index = ManagedLaunchIndex::default();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<ManagedLaunchRecord>(&data) else {
            continue;
        };
        index.by_window.insert(
            (record.tmux_session.clone(), record.window_index),
            record.clone(),
        );
        index
            .by_ssn
            .insert(record.sessions_session_id.clone(), record.clone());
        if let Some(ref pane) = record.pane_id {
            index.by_pane.insert(pane.clone(), record);
        }
    }
    index
}

impl ManagedLaunchIndex {
    pub fn for_window(
        &self,
        tmux_session: &str,
        window_index: u32,
    ) -> Option<&ManagedLaunchRecord> {
        self.by_window
            .get(&(tmux_session.to_string(), window_index))
    }

    pub fn for_pane(&self, pane_id: &str) -> Option<&ManagedLaunchRecord> {
        self.by_pane.get(pane_id)
    }

    pub fn for_ssn(&self, sessions_session_id: &str) -> Option<&ManagedLaunchRecord> {
        self.by_ssn.get(sessions_session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn wrap_managed_launch_command_exports_identity_env() {
        let wrapped =
            wrap_managed_launch_command("codex", env!("CARGO_MANIFEST_DIR"), "ssn_test", "codex");
        assert!(wrapped.contains("SESSIONS_SESSION_ID=ssn_test"));
        assert!(wrapped.contains("SESSIONS_AGENT=codex"));
        assert!(wrapped.contains(&format!(
            "SESSIONS_INITIAL_CWD={}",
            env!("CARGO_MANIFEST_DIR")
        )));
        assert!(wrapped.ends_with("codex"));
        assert!(
            wrapped.contains("unset NO_COLOR"),
            "managed launch must clear NO_COLOR: {wrapped}"
        );
    }

    #[test]
    fn managed_index_finds_record_by_ssn() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let record = ManagedLaunchRecord {
            sessions_session_id: "ssn_by_ssn".into(),
            launch_id: "lch_by_ssn".into(),
            agent: "grok".into(),
            tmux_session: "agents".into(),
            window_index: 3,
            pane_id: Some("%3".into()),
            initial_cwd: "/tmp".into(),
            created_at: Utc::now().to_rfc3339(),
            agent_session_id: None,
            pool: false,
        };
        save_managed_record(home, &record).unwrap();
        let index = load_managed_index(home);
        assert_eq!(
            index
                .for_ssn("ssn_by_ssn")
                .map(|record| record.window_index),
            Some(3)
        );
    }

    #[test]
    fn managed_index_finds_record_by_window() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let record = ManagedLaunchRecord {
            sessions_session_id: "ssn_abc".into(),
            launch_id: "lch_abc".into(),
            agent: "codex".into(),
            tmux_session: "agents".into(),
            window_index: 7,
            pane_id: Some("%42".into()),
            initial_cwd: "/tmp".into(),
            created_at: Utc::now().to_rfc3339(),
            agent_session_id: None,
            pool: false,
        };
        save_managed_record(home, &record).unwrap();
        let index = load_managed_index(home);
        assert_eq!(
            index
                .for_window("agents", 7)
                .map(|r| r.sessions_session_id.as_str()),
            Some("ssn_abc")
        );
        assert_eq!(
            index
                .for_pane("%42")
                .map(|r| r.sessions_session_id.as_str()),
            Some("ssn_abc")
        );
    }

    #[test]
    fn update_managed_agent_session_id_persists_binding() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let record = ManagedLaunchRecord {
            sessions_session_id: "ssn_bind".into(),
            launch_id: "lch_bind".into(),
            agent: "grok".into(),
            tmux_session: "agents".into(),
            window_index: 3,
            pane_id: None,
            initial_cwd: "/tmp".into(),
            created_at: Utc::now().to_rfc3339(),
            agent_session_id: None,
            pool: false,
        };
        save_managed_record(home, &record).unwrap();
        update_managed_agent_session_id(home, "ssn_bind", "agent-sid-1").unwrap();
        let loaded = load_managed_record(home, "ssn_bind").unwrap();
        assert_eq!(loaded.agent_session_id.as_deref(), Some("agent-sid-1"));
    }

    #[test]
    fn update_managed_agent_session_id_rejects_cross_agent_sid() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let grok_sid = "019f1ab1-858d-7120-b097-3309bfd5b6c5";
        let sessions_cwd = env!("CARGO_MANIFEST_DIR");
        // Plant a Grok summary so detect_agent_id_for_session → "grok".
        let summary_dir = crate::agents::grok::session_dir(home, sessions_cwd, grok_sid);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            format!(r#"{{"info":{{"cwd":"{sessions_cwd}"}}}}"#),
        )
        .unwrap();

        let record = ManagedLaunchRecord {
            sessions_session_id: "ssn_opencode".into(),
            launch_id: "lch_opencode".into(),
            agent: "opencode".into(),
            tmux_session: "agents".into(),
            window_index: 5,
            pane_id: Some("%34".into()),
            initial_cwd: "/home/testuser".into(),
            created_at: Utc::now().to_rfc3339(),
            agent_session_id: None,
            pool: false,
        };
        save_managed_record(home, &record).unwrap();
        update_managed_agent_session_id(home, "ssn_opencode", grok_sid).unwrap();
        let loaded = load_managed_record(home, "ssn_opencode").unwrap();
        assert!(
            loaded.agent_session_id.is_none(),
            "must not bind a Grok SID onto an OpenCode managed launch"
        );
    }

    #[test]
    fn clear_managed_agent_session_id_removes_binding() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let record = ManagedLaunchRecord {
            sessions_session_id: "ssn_clear".into(),
            launch_id: "lch_clear".into(),
            agent: "opencode".into(),
            tmux_session: "agents".into(),
            window_index: 1,
            pane_id: None,
            initial_cwd: "/tmp".into(),
            created_at: Utc::now().to_rfc3339(),
            agent_session_id: Some("ses_wrong".into()),
            pool: false,
        };
        save_managed_record(home, &record).unwrap();
        clear_managed_agent_session_id(home, "ssn_clear", Some("ses_wrong")).unwrap();
        assert!(load_managed_record(home, "ssn_clear")
            .unwrap()
            .agent_session_id
            .is_none());
    }

    #[test]
    fn gc_managed_records_superseded_at_window_removes_stale_index_pin() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        for (ssn, index) in [("ssn_stale", 3u32), ("ssn_live", 3u32)] {
            save_managed_record(
                home,
                &ManagedLaunchRecord {
                    sessions_session_id: ssn.into(),
                    launch_id: format!("lch_{ssn}"),
                    agent: "grok".into(),
                    tmux_session: "agents".into(),
                    window_index: index,
                    pane_id: Some("%3".into()),
                    initial_cwd: "/tmp".into(),
                    created_at: Utc::now().to_rfc3339(),
                    agent_session_id: None,
                    pool: false,
                },
            )
            .unwrap();
        }
        let live = HashMap::from([(3, "ssn_live".into())]);
        gc_managed_records_superseded_at_window(home, "agents", &live);
        assert!(load_managed_record(home, "ssn_stale").is_none());
        assert!(load_managed_record(home, "ssn_live").is_some());
    }
}
