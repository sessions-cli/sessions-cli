// WS-08: extracted from daemon/state.rs

use crate::config::Config;
use crate::daemon::tmux::pane_to_window_index;
use crate::daemon::state::titles::{
    agent_session_title_is_placeholder, apply_manual_title_identity,
    ensure_agent_session_title, load_auto_persisted_title_identity,
    reset_unconfirmed_agent_title, session_has_manual_title,
};
use crate::session::load_session_env;
use crate::model::{AgentState, Session};
use std::collections::{HashMap, HashSet};

pub(crate) fn pane_index_from_sessions(sessions: &[Session]) -> HashMap<String, u32> {
    sessions
        .iter()
        .filter(|session| !session.tmux_pane_id.is_empty())
        .map(|session| (session.tmux_pane_id.clone(), session.tab_index))
        .collect()
}

pub(crate) fn resolve_agent_session_id(
    config: &Config,
    existing: &Session,
    fresh: &Session,
    pane_index: &HashMap<String, u32>,
) -> Option<String> {
    let pane_pid = fresh.pane_pid;
    let existing_valid = existing
        .agent_session_id
        .clone()
        .filter(|_| session_owns_agent_session(config, existing, pane_index, pane_pid));
    let fresh_valid = fresh
        .agent_session_id
        .clone()
        .filter(|_| session_owns_agent_session(config, fresh, pane_index, pane_pid));
    match (&existing_valid, &fresh_valid) {
        (Some(existing_id), Some(fresh_id)) if existing_id != fresh_id => fresh_valid,
        _ => existing_valid.or(fresh_valid),
    }
}

pub(crate) fn session_owns_pane(session: &Session, pane_index: &HashMap<String, u32>) -> bool {
    if session.tmux_pane_id.is_empty() {
        return true;
    }
    if let Some(&index) = pane_index.get(&session.tmux_pane_id) {
        return index == session.tab_index;
    }
    pane_to_window_index(&session.tmux_session, &session.tmux_pane_id)
        .is_none_or(|index| index == session.tab_index)
}

pub(crate) fn session_owns_agent_session(
    config: &Config,
    session: &Session,
    pane_index: &HashMap<String, u32>,
    current_pane_pid: u32,
) -> bool {
    let Some(sid) = session.agent_session_id.as_deref() else {
        return session_owns_pane(session, pane_index);
    };
    let pane_pid = if current_pane_pid != 0 {
        current_pane_pid
    } else {
        session.pane_pid
    };
    if current_pane_pid != 0 && session.pane_pid != 0 && current_pane_pid != session.pane_pid {
        return false;
    }
    if pane_pid != 0
        && !crate::session::env::session_env_is_live_for_pane(&config.home, sid, pane_pid)
    {
        return false;
    }
    if matches!(session.state, AgentState::Idle | AgentState::Done)
        && !crate::agents::agent_session_matches_pane_cwd(&config.home, &session.cwd, sid)
    {
        return false;
    }
    let env = load_session_env(&config.session_env_path(sid));
    if let Some(env_pane) = env.tmux_pane_id.as_deref() {
        if env_pane != session.tmux_pane_id.as_str() {
            return false;
        }
        if let Some(env_window) = env.window_index {
            if env_window != session.tab_index {
                return false;
            }
        }
    }
    session_owns_pane(session, pane_index)
}

pub(crate) fn pick_agent_session_owner(
    config: &Config,
    a: &Session,
    b: &Session,
    pane_index: &HashMap<String, u32>,
) -> bool {
    if a.is_active != b.is_active {
        return a.is_active;
    }
    let pane_pid = a.pane_pid.max(b.pane_pid);
    let a_owns = session_owns_agent_session(config, a, pane_index, pane_pid);
    let b_owns = session_owns_agent_session(config, b, pane_index, pane_pid);
    if a_owns != b_owns {
        return a_owns;
    }
    a.tab_index <= b.tab_index
}

/// Compute which sessions should have their agent binding hidden from the
/// snapshot: a session that no longer owns its agent session (stale pane
/// binding) or that lost a same-session-id collision to another window.
///
/// This performs disk I/O (env reads, cwd/pane ownership checks) and must run
/// off the render path — the result is cached and applied by `sorted_sessions`.
pub(crate) fn managed_session_skips_suppression(session: &Session) -> bool {
    session.managed && session.sessions_session_id.is_some()
}

pub(crate) fn build_previous_ssn_index(previous: &HashMap<String, Session>) -> HashMap<String, String> {
    let mut by_ssn = HashMap::new();
    for (id, session) in previous {
        if let Some(ref ssn) = session.sessions_session_id {
            by_ssn
                .entry(ssn.clone())
                .or_insert_with(|| id.clone());
        }
    }
    by_ssn
}

/// Whether a prior snapshot row at the same `tmux:win:N` id still describes this poll row.
///
/// After cold-boot restore, window indices are reused for different `@sessions.id`
/// values. A direct index match must not win over the live tmux option or stable-id
/// merge — otherwise the sidebar keeps stale `tab_index` mappings and focus lands on
/// the wrong pane.
pub(crate) fn direct_refresh_match(prior: &Session, fresh: &Session) -> bool {
    match (
        prior.sessions_session_id.as_deref(),
        fresh.sessions_session_id.as_deref(),
    ) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
    }
}

pub(crate) fn refresh_merge_prior<'a>(
    previous: &'a HashMap<String, Session>,
    fresh: &Session,
    by_ssn: &HashMap<String, String>,
) -> (Option<&'a Session>, bool) {
    let stable = find_previous_session(previous, fresh, by_ssn);
    let direct = previous
        .get(&fresh.id)
        .filter(|prior| direct_refresh_match(prior, fresh));
    let stable_only = stable.is_some() && direct.is_none();
    (direct.or(stable), stable_only)
}

pub(crate) fn find_previous_session<'a>(
    previous: &'a HashMap<String, Session>,
    fresh: &Session,
    by_ssn: &HashMap<String, String>,
) -> Option<&'a Session> {
    if let Some(ref ssn) = fresh.sessions_session_id {
        if let Some(old_id) = by_ssn.get(ssn) {
            if let Some(session) = previous.get(old_id.as_str()) {
                return Some(session);
            }
        }
        if let Some(session) = previous.values().find(|session| {
            session.sessions_session_id.as_deref() == Some(ssn.as_str())
        }) {
            return Some(session);
        }
    }
    if let Some(ref sid) = fresh.agent_session_id {
        let mut matches: Vec<&Session> = previous
            .values()
            .filter(|session| session.agent_session_id.as_deref() == Some(sid.as_str()))
            .collect();
        match matches.len() {
            0 => {}
            1 => return Some(matches[0]),
            _ => {
                matches.sort_by(|left, right| {
                    right
                        .is_active
                        .cmp(&left.is_active)
                        .then(left.tab_index.cmp(&right.tab_index))
                });
                return Some(matches[0]);
            }
        }
    }
    None
}

pub(crate) fn copy_stable_refresh_identity(prior: &Session, fresh: &mut Session) {
    fresh.messaged_at = fresh.messaged_at.or(prior.messaged_at);
    if prior.title_manual {
        fresh.title_manual = true;
        fresh.title = prior.title.clone();
        fresh.description = prior.description.clone();
        fresh.project = prior.project.clone();
    }
    if prior.thread_is_complete() {
        fresh.completed_thread = prior.completed_thread.clone().or(fresh.completed_thread.clone());
        fresh.completed_at = fresh.completed_at.or(prior.completed_at);
    }
}

pub(crate) fn agent_session_suppressions(config: &Config, sessions: &[Session]) -> HashSet<String> {
    let pane_index = pane_index_from_sessions(sessions);
    let mut owner_for: HashMap<String, usize> = HashMap::new();
    let mut suppressed: HashSet<String> = HashSet::new();

    for (index, session) in sessions.iter().enumerate() {
        if managed_session_skips_suppression(session) {
            if let Some(sid) = session.agent_session_id.as_ref() {
                owner_for.insert(sid.clone(), index);
            }
            continue;
        }
        let Some(sid) = session.agent_session_id.as_ref() else {
            continue;
        };
        match owner_for.get(sid).copied() {
            None => {
                owner_for.insert(sid.clone(), index);
            }
            Some(prev_index) => {
                let keep_prev =
                    pick_agent_session_owner(config, &sessions[prev_index], session, &pane_index);
                if keep_prev {
                    suppressed.insert(session.id.clone());
                } else {
                    suppressed.insert(sessions[prev_index].id.clone());
                    owner_for.insert(sid.clone(), index);
                }
            }
        }
    }

    for session in sessions {
        if managed_session_skips_suppression(session) {
            continue;
        }
        if session.agent_session_id.is_some()
            && !session_owns_agent_session(config, session, &pane_index, session.pane_pid)
        {
            suppressed.insert(session.id.clone());
        }
    }
    suppressed
}

/// Apply a precomputed suppression set to a session, hiding a stale or
/// duplicate agent binding. Pure — no I/O.
pub(crate) fn apply_agent_suppression(session: &mut Session) {
    session.agent_session_id = None;
    session.completed_thread = None;
    session.state = AgentState::Idle;
}

#[cfg(test)]
pub(crate) fn dedupe_agent_sessions(config: &Config, sessions: &mut [Session]) {
    let suppressed = agent_session_suppressions(config, sessions);
    for session in sessions.iter_mut() {
        if suppressed.contains(&session.id) {
            apply_agent_suppression(session);
        }
    }
}
pub(crate) fn same_agent_session(existing: &Session, fresh: &Session) -> bool {
    matches!(
        (existing.agent_session_id.as_deref(), fresh.agent_session_id.as_deref()),
        (Some(old), Some(new)) if old == new
    )
}
pub(crate) fn should_preserve_session_identity(existing: &Session, fresh: &Session) -> bool {
    if existing.description.is_empty()
        || crate::pty::is_weak_thread_name(&existing.description)
        || crate::pty::is_console_label(&existing.description)
    {
        return false;
    }
    if is_false_grok_label(existing, fresh) || poll_identity_differs_from_stored(existing, fresh) {
        return false;
    }
    true
}

/// Foreground process changed (e.g. workspace idle → htop); accept poll's new identity.
pub(crate) fn poll_identity_differs_from_stored(existing: &Session, fresh: &Session) -> bool {
    if existing.title == fresh.title {
        return false;
    }
    if existing.agent_session_id.is_some()
        && fresh.agent_session_id.is_some()
        && existing.agent_session_id == fresh.agent_session_id
        && !crate::pty::is_machine_derived_thread(&existing.description)
    {
        return false;
    }
    if fresh.title.is_empty() || crate::pty::is_weak_thread_name(&fresh.description) {
        return false;
    }
    existing.description != fresh.description
}

/// Poll corrected a non-grok foreground process that was previously prefixed with grok.
pub(crate) fn is_false_grok_label(existing: &Session, fresh: &Session) -> bool {
    if crate::pty::parse_app(&existing.title).as_deref() != Some("grok") {
        return false;
    }
    if fresh.agent_session_id.is_some() {
        return false;
    }
    fresh.project != "grok" && crate::pty::parse_app(&fresh.title).as_deref() != Some("grok")
}
pub(crate) fn should_preserve_existing_over_disk_upgrade(existing: &Session) -> bool {
    !existing.title_manual
        && crate::pty::is_sticky_thread_title(&existing.description)
        && !crate::pty::is_bootstrap_sidebar_thread(&existing.description)
}
pub(crate) fn finalize_session_identity(config: &Config, existing: &Session, fresh: &mut Session) {
    if apply_manual_title_identity(config, existing, fresh) {
        return;
    }
    if fresh.agent_session_id.is_some()
        && !session_has_manual_title(config, fresh)
        && same_agent_session(existing, fresh)
    {
        if let Some((title, description, project)) =
            load_auto_persisted_title_identity(config, fresh)
        {
            if crate::pty::is_sticky_thread_title(&description)
                && existing.description != description
            {
                fresh.title = title;
                fresh.description = description;
                fresh.project = project;
                fresh.title_manual = false;
                return;
            }
        }
    }
    let needs_confirmed_title = fresh.agent_session_id.is_some()
        && (crate::pty::is_machine_derived_thread(&fresh.description)
            || crate::pty::is_machine_derived_thread(&existing.description)
            || crate::pty::is_machine_derived_thread(&crate::pty::parse_description(
                &fresh.title,
            ))
            || !crate::pty::is_confident_thread_title(&fresh.description)
            || !crate::pty::is_confident_thread_title(&existing.description));
    if needs_confirmed_title {
        if same_agent_session(existing, fresh)
            && should_preserve_existing_over_disk_upgrade(existing)
        {
            fresh.title = existing.title.clone();
            fresh.description = existing.description.clone();
            fresh.project = existing.project.clone();
            fresh.title_manual = existing.title_manual;
            return;
        }
        // Only read the auto-persisted summary once we know the current title
        // is unconfirmed and actually needs replacing — sessions with a good
        // title skip this disk read entirely.
        if let Some((title, description, project)) =
            load_auto_persisted_title_identity(config, fresh)
        {
            fresh.title = title;
            fresh.description = description;
            fresh.project = project;
            fresh.title_manual = false;
            return;
        }
        if crate::pty::is_sticky_thread_title(&fresh.description) {
            fresh.title_manual = existing.title_manual;
            return;
        }
        if agent_session_title_is_placeholder(fresh) {
            ensure_agent_session_title(config, existing, fresh);
        }
        if agent_session_title_is_placeholder(fresh) {
            reset_unconfirmed_agent_title(config, fresh);
        }
        return;
    }
    if should_preserve_session_identity(existing, fresh) {
        fresh.title = existing.title.clone();
        fresh.description = existing.description.clone();
        fresh.project = existing.project.clone();
        fresh.title_manual = existing.title_manual;
        return;
    }
    if let Some((title, description, project)) = load_auto_persisted_title_identity(config, fresh) {
        fresh.title = title;
        fresh.description = description;
        fresh.project = project;
    }
    fresh.title_manual = existing.title_manual;
}
