use crate::model::{NotifyMessage, NOTIFY_MESSAGE_TYPE};
use serde_json::Value;

pub fn build_notify_message(
    event: &str,
    agent: Option<&str>,
    agent_session_id: Option<&str>,
    kitty_window_id: Option<u64>,
    tmux_pane_id: Option<&str>,
    tmux_session: Option<&str>,
    payload: Value,
    cwd: Option<&str>,
    kitty_pid: Option<&str>,
    kitty_listen_on: Option<&str>,
    sessions_session_id: Option<&str>,
) -> NotifyMessage {
    NotifyMessage {
        t: NOTIFY_MESSAGE_TYPE.into(),
        agent: agent.map(String::from),
        session_id: agent_session_id.map(String::from),
        kitty_window_id,
        tmux_pane_id: tmux_pane_id.map(String::from),
        tmux_session: tmux_session.map(String::from),
        event: event.to_string(),
        ts: chrono::Utc::now().timestamp(),
        payload,
        cwd: cwd.map(String::from),
        kitty_pid: kitty_pid.map(String::from),
        kitty_listen_on: kitty_listen_on.map(String::from),
        sessions_session_id: sessions_session_id.map(String::from),
    }
}
