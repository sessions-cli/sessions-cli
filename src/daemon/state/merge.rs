// WS-08: extracted from daemon/state.rs

use crate::config::Config;
use crate::daemon::state::identity::{
    agent_session_suppressions, build_previous_ssn_index, copy_stable_refresh_identity,
    finalize_session_identity, pane_index_from_sessions, refresh_merge_prior,
    resolve_agent_session_id,
};
use crate::daemon::state::titles::{
    apply_manual_title_identity, ensure_agent_session_title, persist_agent_session_title,
    reset_unconfirmed_agent_title, restore_manual_title_from_tab, restore_title_from_disk,
    session_has_manual_title,
};
use crate::daemon::state::disk_sync::{
    apply_refresh_messaged_at, sync_live_activity_from_disk, sync_turn_completion_from_disk,
};
use crate::daemon::state::notify::{is_console_session, is_home_cwd};
use crate::daemon::tmux::{save_pane_state, write_session_env_tmux};
use crate::pty::resolve_session_names;
use crate::session::{load_session_env, WorkspaceCatalog};
use crate::model::{AgentState, Session};
use crate::session::manifest::{
    hydrate_session_from_manifest, load_manifest, manifest_entry_for_ssn,
};
use crate::daemon::state::ClosedSessionMarker;
use std::collections::HashMap;

pub(crate) fn preserve_hook_cwd_over_stale_poll(config: &Config, existing: &Session, fresh: &mut Session) {
    if existing.cwd.is_empty() || existing.cwd == fresh.cwd {
        return;
    }
    if !is_home_cwd(&fresh.cwd, &config.home) || is_home_cwd(&existing.cwd, &config.home) {
        return;
    }
    fresh.cwd = existing.cwd.clone();
    fresh.cwd_label = existing.cwd_label.clone();
}
pub(crate) fn agent_state_rank(state: AgentState) -> u8 {
    match state {
        AgentState::Idle => 0,
        AgentState::Done => 1,
        AgentState::Working => 2,
        AgentState::Approval => 3,
        AgentState::Error => 4,
    }
}

pub(crate) fn coalesce_agent_state(
    stored: AgentState,
    polled: AgentState,
    polled_completion: bool,
) -> AgentState {
    let polled = polled_state_for_refresh(polled, polled_completion);
    if stored == AgentState::Done {
        return stored;
    }
    if polled_completion && polled == AgentState::Done {
        return polled;
    }
    if agent_state_rank(polled) > agent_state_rank(stored) {
        return polled;
    }
    stored
}

/// Stale pane-file `done` is ignored; lifecycle/hook completions carry `completed_thread`.
pub(crate) fn polled_state_for_refresh(polled: AgentState, polled_completion: bool) -> AgentState {
    if polled == AgentState::Done && !polled_completion {
        AgentState::Idle
    } else {
        polled
    }
}

pub(crate) fn acknowledge_completion_on_focus(existing: Option<&Session>, session: &mut Session) -> bool {
    let was_active = existing.is_some_and(|entry| entry.is_active);
    if session.is_active && !was_active {
        session.acknowledge_if_done()
    } else {
        false
    }
}
pub(crate) struct RefreshComputation {
    /// Merged sessions (close markers already filtered out), in poll order.
    pub(crate) merged: Vec<Session>,
    /// Window session ids seen this poll — drives removal of vanished sessions.
    pub(crate) polled_ids: std::collections::HashSet<String>,
    /// Pane ids seen this poll — used to expire stale close markers.
    pub(crate) polled_panes: std::collections::HashSet<String>,
    /// Agent session ids seen this poll — used to expire stale close markers.
    pub(crate) polled_sids: std::collections::HashSet<String>,
    /// Session ids whose agent binding should be hidden from snapshots.
    /// Computed here (off-lock) so `sorted_sessions` performs no I/O.
    pub(crate) agent_suppressions: std::collections::HashSet<String>,
}

/// Run the full per-session refresh merge off the lock-holding path.
///
/// This performs all the disk I/O of a poll cycle (agent lookups, summary and
/// env reads, title restoration, pane-state and env writes) against a snapshot
/// of the previous state. The caller swaps the result in under the write lock,
/// reconciling against any hook events that landed during the merge.
pub(crate) fn compute_refresh(
    config: &Config,
    polled: Vec<Session>,
    previous: &HashMap<String, Session>,
    closed: &std::collections::HashSet<ClosedSessionMarker>,
    workspaces: &WorkspaceCatalog,
) -> RefreshComputation {
    // Identity sets from the raw poll (before filtering closed sessions) — these
    // decide which close markers and sessions remain valid.
    let polled_panes: std::collections::HashSet<String> = polled
        .iter()
        .map(|s| s.tmux_pane_id.clone())
        .filter(|p| !p.is_empty())
        .collect();
    let polled_sids: std::collections::HashSet<String> = polled
        .iter()
        .filter_map(|s| s.agent_session_id.clone())
        .collect();

    let polled: Vec<Session> = polled
        .into_iter()
        .filter(|session| !closed.iter().any(|marker| marker.matches(session)))
        .collect();
    let polled_ids: std::collections::HashSet<String> =
        polled.iter().map(|p| p.id.clone()).collect();
    let pane_index = pane_index_from_sessions(&polled);
    let by_ssn = build_previous_ssn_index(previous);
    let manifest = load_manifest(config).ok();

    let mut merged = Vec::with_capacity(polled.len());
    for mut fresh in polled {
        let manifest_entry = fresh.sessions_session_id.as_ref().and_then(|ssn| {
            manifest
                .as_ref()
                .and_then(|manifest| manifest_entry_for_ssn(manifest, ssn))
        });
        let (existing, stable_only) = refresh_merge_prior(previous, &fresh, &by_ssn);
        let was_complete = existing.is_some_and(Session::thread_is_complete);
        if let Some(prior) = existing {
            if stable_only {
                copy_stable_refresh_identity(prior, &mut fresh);
            }
            merge_session_refresh_state(config, prior, &mut fresh, &pane_index, workspaces);
        } else {
            if !restore_manual_title_from_tab(config, fresh.tab_index, &mut fresh) {
                if let Some(sid) = fresh.agent_session_id.clone() {
                    let _ = restore_title_from_disk(config, &sid, &mut fresh);
                }
            }
            let _ = sync_turn_completion_from_disk(config, &mut fresh);
            if let Some(entry) = manifest_entry {
                hydrate_session_from_manifest(&config.home, &mut fresh, entry);
            }
        }
        apply_refresh_messaged_at(config, &mut fresh, manifest_entry);
        if let Some(sid) = fresh.agent_session_id.as_deref() {
            let env = load_session_env(&config.session_env_path(sid));
            if env.tmux_pane_id.as_deref() != Some(fresh.tmux_pane_id.as_str())
                || env.window_index != Some(fresh.tab_index)
            {
                let _ = write_session_env_tmux(
                    config,
                    sid,
                    Some(&fresh.tmux_pane_id),
                    Some(fresh.tab_index),
                    &fresh.tmux_session,
                    None,
                    None,
                );
            }
        }
        if fresh.thread_is_complete() && !was_complete && !fresh.tmux_pane_id.is_empty() {
            let _ = save_pane_state(
                &config.tmux_state_dir,
                &fresh.tmux_pane_id,
                AgentState::Idle,
            );
        }
        if acknowledge_completion_on_focus(existing, &mut fresh)
            && !fresh.tmux_pane_id.is_empty()
        {
            let _ = save_pane_state(
                &config.tmux_state_dir,
                &fresh.tmux_pane_id,
                AgentState::Idle,
            );
        }
        merged.push(fresh);
    }

    // Resolve stale/duplicate agent bindings here (I/O) so the snapshot build
    // under the write lock — and every read-path render — stays pure.
    let agent_suppressions = agent_session_suppressions(config, &merged);

    RefreshComputation {
        merged,
        polled_ids,
        polled_panes,
        polled_sids,
        agent_suppressions,
    }
}
pub(crate) fn merge_session_refresh_state(
    config: &Config,
    existing: &Session,
    fresh: &mut Session,
    pane_index: &HashMap<String, u32>,
    workspaces: &WorkspaceCatalog,
) {
    fresh.managed = existing.managed || fresh.managed;
    fresh.sessions_session_id = fresh
        .sessions_session_id
        .clone()
        .or_else(|| existing.sessions_session_id.clone());
    fresh.managed_agent = existing
        .managed_agent
        .clone()
        .or_else(|| fresh.managed_agent.clone());
    fresh.last_event_at = existing.last_event_at;
    if fresh.completed_at.is_none() {
        fresh.completed_at = existing.completed_at;
    }
    fresh.prompt_submitted = existing.prompt_submitted;
    fresh.messaged_at = existing.messaged_at;

    if is_console_session(fresh) {
        fresh.agent_session_id = None;
        fresh.state = if existing.thread_is_complete() {
            existing.state
        } else {
            AgentState::Idle
        };
        apply_manual_title_identity(config, existing, fresh);
        return;
    }

    fresh.agent_session_id = if fresh.managed {
        existing
            .agent_session_id
            .clone()
            .or_else(|| resolve_agent_session_id(config, existing, fresh, pane_index))
    } else {
        resolve_agent_session_id(config, existing, fresh, pane_index)
    };
    if crate::pty::is_foreground_tool_identity(&fresh.title, &fresh.description) {
        fresh.agent_session_id = None;
    }
    fresh.cwd_label = if let Some(sid) = fresh.agent_session_id.as_deref() {
        crate::agents::group_cwd_for_session(&config.home, &fresh.cwd, Some(sid))
    } else {
        crate::pty::format_tilde_path(&fresh.cwd, &config.home)
    };

    if existing.agent_session_id.is_some() && fresh.agent_session_id.is_none() {
        fresh.state = AgentState::Idle;
        fresh.completed_thread = None;
        fresh.completed_at = None;
        fresh.messaged_at = None;
        if !session_has_manual_title(config, existing)
            && !crate::pty::is_foreground_tool_identity(&fresh.title, &fresh.description)
        {
            let workspace = workspaces.workspace_ref_for_window(fresh.tab_index, &fresh.cwd);
            let (title, description, project) = resolve_session_names(
                &config.home,
                &fresh.cwd,
                None,
                None,
                "",
                "",
                "",
                workspace,
                false,
            );
            fresh.title = title;
            fresh.description = description;
            fresh.project = project;
        }
        apply_manual_title_identity(config, existing, fresh);
        ensure_agent_session_title(config, existing, fresh);
        return;
    }

    if existing.thread_is_complete() {
        fresh.title_manual = existing.title_manual;
        let placeholder_completion = existing
            .completed_thread
            .as_deref()
            .is_some_and(crate::pty::is_machine_derived_thread);
        let unconfirmed_completion = !crate::pty::is_sticky_thread_title(&existing.description)
            || existing
                .completed_thread
                .as_deref()
                .is_some_and(|thread| !crate::pty::is_sticky_thread_title(thread));
        if crate::pty::is_machine_derived_thread(&existing.description)
            || placeholder_completion
            || unconfirmed_completion
        {
            ensure_agent_session_title(config, existing, fresh);
            if crate::pty::is_sticky_thread_title(&fresh.description) {
                fresh.completed_thread = Some(fresh.description.clone());
                fresh.state = existing.state;
                fresh.completed_at = existing.completed_at;
            } else {
                reset_unconfirmed_agent_title(config, fresh);
                fresh.completed_thread = None;
                fresh.completed_at = None;
                fresh.state = AgentState::Idle;
            }
        } else {
            fresh.completed_thread = existing.completed_thread.clone();
            fresh.state = existing.state;
            fresh.completed_at = existing.completed_at;
            fresh.title = existing.title.clone();
            fresh.description = existing.description.clone();
            fresh.project = existing.project.clone();
            persist_agent_session_title(config, fresh);
        }
        return;
    }

    if fresh.completed_thread.is_none() {
        if matches!(
            existing.state,
            AgentState::Working | AgentState::Approval | AgentState::Error
        ) {
            fresh.completed_thread = None;
        } else {
            fresh.completed_thread = existing.completed_thread.clone();
        }
    }

    if existing.completion_acknowledged() {
        if matches!(
            fresh.state,
            AgentState::Working | AgentState::Approval | AgentState::Error
        ) {
            fresh.completed_thread = None;
            finalize_session_identity(config, existing, fresh);
            return;
        }
        sync_live_activity_from_disk(config, fresh);
        if matches!(
            fresh.state,
            AgentState::Working | AgentState::Approval | AgentState::Error
        ) {
            fresh.completed_thread = None;
            finalize_session_identity(config, existing, fresh);
            return;
        }
        fresh.state = AgentState::Idle;
        finalize_session_identity(config, existing, fresh);
        return;
    }

    let polled_completion = fresh.completed_thread.is_some() && fresh.state == AgentState::Done;
    fresh.state = coalesce_agent_state(existing.state, fresh.state, polled_completion);
    sync_live_activity_from_disk(config, fresh);
    sync_turn_completion_from_disk(config, fresh);
    preserve_hook_cwd_over_stale_poll(config, existing, fresh);
    finalize_session_identity(config, existing, fresh);
    ensure_agent_session_title(config, existing, fresh);
}
