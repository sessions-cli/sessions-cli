// WS-08: extracted from daemon/state.rs

use crate::config::Config;
use crate::daemon::state::titles::path_leaf;
use crate::daemon::tmux::{pane_effective_cwd, pane_to_window_index};
use crate::model::{AgentState, NotifyMessage, Session};
use crate::pty::resolve_session_names;
use crate::session::{load_session_env, WorkspaceCatalog};
use chrono::Utc;
use std::path::Path;
use tracing::debug;

pub(crate) fn cwd_label_for_path(cwd: &str, home: &Path) -> String {
    if cwd.is_empty() {
        String::new()
    } else {
        crate::pty::format_tilde_path(cwd, home)
    }
}

pub(crate) fn is_home_cwd(cwd: &str, home: &Path) -> bool {
    let cwd = cwd.trim_end_matches('/');
    cwd.is_empty() || cwd == home.to_string_lossy().as_ref()
}

pub(crate) fn resolve_notify_cwd(
    msg_cwd: Option<&str>,
    pane_id: &str,
    tmux_session: &str,
    _home: &Path,
) -> String {
    if let Some(cwd) = msg_cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) {
        return cwd.to_string();
    }
    if !pane_id.is_empty() {
        if let Ok(cwd) = pane_effective_cwd(tmux_session, pane_id) {
            if !cwd.is_empty() {
                return cwd;
            }
        }
    }
    String::new()
}

pub(crate) fn update_session_cwd(entry: &mut Session, cwd: &str, home: &Path) {
    if cwd.is_empty() || entry.cwd == cwd {
        return;
    }

    let old_cwd_label = entry.cwd_label.clone();
    let old_description = entry.description.clone();
    let old_leaf = path_leaf(&entry.cwd, home);

    entry.cwd = cwd.to_string();
    entry.cwd_label = crate::pty::format_tilde_path(cwd, home);

    if !entry.title_manual && (entry.title.trim().is_empty() || entry.title == old_cwd_label) {
        let (title, description, project) = resolve_session_names(
            home,
            cwd,
            None,
            entry.agent_session_id.as_deref(),
            &entry.title,
            &entry.description,
            "",
            None::<crate::session::WorkspaceRef<'_>>,
            false,
        );
        entry.title = title;
        entry.description = description;
        entry.project = project;
    }

    if old_description.trim().is_empty()
        || old_description == old_leaf
        || old_description == old_cwd_label
    {
        entry.description = crate::pty::default_thread_name(cwd, home);
    }
}
pub(crate) fn initial_session_names(
    workspaces: &WorkspaceCatalog,
    window_index: u32,
    cwd: &str,
    home: &Path,
) -> (String, String, String) {
    let workspace = workspaces.workspace_ref_for_window(window_index, cwd);
    resolve_session_names(home, cwd, None, None, "", "", "", workspace, false)
}
pub(crate) fn hook_targets_session(
    entry: &Session,
    event: &str,
    msg: &NotifyMessage,
    pane_id: &str,
    home: &Path,
) -> bool {
    if !pane_id.is_empty() && !entry.tmux_pane_id.is_empty() && pane_id != entry.tmux_pane_id {
        return false;
    }

    let msg_sid = msg.session_id.as_deref();
    let entry_sid = entry.agent_session_id.as_deref();
    let pane_matches =
        pane_id.is_empty() || entry.tmux_pane_id.is_empty() || pane_id == entry.tmux_pane_id;

    if let Some(incoming) = msg_sid {
        if let Some(parent) = crate::agents::parent_session_id_for_subagent(home, incoming) {
            if entry_sid == Some(parent.as_str()) {
                return false;
            }
            if matches!(event, "session_start" | "prompt") {
                return false;
            }
        }
    }

    match (entry_sid, msg_sid) {
        (Some(existing), Some(incoming)) if existing != incoming => {
            let incoming_agent = msg
                .agent
                .as_deref()
                .map(|a| a.trim().to_ascii_lowercase())
                .filter(|a| !a.is_empty());
            let existing_agent = entry
                .managed_agent
                .as_deref()
                .map(|a| a.trim().to_ascii_lowercase())
                .filter(|a| !a.is_empty())
                .or_else(|| {
                    crate::pty::parse_app(&entry.title)
                        .filter(|app| crate::pty::is_agent_app(app))
                        .map(|app| app.to_ascii_lowercase())
                });
            // Detect a bound SID that belongs to a *different* agent than the
            // managed/title agent (e.g. Grok UUID left on an OpenCode window).
            // That poison must not block same-agent recovery via prompt/start.
            let existing_sid_cross_agent = entry_sid.is_some_and(|sid| {
                existing_agent.as_ref().is_some_and(|agent| {
                    !crate::agents::agent_session_matches_expected_agent(home, sid, agent)
                })
            });

            if matches!(event, "session_start" | "prompt") {
                // Don't let one agent's hooks rotate away a different agent's
                // already-bound session in the same pane. Without this guard, a
                // Grok session_start hook can clobber an active OpenCode, Codex,
                // or Claude session because the pane or window index matches.
                //
                // Prompt is included so a correct OpenCode sessionId can replace
                // a poisoned cross-agent binding (env/session_start race); same-
                // agent rebinds (new thread in the same pane) also need prompt.
                if let Some(existing) = &existing_agent {
                    if incoming_agent.as_deref() != Some(existing.as_str()) {
                        debug!(
                            "notify ignored for {event}: existing agent {} != incoming agent {:?}",
                            existing, incoming_agent
                        );
                        return false;
                    }
                }
                // Cross-agent poison on the row: allow the matching agent through
                // even though the bound SID string differs.
                if existing_sid_cross_agent {
                    return true;
                }
                if matches!(event, "session_start") {
                    return true;
                }
                // Same-agent prompt with a new SID (new thread): allow when the
                // managed agent matches, so recovery is not session_start-only.
                if incoming_agent.is_some()
                    && existing_agent.is_some()
                    && incoming_agent == existing_agent
                {
                    return true;
                }
                return false;
            }
            if matches!(
                event,
                "stop" | "turn_complete" | "pre_tool" | "post_tool" | "tool_fail"
            ) {
                return pane_matches;
            }
            false
        }
        (None, Some(_))
            if matches!(
                event,
                "stop" | "turn_complete" | "pre_tool" | "post_tool" | "tool_fail"
            ) =>
        {
            !is_console_session(entry) && pane_matches
        }
        _ => true,
    }
}

pub(crate) fn is_console_session(session: &Session) -> bool {
    if session.agent_session_id.is_some() {
        return false;
    }
    if session
        .managed_agent
        .as_deref()
        .is_some_and(crate::pty::is_agent_app)
    {
        return false;
    }
    if crate::pty::parse_app(&session.title).is_some_and(|app| crate::pty::is_agent_app(&app)) {
        return false;
    }
    crate::pty::is_console_session(&session.description, &session.title)
}
pub(crate) fn apply_thread_hook(
    entry: &mut Session,
    event: &str,
    incoming: AgentState,
) -> Option<AgentState> {
    apply_thread_hook_with_prompt(entry, event, incoming, None)
}

pub(crate) fn apply_thread_hook_with_prompt(
    entry: &mut Session,
    event: &str,
    incoming: AgentState,
    prompt: Option<&str>,
) -> Option<AgentState> {
    match event {
        "session_start" => {
            // A prompt-less session_start on an existing session is almost always a
            // restore/resume artifact (e.g. Grok's SessionStart firing after
            // `grok --resume`, OpenCode's shell.env on resume). Don't clobber the
            // stored message/completion time or reorder the row; wait for an actual
            // user prompt to start a new turn.
            let prompt_empty = prompt.is_none_or(|p| p.trim().is_empty());
            if prompt_empty {
                if entry.thread_is_complete() || entry.completion_acknowledged() {
                    return None;
                }
                if entry.messaged_at.is_some() {
                    // Existing session: keep its sidebar-order timestamp.
                    return None;
                }
                // Brand-new session with no prior prompt stamp: treat this as
                // the creation time for ordering purposes.
            }
            entry.completed_thread = None;
            entry.completed_at = None;
            entry.messaged_at = Some(Utc::now());
            entry.prompt_submitted = false;
            entry.state = incoming;
            entry.last_event_at = Utc::now();
            Some(incoming)
        }
        "prompt" => {
            entry.completed_thread = None;
            entry.completed_at = None;
            entry.messaged_at = Some(Utc::now());
            entry.prompt_submitted = true;
            entry.state = incoming;
            entry.last_event_at = Utc::now();
            Some(incoming)
        }
        "stop" | "turn_complete" => {
            let thread = entry.description.clone();
            if (entry.thread_is_complete() || entry.completion_acknowledged())
                && entry.completed_thread.as_deref() == Some(thread.as_str())
            {
                return None;
            }
            entry.completed_thread = Some(thread);
            entry.completed_at = Some(Utc::now());
            entry.state = AgentState::Done;
            Some(AgentState::Done)
        }
        "pre_tool" | "post_tool" | "tool_fail" | "approval_required" => {
            if entry.thread_is_complete() && event != "approval_required" {
                return None;
            }
            entry.completed_thread = None;
            entry.completed_at = None;
            entry.state = incoming;
            entry.last_event_at = Utc::now();
            Some(incoming)
        }
        _ => {
            entry.state = incoming;
            Some(incoming)
        }
    }
}

pub(crate) fn resolve_window_index(msg: &NotifyMessage, config: &Config) -> Option<u32> {
    let tmux_session = msg
        .tmux_session
        .as_deref()
        .unwrap_or(&config.tmux_session)
        .to_string();

    // Stable managed id wins over stale per-agent env files that still point at an old pane/window.
    if let Some(ref ssn_id) = msg.sessions_session_id {
        if let Some(record) = crate::session::load_managed_record(&config.home, ssn_id) {
            if record.tmux_session == tmux_session {
                return Some(record.window_index);
            }
        }
    }

    if let Some(ref pane) = msg.tmux_pane_id {
        if let Some(idx) = pane_to_window_index(&tmux_session, pane) {
            return Some(idx);
        }
        if let Some(ref sid) = msg.session_id {
            let env = load_session_env(&config.session_env_path(sid));
            let session = env.tmux_session.as_deref().unwrap_or(&tmux_session);
            if let Some(idx) = pane_to_window_index(session, pane) {
                return Some(idx);
            }
        }
    }

    if let Some(ref sid) = msg.session_id {
        let env = load_session_env(&config.session_env_path(sid));
        if let Some(idx) = env.window_index {
            let session = env.tmux_session.as_deref().unwrap_or(tmux_session.as_str());
            if session == tmux_session.as_str() {
                return Some(idx);
            }
        }
    }

    if let Some(idx) = msg
        .kitty_window_id
        .and_then(|v| u32::try_from(v).ok())
        .filter(|&v| v > 0)
    {
        return Some(idx);
    }

    debug!("notify without resolvable window: {:?}", msg.event);
    None
}
