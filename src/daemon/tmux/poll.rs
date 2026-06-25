use crate::agents::{
    assign_claude_session_for_cwd, assign_session_for_cwd, assign_thread_for_cwd,
    claude_session_index, opencode_session_index, rollout_index,
};
use crate::pty::{
    agent_from_command, bootstrap_command_from_pane_start, classify_pane,
    effective_workspace_command, infer_pane_state, is_shell_command, merge_lifecycle_state,
    resolve_session_names, PaneKind,
};
use crate::session::{
    pane_session_index, session_id_from_index, WorkspaceCatalog,
};
use crate::model::{AgentState, Session};
use anyhow::Result;
use chrono::Utc;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::windows::{effective_pane_cwd, list_windows, read_pane_state};

static LAST_POLL_AT: AtomicU64 = AtomicU64::new(0);
static POLL_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn last_poll_unix_ms() -> u64 {
    LAST_POLL_AT.load(Ordering::Relaxed)
}

pub fn poll_count() -> u64 {
    POLL_COUNT.load(Ordering::Relaxed)
}
fn is_live_managed_window(
    live_ssn: Option<&str>,
    managed: Option<&crate::session::ManagedLaunchRecord>,
) -> bool {
    match (live_ssn, managed) {
        (Some(tmux_id), Some(record)) => tmux_id == record.sessions_session_id.as_str(),
        _ => false,
    }
}

fn manifest_title_if_sticky(
    entry: &crate::session::manifest::ManifestEntry,
) -> Option<(String, String, String)> {
    let title = entry.title.as_ref()?;
    let thread = crate::pty::parse_description(title);
    if !crate::pty::is_sticky_thread_title(&thread) {
        return None;
    }
    let project = if entry.agent.is_empty() {
        crate::pty::parse_app(title).unwrap_or_default()
    } else {
        entry.agent.clone()
    };
    Some((title.clone(), thread, project))
}
pub fn poll_tmux(
    session: &str,
    home: &Path,
    state_dir: &Path,
    workspaces_path: &Path,
) -> Result<(Vec<Session>, WorkspaceCatalog)> {
    crate::process::clear_cwd_cache();
    let windows = list_windows(session)?;
    let workspaces = WorkspaceCatalog::load(workspaces_path);
    POLL_COUNT.fetch_add(1, Ordering::Relaxed);
    LAST_POLL_AT.store(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        Ordering::Relaxed,
    );
    crate::daemon::metrics::record_poll();

    let scan_started = std::time::Instant::now();
    let pane_session_index = pane_session_index(home);
    let codex_index = rollout_index(home);
    let claude_index = claude_session_index(home);
    let opencode_index = opencode_session_index(home);
    crate::daemon::metrics::record_agent_scan(scan_started.elapsed().as_micros() as u64);
    let managed_index = crate::session::load_managed_index(home);
    let manifest = crate::session::manifest::try_load_manifest(home);
    let mut assigned_codex_threads = std::collections::HashSet::new();
    let mut assigned_claude_sessions = std::collections::HashSet::new();
    let mut assigned_opencode_sessions = std::collections::HashSet::new();
    let mut sessions = Vec::new();
    for win in windows {
        let cwd = effective_pane_cwd(&win.cwd, win.pane_pid);
        let live_ssn = win.sessions_session_id.as_deref();
        let managed = live_ssn
            .and_then(|ssn| managed_index.for_ssn(ssn))
            .or_else(|| managed_index.for_window(session, win.index));
        let manifest_entry = live_ssn.and_then(|ssn| {
            manifest
                .as_ref()
                .and_then(|manifest| crate::session::manifest::manifest_entry_for_ssn(manifest, ssn))
        });
        let is_live_managed = is_live_managed_window(live_ssn, managed);
        let start_bootstrap = bootstrap_command_from_pane_start(&win.start_command);
        let bootstrap_command = workspaces
            .bootstrap_command_for_window(win.index, &cwd)
            .or(start_bootstrap.as_deref())
            .unwrap_or("");
        let effective_command =
            effective_workspace_command(bootstrap_command, &win.current_command);
        let mut runtime_agent = agent_from_command(effective_command);
        if runtime_agent.is_none() {
            if let Some(record) = managed {
                let agent = record.agent.trim();
                if !agent.is_empty()
                    && agent != "console"
                    && crate::pty::is_agent_app(agent)
                {
                    runtime_agent = Some(agent.to_string());
                }
            } else if let Some(entry) = manifest_entry {
                let agent = entry.agent.trim();
                if !agent.is_empty()
                    && agent != "console"
                    && crate::pty::is_agent_app(agent)
                {
                    runtime_agent = Some(agent.to_string());
                }
            }
        }
        let mut agent_session_id = if is_live_managed {
            managed
                .and_then(|record| record.agent_session_id.clone())
                .or_else(|| manifest_entry.and_then(|entry| entry.agent_session_id.clone()))
        } else {
            None
        };
        if agent_session_id.is_none() {
            agent_session_id = managed
                .and_then(|record| record.agent_session_id.clone())
                .or_else(|| {
                    session_id_from_index(&pane_session_index, &win.pane_id, win.index, session)
                });
        }
        let skip_cwd_inference = is_live_managed;
        if !skip_cwd_inference
            && agent_session_id.is_none()
            && runtime_agent.as_deref() == Some("codex")
        {
            agent_session_id =
                assign_thread_for_cwd(&codex_index, &cwd, &mut assigned_codex_threads);
        }
        if !skip_cwd_inference
            && agent_session_id.is_none()
            && runtime_agent.as_deref() == Some("claude")
        {
            agent_session_id =
                assign_claude_session_for_cwd(&claude_index, &cwd, &mut assigned_claude_sessions);
        }
        if !skip_cwd_inference
            && agent_session_id.is_none()
            && runtime_agent.as_deref() == Some("opencode")
        {
            agent_session_id =
                assign_session_for_cwd(&opencode_index, &cwd, &mut assigned_opencode_sessions);
        }
        if let Some(sid) = agent_session_id.as_ref() {
            let managed_bound = managed
                .and_then(|record| record.agent_session_id.as_deref())
                == Some(sid.as_str());
            if !managed_bound
                && ((is_shell_command(&win.current_command)
                    && !crate::agents::agent_session_matches_pane_cwd(home, &cwd, sid))
                    || (!is_shell_command(&win.current_command)
                        && !crate::session::env::session_env_is_live_for_pane(
                            home, sid, win.pane_pid,
                        )))
            {
                agent_session_id = None;
            }
        }
        let cwd_label =
            crate::agents::group_cwd_for_session(home, &cwd, agent_session_id.as_deref());
        let at_shell_prompt = is_shell_command(effective_command);
        let naming_command = if at_shell_prompt {
            win.current_command.as_str()
        } else {
            effective_command
        };
        let workspace = workspaces
            .workspace_ref_for_window(win.index, &cwd)
            .map(|ws| WorkspaceCatalog::workspace_ref_with_command(ws.title, naming_command))
            .or_else(|| {
                runtime_agent.as_ref().map(|agent| {
                    WorkspaceCatalog::workspace_ref_with_command(agent, effective_command)
                })
            });

        let binary = win.current_command.split_whitespace().next().unwrap_or("");
        let pane_kind = classify_pane(binary, &win.current_command, &cwd);
        let (classify_agent, classify_thread) = if at_shell_prompt {
            (None, "")
        } else {
            match &pane_kind {
                PaneKind::Shell { .. } => (None, ""),
                PaneKind::Tool { app, thread, .. } => (Some(app.as_str()), thread.as_str()),
            }
        };
        let classify_thread = if crate::pty::is_machine_derived_thread(classify_thread) {
            ""
        } else {
            classify_thread
        };
        let naming_foreground = if is_live_managed && at_shell_prompt && agent_session_id.is_none()
        {
            runtime_agent.as_deref()
        } else {
            crate::pty::poll_foreground_app(
                at_shell_prompt,
                classify_agent,
                runtime_agent.as_deref(),
            )
        };
        if agent_session_id.is_some()
            && naming_foreground.is_some_and(|app| !crate::pty::is_agent_app(app))
        {
            agent_session_id = None;
        }
        let lifecycle_state = infer_pane_state(binary, win.pane_dead, win.pane_dead_status);

        let is_idle_shell = is_shell_command(&win.current_command)
            && agent_session_id.is_none()
            && workspace
                .and_then(|ws| agent_from_command(ws.command))
                .is_none();
        let grok_state = if is_idle_shell {
            AgentState::Idle
        } else if agent_session_id.is_some() || !is_shell_command(&win.current_command) {
            read_pane_state(state_dir, &win.pane_id)
        } else {
            AgentState::Idle
        };

        // REPL agents (claude/grok/codex) stay alive between turns. Process-alive
        // lifecycle must not inject `working` on every poll — that fights hook/disk
        // completion and makes acknowledged threads flash the run spinner.
        let is_agent_repl = agent_session_id.is_some()
            || runtime_agent.is_some()
            || crate::agents::agent_for_binary(binary).is_some();
        let merged_state = if is_agent_repl {
            grok_state
        } else {
            merge_lifecycle_state(grok_state, lifecycle_state)
        };

        let poll_title_source = if agent_session_id.is_some()
            || (at_shell_prompt && managed.is_none())
        {
            ""
        } else {
            win.name.as_str()
        };
        let (path_title, path_description, project) =
            if let Some((title, description, project)) =
                manifest_entry.and_then(manifest_title_if_sticky)
            {
                (title, description, project)
            } else {
                resolve_session_names(
                    home,
                    &cwd,
                    naming_foreground,
                    agent_session_id.as_deref(),
                    poll_title_source,
                    classify_thread,
                    classify_thread,
                    workspace,
                    false,
                )
            };

        let (completed_thread, completed_at) = if merged_state == AgentState::Done {
            (Some(path_description.clone()), Some(Utc::now()))
        } else {
            (None, None)
        };

        let last_event_at = agent_session_id
            .as_deref()
            .and_then(|sid| {
                let lookup_cwd = crate::agents::disk_lookup_cwd(home, &cwd, Some(sid));
                crate::agents::session_activity_at(home, &lookup_cwd, sid)
            })
            .unwrap_or_else(Utc::now);

        // Live `@sessions.id` wins over managed records keyed by window index — after
        // cold-boot restore, indices are reused and stale managed/*.json rows linger.
        let (managed_flag, sessions_session_id, managed_agent) =
            if let Some(live) = win.sessions_session_id.clone() {
                (
                    true,
                    Some(live.clone()),
                    managed
                        .filter(|record| record.sessions_session_id == live)
                        .map(|record| record.agent.clone())
                        .or_else(|| manifest_entry.map(|entry| entry.agent.clone())),
                )
            } else if let Some(record) = managed {
                if record.pane_id.as_deref() == Some(win.pane_id.as_str()) {
                    (
                        true,
                        Some(record.sessions_session_id.clone()),
                        Some(record.agent.clone()),
                    )
                } else {
                    (false, None, None)
                }
            } else {
                (false, None, None)
            };

        sessions.push(Session {
            id: Session::session_id_from_window(win.index),
            kitty_window_id: win.index as u64,
            kitty_tab_id: 0,
            kitty_os_window_id: 0,
            tab_index: win.index,
            tmux_session: session.to_string(),
            tmux_pane_id: win.pane_id.clone(),
            pane_pid: win.pane_pid,
            agent_session_id,
            title: path_title,
            description: path_description,
            cwd,
            managed: managed_flag,
            sessions_session_id,
            managed_agent,
            cwd_label,
            project,
            state: merged_state,
            completed_thread,
            completed_at,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: win.active,
            last_event_at,
        });
    }
    Ok((sessions, workspaces))
}
