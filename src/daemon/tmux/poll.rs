use crate::agents::{
    assign_claude_session_for_cwd, assign_session_for_cwd, assign_thread_for_cwd,
    claude_session_index, opencode_session_index, rollout_index,
};
use crate::model::{AgentState, Session};
use crate::pty::{
    agent_from_command, bootstrap_command_from_pane_start, classify_pane,
    effective_workspace_command, infer_pane_state, is_shell_command, merge_lifecycle_state,
    resolve_session_names, PaneKind,
};
use crate::session::{pane_session_index, session_id_from_index, WorkspaceCatalog};
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

/// Which expensive agent on-disk indexes are required this poll for cwd-inference.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct AgentIndexNeeds {
    codex: bool,
    claude: bool,
    opencode: bool,
}

/// Pure need-detection for one window. Indexes are only used when
/// `agent_session_id` is missing and runtime looks like codex/claude/opencode
/// (OpenCode also requires a non-shell REPL when live-managed).
fn mark_agent_index_needs(
    needs: &mut AgentIndexNeeds,
    runtime_agent: Option<&str>,
    has_agent_session_id: bool,
    is_live_managed: bool,
    current_is_shell: bool,
) {
    if has_agent_session_id {
        return;
    }
    match runtime_agent {
        Some("codex") => needs.codex = true,
        Some("claude") => needs.claude = true,
        Some("opencode") => {
            // Match poll_tmux: live-managed OpenCode without sid only assigns when REPL runs.
            if !is_live_managed || !current_is_shell {
                needs.opencode = true;
            }
        }
        _ => {}
    }
}

/// Cheap pre-scan over live windows: which agent indexes might be needed for
/// unbound codex/claude/opencode panes. Grok-only machines skip all three.
fn agent_indexes_needed(
    windows: &[super::windows::TmuxWindow],
    tmux_session: &str,
    managed_index: &crate::session::ManagedLaunchIndex,
    manifest: Option<&crate::session::manifest::SessionManifest>,
    pane_session_index: &crate::session::PaneSessionIndex,
) -> AgentIndexNeeds {
    let mut needs = AgentIndexNeeds::default();
    for win in windows {
        if needs.codex && needs.claude && needs.opencode {
            break;
        }
        let live_ssn = win.sessions_session_id.as_deref();
        let managed = live_ssn
            .and_then(|ssn| managed_index.for_ssn(ssn))
            .or_else(|| managed_index.for_window(tmux_session, win.index));
        let manifest_entry = live_ssn.and_then(|ssn| {
            manifest.and_then(|m| crate::session::manifest::manifest_entry_for_ssn(m, ssn))
        });
        let is_live_managed = is_live_managed_window(live_ssn, managed);

        // Lightweight known-sid paths (no cross-agent disk validation).
        let has_agent_session_id = managed
            .and_then(|r| r.agent_session_id.as_ref())
            .is_some_and(|s| !s.is_empty())
            || manifest_entry
                .and_then(|e| e.agent_session_id.as_ref())
                .is_some_and(|s| !s.is_empty())
            || session_id_from_index(pane_session_index, &win.pane_id, win.index, tmux_session)
                .is_some();

        // Prefer command/bootstrap signals; fall back to managed/manifest agent.
        let start_bootstrap = bootstrap_command_from_pane_start(&win.start_command);
        let effective = effective_workspace_command(
            start_bootstrap.as_deref().unwrap_or(""),
            &win.current_command,
        );
        let mut runtime_agent = agent_from_command(effective);
        if runtime_agent.is_none() {
            if let Some(record) = managed {
                let agent = record.agent.trim();
                if !agent.is_empty() && agent != "console" && crate::pty::is_agent_app(agent) {
                    runtime_agent = Some(agent.to_string());
                }
            } else if let Some(entry) = manifest_entry {
                let agent = entry.agent.trim();
                if !agent.is_empty() && agent != "console" && crate::pty::is_agent_app(agent) {
                    runtime_agent = Some(agent.to_string());
                }
            }
        }

        mark_agent_index_needs(
            &mut needs,
            runtime_agent.as_deref(),
            has_agent_session_id,
            is_live_managed,
            is_shell_command(&win.current_command),
        );
    }
    needs
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

    // Cheap data first — avoid walking multi-GB Codex history when only Grok runs.
    let pane_session_index = pane_session_index(home);
    let managed_index = crate::session::load_managed_index(home);
    let manifest = crate::session::manifest::try_load_manifest(home);
    let index_needs = agent_indexes_needed(
        &windows,
        session,
        &managed_index,
        manifest.as_ref(),
        &pane_session_index,
    );

    let scan_started = std::time::Instant::now();
    let codex_index = if index_needs.codex {
        rollout_index(home)
    } else {
        Default::default()
    };
    let claude_index = if index_needs.claude {
        claude_session_index(home)
    } else {
        Default::default()
    };
    let opencode_index = if index_needs.opencode {
        opencode_session_index(home)
    } else {
        Default::default()
    };
    crate::daemon::metrics::record_agent_scan(scan_started.elapsed().as_micros() as u64);
    let mut assigned_codex_threads = std::collections::HashSet::new();
    let mut assigned_claude_sessions = std::collections::HashSet::new();
    let mut assigned_opencode_sessions = std::collections::HashSet::new();
    let mut sessions = Vec::new();
    for win in windows {
        // Detached pre-hydrated spares are not user sessions until claimed.
        // Match both `@sessions.pool=1` and the `pool ·` name prefix so a missed
        // option never leaks a spare into the sidebar.
        if win.pool || crate::daemon::tmux::is_pool_window_name(&win.name) {
            continue;
        }
        let cwd = effective_pane_cwd(&win.cwd, win.pane_pid);
        // tmux `pane_current_command` is only the process name (e.g. Python /
        // python3.13). Resolve full argv from the pane process tree so scripts
        // surface as `./train.py` instead of the interpreter binary.
        let current_command =
            crate::process::foreground_command_for_pane(win.pane_pid, &win.current_command)
                .unwrap_or_else(|| win.current_command.clone());
        let live_ssn = win.sessions_session_id.as_deref();
        let managed = live_ssn
            .and_then(|ssn| managed_index.for_ssn(ssn))
            .or_else(|| managed_index.for_window(session, win.index));
        let manifest_entry = live_ssn.and_then(|ssn| {
            manifest.as_ref().and_then(|manifest| {
                crate::session::manifest::manifest_entry_for_ssn(manifest, ssn)
            })
        });
        let is_live_managed = is_live_managed_window(live_ssn, managed);
        let start_bootstrap = bootstrap_command_from_pane_start(&win.start_command);
        let bootstrap_command = workspaces
            .bootstrap_command_for_window(win.index, &cwd)
            .or(start_bootstrap.as_deref())
            .unwrap_or("");
        let effective_command = effective_workspace_command(bootstrap_command, &current_command);
        let mut runtime_agent = agent_from_command(effective_command);
        if runtime_agent.is_none() {
            if let Some(record) = managed {
                let agent = record.agent.trim();
                if !agent.is_empty() && agent != "console" && crate::pty::is_agent_app(agent) {
                    runtime_agent = Some(agent.to_string());
                }
            } else if let Some(entry) = manifest_entry {
                let agent = entry.agent.trim();
                if !agent.is_empty() && agent != "console" && crate::pty::is_agent_app(agent) {
                    runtime_agent = Some(agent.to_string());
                }
            }
        }
        let mut agent_session_id = managed
            .and_then(|record| {
                let sid = record.agent_session_id.as_ref()?;
                // Drop cross-agent poison from the durable record (e.g. Grok UUID
                // on an OpenCode managed launch) before it becomes the group key.
                if crate::agents::agent_session_matches_expected_agent(home, sid, &record.agent) {
                    Some(sid.clone())
                } else {
                    None
                }
            })
            .or_else(|| manifest_entry.and_then(|entry| entry.agent_session_id.clone()));
        if agent_session_id.is_none() {
            agent_session_id =
                session_id_from_index(&pane_session_index, &win.pane_id, win.index, session);
        }
        if let Some(sid) = agent_session_id.as_ref() {
            if !crate::agents::agent_session_id_matches_runtime_agent(
                home,
                sid,
                runtime_agent.as_deref(),
            ) {
                agent_session_id = None;
            }
        }
        // Managed launches also declare SESSIONS_AGENT — reject SIDs that belong
        // to a different agent even when runtime_agent is still ambiguous.
        if let Some(sid) = agent_session_id.as_ref() {
            if let Some(record) = managed {
                if !crate::agents::agent_session_matches_expected_agent(home, sid, &record.agent) {
                    agent_session_id = None;
                }
            }
        }
        // Live-managed windows normally skip cwd inference so we don't swap out a
        // bound agent session. But when the managed record/manifest has no
        // agent_session_id (e.g. an OpenCode session restored before the binding
        // was back-synced), we still want to discover it from the agent's on-disk
        // index so the manifest can be repaired and future resumes work.
        let skip_cwd_inference = is_live_managed && agent_session_id.is_some();
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
        let opencode_repl_running =
            runtime_agent.as_deref() == Some("opencode") && !is_shell_command(&current_command);
        if !skip_cwd_inference
            && agent_session_id.is_none()
            && runtime_agent.as_deref() == Some("opencode")
            && (!is_live_managed || opencode_repl_running)
        {
            agent_session_id =
                assign_session_for_cwd(&opencode_index, &cwd, &mut assigned_opencode_sessions);
        }
        if let Some(sid) = agent_session_id.as_ref() {
            let managed_bound =
                managed.and_then(|record| record.agent_session_id.as_deref()) == Some(sid.as_str());
            if !managed_bound
                && ((is_shell_command(&current_command)
                    && !crate::agents::agent_session_matches_pane_cwd(home, &cwd, sid))
                    || (!is_shell_command(&current_command)
                        && !crate::session::env::session_env_is_live_for_pane(
                            home,
                            sid,
                            win.pane_pid,
                        )))
            {
                agent_session_id = None;
            }
        }
        let cwd_label =
            crate::agents::group_cwd_for_session(home, &cwd, agent_session_id.as_deref());
        let at_shell_prompt = is_shell_command(effective_command);
        let naming_command = if at_shell_prompt {
            current_command.as_str()
        } else {
            effective_command
        };
        let idle_unbound_shell =
            at_shell_prompt && agent_session_id.is_none() && runtime_agent.is_none();
        // Idle unbound shells: keep the workspace catalog row for project labeling,
        // but force a shell command so finished script bootstraps (./run-local.sh)
        // do not stick as the sidebar title after the process exits.
        let workspace = if idle_unbound_shell {
            workspaces
                .workspace_ref_for_window(win.index, &cwd)
                .map(|ws| WorkspaceCatalog::workspace_ref_with_command(ws.title, "zsh"))
        } else {
            workspaces
                .workspace_ref_for_window(win.index, &cwd)
                .map(|ws| WorkspaceCatalog::workspace_ref_with_command(ws.title, naming_command))
                .or_else(|| {
                    runtime_agent.as_ref().map(|agent| {
                        WorkspaceCatalog::workspace_ref_with_command(agent, effective_command)
                    })
                })
        };

        let binary = current_command.split_whitespace().next().unwrap_or("");
        let pane_kind = classify_pane(binary, &current_command, &cwd);
        let (classify_agent, classify_thread) = if at_shell_prompt {
            (None, "")
        } else {
            match &pane_kind {
                PaneKind::Shell { .. } => (None, ""),
                PaneKind::Tool { app, thread, .. } => (Some(app.as_str()), thread.as_str()),
            }
        };
        // Agent binary placeholders (grok-0.2.39-mac) are not thread titles; live
        // script paths remain valid even though they are "weak" for agent threads.
        let classify_thread = if crate::pty::is_machine_derived_thread(classify_thread)
            && !crate::pty::is_live_command_label(classify_thread)
        {
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
        let managed_bound = agent_session_id.as_ref().is_some_and(|sid| {
            managed.and_then(|record| record.agent_session_id.as_deref()) == Some(sid.as_str())
        });
        if agent_session_id.is_some()
            && !managed_bound
            && naming_foreground.is_some_and(|app| !crate::pty::is_agent_app(app))
        {
            agent_session_id = None;
        }
        let lifecycle_state = infer_pane_state(binary, win.pane_dead, win.pane_dead_status);

        let is_idle_shell = is_shell_command(&current_command)
            && agent_session_id.is_none()
            && workspace
                .and_then(|ws| agent_from_command(ws.command))
                .is_none();
        let grok_state = if is_idle_shell {
            AgentState::Idle
        } else if agent_session_id.is_some() || !is_shell_command(&current_command) {
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

        let poll_title_source = if agent_session_id.is_some() {
            ""
        } else if at_shell_prompt {
            if runtime_agent.is_some() {
                win.name.as_str()
            } else if crate::pty::is_console_label(win.name.as_str()) {
                // Window name "console" is only meaningful at home. Project-dir
                // shells keep an empty source so resolve uses the cwd leaf.
                let at_home = {
                    let home_str = home.to_string_lossy();
                    cwd.trim_end_matches('/') == home_str.as_ref()
                };
                if at_home {
                    crate::pty::CONSOLE_LABEL
                } else {
                    ""
                }
            } else {
                ""
            }
        } else {
            win.name.as_str()
        };
        // Live tool/script identity is a description, not a user prompt. Feeding
        // paths through `shorten_prompt` mangles them (./.local/bin/foo → localbinfoo).
        let naming_prompt = if crate::pty::is_live_command_label(classify_thread) {
            ""
        } else {
            classify_thread
        };
        let (path_title, path_description, project) = if let Some((title, description, project)) =
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
                naming_prompt,
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
        let managed_agent_from_record = |record: &crate::session::ManagedLaunchRecord| {
            let agent = record.agent.trim();
            if !agent.is_empty() && agent != "console" && crate::pty::is_agent_app(agent) {
                return Some(agent.to_string());
            }
            record
                .agent_session_id
                .as_deref()
                .and_then(|sid| crate::agents::detect_agent_id_for_session(home, sid))
                .map(str::to_string)
        };
        let (managed_flag, sessions_session_id, managed_agent) =
            if let Some(live) = win.sessions_session_id.clone() {
                (
                    true,
                    Some(live.clone()),
                    managed
                        .filter(|record| record.sessions_session_id == live)
                        .and_then(managed_agent_from_record)
                        .or_else(|| {
                            manifest_entry
                                .map(|entry| entry.agent.clone())
                                .filter(|agent| {
                                    !agent.is_empty()
                                        && agent != "console"
                                        && crate::pty::is_agent_app(agent)
                                })
                        })
                        .or_else(|| {
                            agent_session_id.as_deref().and_then(|sid| {
                                crate::agents::detect_agent_id_for_session(home, sid)
                                    .map(str::to_string)
                            })
                        }),
                )
            } else if let Some(record) = managed {
                if record.pane_id.as_deref() == Some(win.pane_id.as_str()) {
                    (
                        true,
                        Some(record.sessions_session_id.clone()),
                        managed_agent_from_record(record),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::tmux::windows::TmuxWindow;
    use crate::session::{ManagedLaunchIndex, PaneSessionIndex};

    fn sample_window(index: u32, current: &str, start: &str, ssn: Option<&str>) -> TmuxWindow {
        TmuxWindow {
            index,
            name: "session".into(),
            cwd: "/home/testuser/projects/acme".into(),
            current_command: current.into(),
            start_command: start.into(),
            pane_id: format!("%{index}"),
            pane_pid: 0,
            active: true,
            pane_dead: false,
            pane_dead_status: None,
            sessions_session_id: ssn.map(str::to_string),
            pool: false,
        }
    }

    #[test]
    fn mark_needs_pure_grok_skips_all_indexes() {
        let mut needs = AgentIndexNeeds::default();
        mark_agent_index_needs(&mut needs, Some("grok"), false, true, false);
        mark_agent_index_needs(&mut needs, Some("grok"), true, true, false);
        mark_agent_index_needs(&mut needs, None, false, false, true);
        assert_eq!(
            needs,
            AgentIndexNeeds {
                codex: false,
                claude: false,
                opencode: false,
            }
        );
    }

    #[test]
    fn mark_needs_unbound_codex_claude_opencode() {
        let mut needs = AgentIndexNeeds::default();
        mark_agent_index_needs(&mut needs, Some("codex"), false, false, false);
        assert!(needs.codex && !needs.claude && !needs.opencode);

        mark_agent_index_needs(&mut needs, Some("claude"), false, true, true);
        assert!(needs.codex && needs.claude && !needs.opencode);

        mark_agent_index_needs(&mut needs, Some("opencode"), false, false, true);
        assert!(needs.opencode);
    }

    #[test]
    fn mark_needs_bound_session_skips_index() {
        let mut needs = AgentIndexNeeds::default();
        mark_agent_index_needs(&mut needs, Some("codex"), true, true, false);
        mark_agent_index_needs(&mut needs, Some("claude"), true, true, false);
        mark_agent_index_needs(&mut needs, Some("opencode"), true, true, false);
        assert_eq!(needs, AgentIndexNeeds::default());
    }

    #[test]
    fn mark_needs_live_managed_opencode_shell_without_sid_skips() {
        // Live-managed OpenCode at shell without sid does not call assign_session_for_cwd.
        let mut needs = AgentIndexNeeds::default();
        mark_agent_index_needs(&mut needs, Some("opencode"), false, true, true);
        assert!(!needs.opencode);

        // REPL running does need the index for discovery.
        mark_agent_index_needs(&mut needs, Some("opencode"), false, true, false);
        assert!(needs.opencode);
    }

    #[test]
    fn agent_indexes_needed_pure_grok_windows() {
        let windows = vec![
            sample_window(1, "grok", "grok", Some("ssn_g1")),
            sample_window(
                2,
                "zsh",
                r#"/bin/zsh -lc "grok || exec /bin/zsh -l""#,
                Some("ssn_g2"),
            ),
        ];
        let needs = agent_indexes_needed(
            &windows,
            "workspace",
            &ManagedLaunchIndex::default(),
            None,
            &PaneSessionIndex::default(),
        );
        assert_eq!(
            needs,
            AgentIndexNeeds {
                codex: false,
                claude: false,
                opencode: false,
            }
        );
    }

    #[test]
    fn agent_indexes_needed_unbound_codex_command() {
        let windows = vec![sample_window(3, "codex", "codex", None)];
        let needs = agent_indexes_needed(
            &windows,
            "workspace",
            &ManagedLaunchIndex::default(),
            None,
            &PaneSessionIndex::default(),
        );
        assert!(needs.codex);
        assert!(!needs.claude);
        assert!(!needs.opencode);
    }

    #[test]
    fn agent_indexes_needed_unbound_claude_and_opencode_commands() {
        let windows = vec![
            sample_window(4, "claude", "claude", None),
            sample_window(5, "opencode", "opencode", None),
        ];
        let needs = agent_indexes_needed(
            &windows,
            "workspace",
            &ManagedLaunchIndex::default(),
            None,
            &PaneSessionIndex::default(),
        );
        assert!(!needs.codex);
        assert!(needs.claude);
        assert!(needs.opencode);
    }
}
