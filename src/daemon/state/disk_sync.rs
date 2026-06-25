// WS-08: extracted from daemon/state.rs

use crate::config::Config;
use crate::daemon::state::merge::agent_state_rank;
use crate::session::manifest::ManifestEntry;
use crate::session::WorkspaceCatalog;
use crate::model::{AgentState, Session};
use chrono::Utc;

pub(crate) fn sync_messaged_at_from_agent_disk(config: &Config, session: &mut Session) {
    let Some(sid) = session.agent_session_id.as_deref() else {
        return;
    };
    let lookup_cwd = crate::agents::disk_lookup_cwd(&config.home, &session.cwd, Some(sid));

    // Sidebar order is hook-owned after the first prompt. Disk activity includes tool
    // hooks — never bump messaged_at from polls or running sessions swap places.
    if session.messaged_at.is_none() {
        if let Some(at) = crate::agents::session_messaged_at(&config.home, &lookup_cwd, sid) {
            session.messaged_at = Some(at);
        }
    }

    if let Some(at) = crate::agents::session_activity_at(&config.home, &lookup_cwd, sid) {
        if session.last_event_at < at {
            session.last_event_at = at;
        }
    }
}
pub(crate) fn sync_live_activity_from_disk(config: &Config, session: &mut Session) {
    if session.thread_is_complete() || session.state == AgentState::Done {
        return;
    }
    let Some(sid) = session.agent_session_id.as_deref() else {
        return;
    };
    let lookup_cwd = crate::agents::disk_lookup_cwd(&config.home, &session.cwd, Some(sid));
    let Some(agent) = crate::agents::infer_agent_for_session(&config.home, &lookup_cwd, sid) else {
        return;
    };
    let Some(activity) = agent.live_activity(&config.home, &lookup_cwd, sid) else {
        return;
    };
    // After the user has opened a completed thread, ignore stale in-flight disk
    // markers from the finished turn — wait for hooks or newer disk activity.
    if session.completion_acknowledged()
        && session
            .completed_at
            .is_some_and(|completed_at| activity.at <= completed_at)
    {
        return;
    }
    let hook_rank = agent_state_rank(session.state);
    let disk_rank = agent_state_rank(activity.state);
    if session.state == AgentState::Idle || disk_rank > hook_rank {
        session.state = activity.state;
        session.completed_thread = None;
        session.completed_at = None;
    }
    if activity.at > session.last_event_at {
        session.last_event_at = activity.at;
    }
}

pub(crate) fn sync_turn_completion_from_disk(config: &Config, session: &mut Session) -> bool {
    if session.thread_is_complete() || !session.is_in_progress() {
        return false;
    }
    let Some(sid) = session.agent_session_id.as_deref() else {
        return false;
    };
    let lookup_cwd = crate::agents::disk_lookup_cwd(&config.home, &session.cwd, Some(sid));
    let agent = match crate::agents::infer_agent_for_session(&config.home, &lookup_cwd, sid) {
        Some(agent) => agent,
        None => return false,
    };
    let Some(boundary) = agent.turn_boundary(&config.home, &lookup_cwd, sid) else {
        return false;
    };
    if !crate::agents::turn_is_complete(&boundary) {
        return false;
    }
    let completed_at = boundary.last_completed.unwrap_or_else(Utc::now);
    session.completed_thread = Some(session.description.clone());
    session.completed_at = Some(completed_at);
    session.state = AgentState::Done;
    true
}
pub(crate) fn workspace_default_thread_for_session(session: &Session, config: &Config) -> Option<String> {
    let workspaces = WorkspaceCatalog::load(&config.workspaces_path);
    let workspace = workspaces.workspace_ref_for_window(session.tab_index, &session.cwd)?;
    if !workspace.title.contains(" · ") {
        return None;
    }
    let thread = crate::pty::parse_description(workspace.title);
    if thread.is_empty() || thread == "session" {
        return None;
    }
    Some(thread)
}

pub(crate) fn session_has_workspace_default_identity(session: &Session, config: &Config) -> bool {
    let Some(default_thread) = workspace_default_thread_for_session(session, config) else {
        return false;
    };
    session.description.trim() == default_thread.trim()
}

pub(crate) fn stamp_new_session_order(session: &mut Session) {
    if session.messaged_at.is_none() {
        session.messaged_at = Some(Utc::now());
    }
}

/// Resolve sidebar `messaged_at` after poll/restore merge.
///
/// Manifest hook timestamps are authoritative for restored managed sessions.
/// Agent disk is the fallback. `stamp_new_session_order` applies only to
/// unmanaged rows or managed rows with no manifest entry (true new launch).
pub(crate) fn apply_refresh_messaged_at(
    config: &Config,
    session: &mut Session,
    manifest_entry: Option<&ManifestEntry>,
) {
    if let Some(entry) = manifest_entry {
        if let Some(at) = entry.messaged_at {
            session.messaged_at = Some(at);
            return;
        }
        if session.messaged_at.is_none() {
            sync_messaged_at_from_agent_disk(config, session);
        }
        return;
    }
    if session.messaged_at.is_none() {
        sync_messaged_at_from_agent_disk(config, session);
    }
    if session.messaged_at.is_none() {
        stamp_new_session_order(session);
    }
}

pub(crate) fn hydrate_messaged_at(session: &mut Session, config: &Config) {
    if session.messaged_at.is_some() {
        return;
    }
    if session.completed_at.is_some() || session.completed_thread.is_some() {
        session.messaged_at = Some(session.last_event_at);
        return;
    }
    if session.is_in_progress() {
        session.messaged_at = Some(session.last_event_at);
        return;
    }
    // Legacy `prompt_submitted` was inferred from workspace titles — ignore those.
    if session.prompt_submitted && !session_has_workspace_default_identity(session, config) {
        session.messaged_at = Some(session.last_event_at);
    }
}
