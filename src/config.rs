use crate::paths;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub socket_path: PathBuf,
    pub state_path: PathBuf,
    pub spool_dir: PathBuf,
    pub log_path: PathBuf,
    pub poll_interval_ms: u64,
    pub persist_interval_secs: u64,
    pub home: PathBuf,
    pub tmux_session: String,
    pub tmux_ui_session: String,
    pub workspaces_path: PathBuf,
    pub tmux_state_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let home = paths::home();
        let data_root = paths::data_root(&home);
        Self {
            socket_path: home.join(".local/run/sessionsd.sock"),
            state_path: data_root.join("state/sessionsd.json"),
            spool_dir: paths::spool_dir(&home),
            log_path: paths::logs_dir(&home).join("sessionsd.log"),
            poll_interval_ms: 1500,
            persist_interval_secs: 5,
            home: home.clone(),
            tmux_session: "agents".into(),
            tmux_ui_session: "sessions-ui".into(),
            workspaces_path: Self::resolve_workspaces_path(&home),
            tmux_state_dir: data_root.join("state/tab-status"),
        }
    }
}

impl Config {
    fn resolve_workspaces_path(home: &std::path::Path) -> PathBuf {
        let primary = home.join(".config/sessions/workspaces.toml");
        if primary.exists() {
            return primary;
        }
        home.join(".config/kitty/workspaces.toml")
    }

    pub fn agent_state_dir(&self) -> PathBuf {
        paths::state_dir(&self.home)
    }

    /// Backward-compat alias for agent state directory.
    pub fn grok_state_dir(&self) -> PathBuf {
        self.agent_state_dir()
    }

    pub fn session_env_path(&self, agent_session_id: &str) -> PathBuf {
        self.agent_state_dir()
            .join(format!("{agent_session_id}.env"))
    }

    pub fn session_title_path(&self, agent_session_id: &str) -> PathBuf {
        self.agent_state_dir()
            .join(format!("{agent_session_id}.title"))
    }

    pub fn session_title_path_for_tab(&self, tab_index: u32) -> PathBuf {
        self.agent_state_dir()
            .join(format!("tmux-win-{tab_index}.title"))
    }

    pub fn session_title_manual_path_for_tab(&self, tab_index: u32) -> PathBuf {
        self.agent_state_dir()
            .join(format!("tmux-win-{tab_index}.title.manual"))
    }

    pub fn sidebar_group_order_path(&self) -> PathBuf {
        paths::state_dir(&self.home).join("sidebar-group-order.json")
    }

    pub fn sidebar_folded_groups_path(&self) -> PathBuf {
        paths::state_dir(&self.home).join("sidebar-folded-groups.json")
    }

    pub fn sidebar_ui_path(&self) -> PathBuf {
        paths::state_dir(&self.home).join("sidebar-ui.json")
    }

    /// Directory holding sidebar notes (`prefs.json` + `notes/*.json`).
    pub fn sidebar_notepad_dir(&self) -> PathBuf {
        paths::state_dir(&self.home).join("sidebar-notepad")
    }

    pub fn sidebar_notepad_prefs_path(&self) -> PathBuf {
        self.sidebar_notepad_dir().join("prefs.json")
    }

    pub fn sidebar_notepad_notes_dir(&self) -> PathBuf {
        self.sidebar_notepad_dir().join("notes")
    }

    pub fn sidebar_note_path(&self, note_id: &str) -> PathBuf {
        self.sidebar_notepad_notes_dir()
            .join(format!("{note_id}.json"))
    }

    pub fn session_manifest_path(&self) -> PathBuf {
        paths::state_dir(&self.home).join("session-manifest.json")
    }

    /// Last healthy set of live managed session ids — restore allowlist after down/crash.
    pub fn live_session_snapshot_path(&self) -> PathBuf {
        paths::state_dir(&self.home).join("live-session-snapshot.json")
    }

    pub fn new_session_defaults_path(&self) -> PathBuf {
        self.agent_state_dir().join("new-session-defaults.json")
    }

    pub fn new_session_draft_path(&self) -> PathBuf {
        self.agent_state_dir().join("new-session-draft.json")
    }

    /// Legacy path — migrated on first load if the new file is absent.
    pub fn legacy_new_chat_defaults_path(&self) -> PathBuf {
        self.agent_state_dir().join("new-chat-defaults.json")
    }

    pub fn automations_dir(&self) -> PathBuf {
        paths::state_dir(&self.home).join("automations")
    }

    pub fn automation_dir(&self, id: &str) -> PathBuf {
        self.automations_dir().join(id)
    }
}
