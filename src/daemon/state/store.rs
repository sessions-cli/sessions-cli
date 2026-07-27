// WS-08: extracted from daemon/state.rs

use crate::config::Config;
use crate::daemon::state::ClosedSessionMarker;
use crate::model::Session;
use crate::session::manifest::load_manifest;
use std::collections::{HashMap, HashSet};

pub(crate) fn closed_markers_from_manifest(config: &Config) -> HashSet<ClosedSessionMarker> {
    let Ok(manifest) = load_manifest(config) else {
        return HashSet::new();
    };
    manifest
        .entries
        .iter()
        .filter(|entry| entry.closed)
        .map(|entry| ClosedSessionMarker {
            sessions_session_id: Some(entry.sessions_session_id.clone()),
            agent_session_id: entry.agent_session_id.clone(),
            tmux_pane_id: String::new(),
        })
        .collect()
}

/// SSNs present in `previous` but missing from the post-merge live map.
///
/// Keyed by stable `sessions_session_id`, not `tmux:win:N`. A managed session
/// that reattached under a new window index must not be tombstoned — that was
/// the false-close path that permanently hid live windows after restore/reload.
pub(crate) fn sessions_session_ids_to_tombstone(
    previous: &HashMap<String, Session>,
    live: &HashMap<String, Session>,
) -> Vec<String> {
    let live_ssns: HashSet<String> = live
        .values()
        .filter_map(|session| session.sessions_session_id.clone())
        .collect();
    let mut vanished = Vec::new();
    let mut seen = HashSet::new();
    for session in previous.values() {
        let Some(ssn) = session.sessions_session_id.as_ref() else {
            continue;
        };
        if live_ssns.contains(ssn) {
            continue;
        }
        if seen.insert(ssn.clone()) {
            vanished.push(ssn.clone());
        }
    }
    vanished
}
pub(crate) fn sessions_changed(
    previous: &HashMap<String, Session>,
    current: &HashMap<String, Session>,
) -> bool {
    if previous.len() != current.len() {
        return true;
    }
    for (id, session) in current {
        let Some(old) = previous.get(id) else {
            return true;
        };
        if old.tab_index != session.tab_index
            || old.sessions_session_id != session.sessions_session_id
            || old.tmux_pane_id != session.tmux_pane_id
            || old.managed != session.managed
            || old.title != session.title
            || old.description != session.description
            || old.project != session.project
            || old.cwd != session.cwd
            || old.cwd_label != session.cwd_label
            || old.state != session.state
            || old.is_active != session.is_active
            || old.agent_session_id != session.agent_session_id
            || old.completed_thread != session.completed_thread
            || old.completed_at != session.completed_at
            || old.prompt_submitted != session.prompt_submitted
            || old.messaged_at != session.messaged_at
            || old.last_event_at != session.last_event_at
            || old.title_manual != session.title_manual
        {
            return true;
        }
    }
    false
}
