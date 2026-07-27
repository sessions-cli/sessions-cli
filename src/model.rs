use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Working,
    Approval,
    Error,
    Done,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Approval => "approval",
            Self::Error => "error",
            Self::Done => "done",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "working" => Self::Working,
            "approval" => Self::Approval,
            "error" => Self::Error,
            "done" => Self::Done,
            _ => Self::Idle,
        }
    }

    pub fn rings_bell(self) -> bool {
        matches!(self, Self::Done)
    }

    pub fn completes_thread(self) -> bool {
        matches!(self, Self::Done | Self::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// Legacy wire field; in tmux mode holds the tmux window index.
    pub kitty_window_id: u64,
    pub kitty_tab_id: u64,
    pub kitty_os_window_id: u64,
    /// Tmux window index (1-based); primary focus target.
    pub tab_index: u32,
    #[serde(default)]
    pub tmux_session: String,
    #[serde(default)]
    pub tmux_pane_id: String,
    /// Foreground pane PID from the latest tmux poll — detects stale agent session bindings.
    #[serde(default)]
    pub pane_pid: u32,
    #[serde(alias = "grok_session_id", skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    pub title: String,
    pub description: String,
    pub cwd: String,
    pub cwd_label: String,
    pub project: String,
    pub state: AgentState,
    /// Thread description that received a terminal `stop` hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_thread: Option<String>,
    /// When the most recent thread completed — sidebar time badge prefers this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// When the user last submitted a prompt — sidebar time uses this before completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaged_at: Option<DateTime<Utc>>,
    /// Legacy wire field; only set by prompt/session_start hooks for patch clearing.
    #[serde(default)]
    pub prompt_submitted: bool,
    /// User renamed this session from the sidebar — auto-naming must not overwrite it.
    #[serde(default)]
    pub title_manual: bool,
    pub is_active: bool,
    pub last_event_at: DateTime<Utc>,
    /// Sessions-created window with authoritative local identity.
    #[serde(default)]
    pub managed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_agent: Option<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            id: String::new(),
            kitty_window_id: 0,
            kitty_tab_id: 0,
            kitty_os_window_id: 0,
            tab_index: 0,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: String::new(),
            description: String::new(),
            cwd: String::new(),
            cwd_label: String::new(),
            project: String::new(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            managed: false,
            sessions_session_id: None,
            managed_agent: None,
        }
    }
}

impl Session {
    pub fn session_id_from_window(window_index: u32) -> String {
        format!("tmux:win:{window_index}")
    }

    /// Unacknowledged `turn_complete` — green highlight and bell both use this.
    pub fn thread_is_complete(&self) -> bool {
        self.state == AgentState::Done && self.completed_thread.is_some()
    }

    pub fn display_state(&self) -> AgentState {
        if self.thread_is_complete() {
            AgentState::Done
        } else if self.state == AgentState::Done {
            AgentState::Idle
        } else {
            self.state
        }
    }

    /// Clears the visible "done" highlight after the user visits the session.
    pub fn acknowledge_if_done(&mut self) -> bool {
        if !self.thread_is_complete() {
            return false;
        }
        self.state = AgentState::Idle;
        true
    }

    /// Actively running agents stay pinned; stale `working` pane files do not.
    pub fn pins_to_group_top(&self) -> bool {
        self.shows_run_spinner()
    }

    /// Agent turn still in flight — working, awaiting approval, or recovering from a tool error.
    pub fn is_in_progress(&self) -> bool {
        matches!(
            self.state,
            AgentState::Working | AgentState::Approval | AgentState::Error
        )
    }

    /// Sidebar snake spinner — any in-progress turn with recent hook activity.
    pub fn shows_run_spinner(&self) -> bool {
        self.is_in_progress() && self.is_actively_running()
    }

    /// A turn is active when hooks fired recently; stale `working` pane files age out.
    pub fn is_actively_running(&self) -> bool {
        let age = Utc::now().signed_duration_since(self.last_event_at);
        age <= chrono::Duration::minutes(10)
    }

    /// Sidebar ordering inside a directory: most recently messaged thread first.
    /// Only `messaged_at` (`prompt` / `session_start`) ranks rows — not completion,
    /// acknowledgment, run state, or tool-hook `last_event_at`.
    pub fn cmp_within_group(&self, other: &Self) -> std::cmp::Ordering {
        cmp_within_group_by_time(
            self.messaged_at,
            other.messaged_at,
            other.tab_index.cmp(&self.tab_index),
        )
    }

    /// Directory groups by newest user message — saved group order handles position.
    pub fn cmp_groups(a: &[Session], b: &[Session]) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let a_newest = a.iter().filter_map(|session| session.messaged_at).max();
        let b_newest = b.iter().filter_map(|session| session.messaged_at).max();
        match (a_newest, b_newest) {
            (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    /// Completed thread the user has opened — keep subtle tint, show age not square.
    pub fn completion_acknowledged(&self) -> bool {
        self.state == AgentState::Idle && self.completed_thread.is_some()
    }

    /// Sidebar time badge — completion age, else age since last user prompt.
    pub fn time_badge_at(&self) -> Option<DateTime<Utc>> {
        self.completed_at.or(self.messaged_at)
    }

    /// Sidebar row tint — completion is shown via trailing green square only.
    pub fn sidebar_state(&self) -> AgentState {
        if self.thread_is_complete() {
            AgentState::Done
        } else {
            AgentState::Idle
        }
    }
}

fn cmp_within_group_by_time(
    self_at: Option<DateTime<Utc>>,
    other_at: Option<DateTime<Utc>>,
    tie: std::cmp::Ordering,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (self_at, other_at) {
        (Some(a), Some(b)) => b.cmp(&a).then(tie),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => tie,
    }
}

/// Wire-protocol message type for hook → daemon notify payloads.
pub const NOTIFY_MESSAGE_TYPE: &str = "notify";
/// Legacy type string still accepted on deserialize.
pub const NOTIFY_MESSAGE_TYPE_LEGACY: &str = "grok";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotifyMessage {
    pub t: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kitty_window_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmux_pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
    pub event: String,
    pub ts: i64,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kitty_pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kitty_listen_on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions_session_id: Option<String>,
}

impl NotifyMessage {
    /// True for hook notify payloads (`notify` or legacy `grok` type tag).
    pub fn is_sessions_notify(&self) -> bool {
        self.t == NOTIFY_MESSAGE_TYPE || self.t == NOTIFY_MESSAGE_TYPE_LEGACY
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ClientCommand {
    Subscribe,
    Focus {
        /// Ordered session number from the sidebar / key bindings.
        #[serde(alias = "kitty_window_id")]
        window_index: u32,
        /// Direct tmux window index when the client knows the exact target.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab_index: Option<u32>,
    },
    List,
    Status {
        #[serde(default)]
        verbose: bool,
    },
    Refresh,
    Rename {
        session_id: String,
        title: String,
    },
    CloseSession {
        session_id: String,
    },
    AcknowledgeCompletion {
        session_id: String,
    },
    TelemetryFlush,
    RestoreComplete,
    /// Enter booting before agents tmux is torn down (`sessions down`).
    PrepareRestore,
    FlushManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Snapshot {
        sessions: Vec<Session>,
        version: u64,
    },
    Patch {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<AgentState>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd_label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_active: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_event_at: Option<DateTime<Utc>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_thread: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<DateTime<Utc>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        messaged_at: Option<DateTime<Utc>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_submitted: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title_manual: Option<bool>,
        /// Sidebar should force-repaint for a newly rung alert. The daemon plays
        /// the system sound on notify; this flag is not required for audio.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ring_bell: Option<bool>,
        version: u64,
    },
    Status {
        healthy: bool,
        session_count: usize,
        version: u64,
        last_poll_at: Option<DateTime<Utc>>,
        #[serde(default)]
        booting: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metrics: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        update: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    pub sessions: Vec<Session>,
    pub version: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_badge_hidden_until_first_prompt() {
        let mut session = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "grok".into(),
            description: String::new(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "grok".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: true,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        assert!(session.time_badge_at().is_none());
        session.messaged_at = Some(Utc::now());
        assert!(session.time_badge_at().is_some());
        session.messaged_at = None;
        session.completed_at = Some(Utc::now());
        assert!(session.time_badge_at().is_some());
    }

    #[test]
    fn agent_state_roundtrip() {
        assert_eq!(AgentState::from_str("working"), AgentState::Working);
        assert_eq!(AgentState::Done.as_str(), "done");
    }

    #[test]
    fn display_state_hides_stale_done_without_completion_marker() {
        let session = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · ship api".into(),
            description: "ship api".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Done,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        assert!(!session.thread_is_complete());
        assert_eq!(session.display_state(), AgentState::Idle);
    }

    #[test]
    fn thread_is_complete_survives_description_refresh() {
        let session = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · longer refreshed title".into(),
            description: "longer refreshed title".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Done,
            completed_thread: Some("short title".into()),
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        assert!(session.thread_is_complete());
        assert_eq!(session.sidebar_state(), AgentState::Done);
    }

    #[test]
    fn cmp_within_group_orders_by_messaged_at_regardless_of_acknowledgment() {
        let active = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · fresh".into(),
            description: "fresh".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Working,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now() - chrono::Duration::hours(2)),
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        let mut acknowledged = Session {
            state: AgentState::Done,
            completed_thread: Some("ship api".into()),
            completed_at: Some(Utc::now() - chrono::Duration::minutes(36)),
            messaged_at: Some(Utc::now() - chrono::Duration::minutes(2)),
            tab_index: 2,
            id: "tmux:win:2".into(),
            kitty_window_id: 2,
            kitty_tab_id: 2,
            ..active.clone()
        };
        assert!(acknowledged.acknowledge_if_done());
        assert!(acknowledged.completion_acknowledged());
        assert_eq!(
            acknowledged.cmp_within_group(&active),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn cmp_within_group_orders_newest_message_first() {
        let older = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · old".into(),
            description: "old".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Done,
            completed_thread: Some("old".into()),
            completed_at: None,
            messaged_at: Some(Utc::now() - chrono::Duration::minutes(30)),
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now() - chrono::Duration::minutes(30),
            ..Default::default()
        };
        let newer = Session {
            messaged_at: Some(Utc::now() - chrono::Duration::minutes(5)),
            description: "new".into(),
            completed_thread: Some("new".into()),
            tab_index: 2,
            ..older.clone()
        };
        assert_eq!(newer.cmp_within_group(&older), std::cmp::Ordering::Less);
    }

    #[test]
    fn completed_threads_do_not_pin_to_group_top() {
        let completed = Session {
            id: "tmux:win:3".into(),
            kitty_window_id: 3,
            kitty_tab_id: 3,
            kitty_os_window_id: 1,
            tab_index: 3,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · ship api".into(),
            description: "ship api".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Done,
            completed_thread: Some("ship api".into()),
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        assert!(!completed.pins_to_group_top());
        assert!(!completed.shows_run_spinner());
    }

    #[test]
    fn stale_working_sessions_do_not_pin_to_top() {
        let stale = Session {
            id: "tmux:win:8".into(),
            kitty_window_id: 8,
            kitty_tab_id: 8,
            kitty_os_window_id: 1,
            tab_index: 8,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · misc".into(),
            description: "misc workspace".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Working,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now() - chrono::Duration::minutes(27),
            ..Default::default()
        };
        assert!(!stale.pins_to_group_top());
        assert!(!stale.shows_run_spinner());
    }

    #[test]
    fn cmp_within_group_orders_by_message_time_not_run_state() {
        let older_message = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · fresh".into(),
            description: "fresh".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now() - chrono::Duration::minutes(1)),
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        let running = Session {
            state: AgentState::Working,
            messaged_at: Some(Utc::now() - chrono::Duration::minutes(30)),
            last_event_at: Utc::now() - chrono::Duration::minutes(2),
            description: "active runner".into(),
            tab_index: 2,
            id: "tmux:win:2".into(),
            kitty_window_id: 2,
            kitty_tab_id: 2,
            ..older_message.clone()
        };
        assert!(running.pins_to_group_top());
        assert!(running.shows_run_spinner());
        assert_eq!(
            older_message.cmp_within_group(&running),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn cmp_within_group_sorts_approval_by_message_time_not_pin() {
        let idle = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · fresh".into(),
            description: "fresh".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now() - chrono::Duration::minutes(30)),
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now() - chrono::Duration::minutes(30),
            ..Default::default()
        };
        let approval = Session {
            state: AgentState::Approval,
            messaged_at: Some(Utc::now()),
            last_event_at: Utc::now(),
            tab_index: 2,
            id: "tmux:win:2".into(),
            kitty_window_id: 2,
            kitty_tab_id: 2,
            ..idle.clone()
        };
        assert!(approval.pins_to_group_top());
        assert!(approval.shows_run_spinner());
        assert_eq!(approval.cmp_within_group(&idle), std::cmp::Ordering::Less);
    }

    #[test]
    fn cmp_within_group_ignores_tool_hook_activity() {
        let recently_messaged = Session {
            id: "tmux:win:2".into(),
            kitty_window_id: 2,
            kitty_tab_id: 2,
            kitty_os_window_id: 1,
            tab_index: 2,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · newer".into(),
            description: "newer".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Working,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now() - chrono::Duration::minutes(1)),
            prompt_submitted: true,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now() - chrono::Duration::minutes(1),
            ..Default::default()
        };
        let older_message = Session {
            tab_index: 5,
            id: "tmux:win:5".into(),
            kitty_window_id: 5,
            kitty_tab_id: 5,
            state: AgentState::Working,
            messaged_at: Some(Utc::now() - chrono::Duration::minutes(30)),
            description: "older".into(),
            title: "app · older".into(),
            // Tool hooks keep bumping this ahead of the newer prompt.
            last_event_at: Utc::now(),
            ..recently_messaged.clone()
        };
        assert_eq!(
            recently_messaged.cmp_within_group(&older_message),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn cmp_within_group_ignores_last_event_at_without_message() {
        let recent_tab = Session {
            id: "tmux:win:2".into(),
            kitty_window_id: 2,
            kitty_tab_id: 2,
            kitty_os_window_id: 1,
            tab_index: 2,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · newer".into(),
            description: "newer".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Working,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        let older_tab = Session {
            tab_index: 5,
            id: "tmux:win:5".into(),
            kitty_window_id: 5,
            kitty_tab_id: 5,
            last_event_at: Utc::now() - chrono::Duration::minutes(30),
            description: "older".into(),
            title: "app · older".into(),
            ..recent_tab.clone()
        };
        assert_eq!(
            older_tab.cmp_within_group(&recent_tab),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn sidebar_state_only_colors_completed_threads() {
        let working = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · ship api".into(),
            description: "ship api".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Working,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        assert_eq!(working.sidebar_state(), AgentState::Idle);

        let done = Session {
            state: AgentState::Done,
            completed_thread: Some("ship api".into()),
            ..working.clone()
        };
        assert_eq!(done.sidebar_state(), AgentState::Done);

        let mut acknowledged = done.clone();
        assert!(acknowledged.acknowledge_if_done());
        assert_eq!(acknowledged.sidebar_state(), AgentState::Idle);
        assert!(acknowledged.completion_acknowledged());
    }

    #[test]
    fn display_state_shows_done_for_completed_thread() {
        let session = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · ship api".into(),
            description: "ship api".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Done,
            completed_thread: Some("ship api".into()),
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        assert!(session.thread_is_complete());
        assert_eq!(session.display_state(), AgentState::Done);
    }

    #[test]
    fn acknowledge_if_done_clears_sidebar_green_highlight() {
        let mut session = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · ship api".into(),
            description: "ship api".into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Done,
            completed_thread: Some("ship api".into()),
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        };
        assert!(session.thread_is_complete());
        assert!(session.acknowledge_if_done());
        assert_eq!(session.display_state(), AgentState::Idle);
        assert_eq!(session.completed_thread, Some("ship api".into()));
        assert!(!session.thread_is_complete());
        assert!(session.completion_acknowledged());
        assert_eq!(session.sidebar_state(), AgentState::Idle);
        assert!(!session.acknowledge_if_done());
    }

    #[test]
    fn completes_thread_only_on_terminal_states() {
        assert!(AgentState::Done.completes_thread());
        assert!(AgentState::Error.completes_thread());
        assert!(!AgentState::Working.completes_thread());
        assert!(!AgentState::Approval.completes_thread());
        assert!(!AgentState::Idle.completes_thread());
    }

    #[test]
    fn rings_bell_only_when_thread_finishes() {
        assert!(AgentState::Done.rings_bell());
        assert!(!AgentState::Approval.rings_bell());
        assert!(!AgentState::Working.rings_bell());
        assert!(!AgentState::Error.rings_bell());
    }

    #[test]
    fn notify_message_serde() {
        let msg = NotifyMessage {
            t: NOTIFY_MESSAGE_TYPE.into(),
            agent: Some("codex".into()),
            session_id: Some("abc".into()),
            kitty_window_id: None,
            tmux_pane_id: Some("%1".into()),
            tmux_session: Some("agents".into()),
            event: "stop".into(),
            ts: 1717689600,
            payload: serde_json::json!({}),
            cwd: None,
            kitty_pid: None,
            kitty_listen_on: None,
            ..Default::default()
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: NotifyMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event, "stop");
        assert_eq!(back.t, NOTIFY_MESSAGE_TYPE);
        assert!(back.is_sessions_notify());
    }

    #[test]
    fn notify_message_accepts_legacy_grok_type() {
        let json = r#"{"t":"grok","event":"stop","ts":1,"payload":{}}"#;
        let msg: NotifyMessage = serde_json::from_str(json).unwrap();
        assert!(msg.is_sessions_notify());
    }

    #[test]
    fn client_command_focus_alias() {
        let cmd: ClientCommand =
            serde_json::from_str(r#"{"cmd":"focus","kitty_window_id":3}"#).unwrap();
        assert!(matches!(
            cmd,
            ClientCommand::Focus {
                window_index: 3,
                tab_index: None,
            }
        ));
    }

    #[test]
    fn client_command_focus_accepts_tab_index() {
        let cmd: ClientCommand =
            serde_json::from_str(r#"{"cmd":"focus","window_index":2,"tab_index":7}"#).unwrap();
        assert!(matches!(
            cmd,
            ClientCommand::Focus {
                window_index: 2,
                tab_index: Some(7),
            }
        ));
    }
}
