//! Launch an automation run as a background managed agent session.

use super::findings::{classify_output, RESULT_MARKER};
use super::schema::{Automation, AutomationRun, ExecutionEnvironment, RunOutcome, RunStatus};
use super::store;
use crate::agents;
use crate::config::Config;
use crate::daemon::tmux;
use crate::session::lifecycle::{create_unified, LaunchSpec};
use crate::session::managed::new_sessions_session_id;
use crate::session::manifest::ManifestSource;
use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};

/// How long a run may stay `running` before the daemon marks it failed.
pub const STALE_RUN_TIMEOUT: Duration = Duration::hours(2);

/// Fire an automation once (manual or scheduled). Creates a run record + detached tmux window.
pub fn fire_automation(
    config: &Config,
    automation: &Automation,
    caught_up: bool,
) -> Result<AutomationRun> {
    if matches!(
        automation.execution_environment,
        ExecutionEnvironment::Worktree
    ) {
        bail!("worktree execution is not implemented yet; use execution_environment = local");
    }
    let cwd = automation
        .primary_cwd()
        .with_context(|| format!("automation {} has no cwd", automation.id))?;
    if !std::path::Path::new(cwd).is_dir() {
        bail!("automation cwd does not exist: {cwd}");
    }
    if automation.agent == "console" {
        bail!("console agent is not supported for automations");
    }

    let model = if automation.model.trim().is_empty() {
        agents::default_model_id(&automation.agent).to_string()
    } else {
        automation.model.clone()
    };

    let mut run = AutomationRun::new(automation, cwd);
    run.model = model.clone();
    run.caught_up = caught_up;
    run.status = RunStatus::Pending;
    store::save_run(config, &run)?;

    let prompt = automation.prompt.as_str();
    let deliver_via_tmux = !prompt.is_empty()
        && agents::deliver_prompt_via_tmux(&automation.agent, &model, cwd, prompt);
    let launch_command = agents::build_launch_command_with_prompt(
        &automation.agent,
        &model,
        if deliver_via_tmux { None } else { Some(prompt) },
    );

    let sessions_session_id = new_sessions_session_id();
    let window_name = automation_window_name(&automation.name, &automation.id);

    let created = match create_unified(
        config,
        LaunchSpec {
            sessions_session_id: sessions_session_id.clone(),
            source: ManifestSource::Automation,
            cwd: cwd.to_string(),
            agent: automation.agent.clone(),
            launch_command,
            workspace_index: None,
            focus: false,
            window_name: Some(window_name),
            bootstrap_new_session: false,
            model_id: Some(model.clone()),
            user_prompt: if prompt.is_empty() {
                None
            } else {
                Some(prompt.to_string())
            },
        },
    ) {
        Ok(created) => created,
        Err(err) => {
            run.status = RunStatus::Failed;
            run.finished_at = Some(Utc::now());
            run.error = Some(err.to_string());
            run.outcome = Some(RunOutcome::Unknown);
            run.unread = true;
            store::save_run(config, &run)?;
            bump_failure_state(config, automation, &run)?;
            return Err(err).context("launch automation session");
        }
    };

    run.sessions_session_id = Some(sessions_session_id);
    run.window_index = Some(created.index);
    run.status = RunStatus::Running;
    store::save_run(config, &run)?;

    if deliver_via_tmux && !prompt.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(2_000));
        if let Err(e) = tmux::send_literal_to_window(&config.tmux_session, created.index, prompt) {
            tracing::warn!("automation {} prompt delivery failed: {e}", automation.id);
        } else {
            let _ = tmux::send_keys_to_window(&config.tmux_session, created.index, &["Enter"]);
        }
    }

    let mut state = store::load_state(config, &automation.id).unwrap_or_default();
    state.last_fired_at = Some(Utc::now());
    state.last_run_id = Some(run.id.clone());
    state.last_error = None;
    if let Ok(salt) = store::load_or_create_jitter_salt(config) {
        if let Ok(Some(next)) = super::schedule::next_fire_after(automation, Utc::now(), &salt) {
            state.next_due_at = Some(next);
        }
    }
    let _ = store::save_state(config, &automation.id, &state);

    // Best-effort daemon refresh so the sidebar picks up the new window.
    refresh_daemon_best_effort(config);

    Ok(run)
}

fn automation_window_name(name: &str, id: &str) -> String {
    let base = if name.trim().is_empty() { id } else { name };
    let slug: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "auto" } else { slug };
    // tmux window names: keep short
    let slug: String = slug.chars().take(28).collect();
    format!("auto-{slug}")
}

fn bump_failure_state(config: &Config, automation: &Automation, run: &AutomationRun) -> Result<()> {
    let mut state = store::load_state(config, &automation.id).unwrap_or_default();
    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    state.last_run_id = Some(run.id.clone());
    state.last_error = run.error.clone();
    store::save_state(config, &automation.id, &state)
}

fn refresh_daemon_best_effort(config: &Config) {
    let client = crate::bar::client::DaemonClient::from_config(config);
    client.refresh_async();
}

/// Reconcile running runs against live sessions / timeouts.
pub fn reconcile_running_runs(config: &Config) -> Result<usize> {
    let mut updated = 0;
    let live_ssns: std::collections::HashSet<String> =
        crate::daemon::tmux::list_live_sessions_session_ids(&config.tmux_session)
            .unwrap_or_default()
            .into_keys()
            .collect();

    // Optional: daemon session state for Done/Error
    let daemon_sessions = load_daemon_session_map(config);

    for automation in store::list_automations(config)? {
        for mut run in store::list_runs(config, &automation.id, 30)? {
            if run.status != RunStatus::Running && run.status != RunStatus::Pending {
                continue;
            }
            let mut changed = false;

            // Stale timeout
            if Utc::now() - run.started_at > STALE_RUN_TIMEOUT {
                run.status = RunStatus::Failed;
                run.finished_at = Some(Utc::now());
                run.error = Some("run timed out (stale)".into());
                run.outcome = Some(RunOutcome::Unknown);
                run.unread = true;
                changed = true;
            } else if let Some(ref ssn) = run.sessions_session_id {
                if !live_ssns.contains(ssn) && run.status == RunStatus::Running {
                    // Window gone — treat as interrupted unless daemon said Done first
                    if let Some(agent_state) = daemon_sessions.get(ssn) {
                        match agent_state.as_str() {
                            "done" => {
                                complete_run_success(&mut run, None);
                                changed = true;
                            }
                            "error" => {
                                run.status = RunStatus::Failed;
                                run.finished_at = Some(Utc::now());
                                run.outcome = Some(RunOutcome::Unknown);
                                run.error = Some("agent reported error".into());
                                run.unread = true;
                                changed = true;
                            }
                            _ => {
                                run.status = RunStatus::Failed;
                                run.finished_at = Some(Utc::now());
                                run.error = Some("session window closed before completion".into());
                                run.outcome = Some(RunOutcome::Unknown);
                                run.unread = true;
                                changed = true;
                            }
                        }
                    } else {
                        run.status = RunStatus::Failed;
                        run.finished_at = Some(Utc::now());
                        run.error = Some("session window closed before completion".into());
                        run.outcome = Some(RunOutcome::Unknown);
                        run.unread = true;
                        changed = true;
                    }
                } else if let Some(agent_state) = daemon_sessions.get(ssn) {
                    match agent_state.as_str() {
                        "done" => {
                            complete_run_success(&mut run, None);
                            changed = true;
                        }
                        "error" => {
                            run.status = RunStatus::Failed;
                            run.finished_at = Some(Utc::now());
                            run.outcome = Some(RunOutcome::Unknown);
                            run.error = Some("agent reported error".into());
                            run.unread = true;
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }

            if changed {
                // Try to classify from pane capture if still available
                if run.status == RunStatus::Done {
                    if let Some(wi) = run.window_index {
                        if let Ok(text) = capture_pane_tail(config, wi) {
                            let outcome = classify_output(&text);
                            apply_outcome(&mut run, outcome);
                        }
                    }
                }
                store::save_run(config, &run)?;
                let mut state = store::load_state(config, &automation.id).unwrap_or_default();
                match run.status {
                    RunStatus::Failed => {
                        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                        state.last_error = run.error.clone();
                    }
                    RunStatus::Done | RunStatus::Archived => {
                        state.consecutive_failures = 0;
                        state.last_error = None;
                    }
                    _ => {}
                }
                state.last_run_id = Some(run.id.clone());
                let _ = store::save_state(config, &automation.id, &state);
                updated += 1;
            }
        }
    }
    Ok(updated)
}

fn complete_run_success(run: &mut AutomationRun, summary: Option<String>) {
    run.status = RunStatus::Done;
    run.finished_at = Some(Utc::now());
    run.summary = summary;
    // Default findings until pane classify
    if run.outcome.is_none() {
        run.outcome = Some(RunOutcome::Findings);
    }
    run.unread = !matches!(run.outcome, Some(RunOutcome::Empty));
}

fn apply_outcome(run: &mut AutomationRun, outcome: RunOutcome) {
    run.outcome = Some(outcome);
    match outcome {
        RunOutcome::Empty => {
            run.status = RunStatus::Archived;
            run.unread = false;
        }
        RunOutcome::Findings | RunOutcome::Unknown => {
            run.unread = true;
        }
    }
}

fn capture_pane_tail(config: &Config, window_index: u32) -> Result<String> {
    // Capture last ~100 lines for AUTOMATION_RESULT marker.
    let target = format!("{}:{}", config.tmux_session, window_index);
    let output = std::process::Command::new("tmux")
        .args(["capture-pane", "-p", "-t", &target, "-S", "-100"])
        .output()
        .context("tmux capture-pane")?;
    if !output.status.success() {
        bail!("capture-pane failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn load_daemon_session_map(config: &Config) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let state = crate::daemon::persist::load_state_or_empty(config);
    for session in state.sessions {
        if let Some(ssn) = session.sessions_session_id.clone() {
            map.insert(ssn, session.state.as_str().to_string());
        }
    }
    let _ = RESULT_MARKER; // keep marker module linked for docs
    map
}

/// Evaluate due automations and fire once each. Returns number of fires.
pub fn tick_scheduler(config: &Config) -> Result<usize> {
    store::ensure_root(config)?;
    let salt = store::load_or_create_jitter_salt(config)?;
    let now = Utc::now();
    let mut fired = 0;

    // Reconcile first so completed runs free capacity.
    let _ = reconcile_running_runs(config);

    for automation in store::list_automations(config)? {
        if !automation.is_active() {
            continue;
        }
        // Skip if a run is already in progress for this automation.
        let runs = store::list_runs(config, &automation.id, 5)?;
        if runs.iter().any(|r| r.status == RunStatus::Running) {
            continue;
        }
        let state = store::load_state(config, &automation.id).unwrap_or_default();
        let due = super::schedule::is_due(&automation, state.last_fired_at, now, &salt)?;
        if !due {
            continue;
        }
        let caught_up = state
            .last_fired_at
            .map(|t| now - t > Duration::hours(12))
            .unwrap_or(false);
        match fire_automation(config, &automation, caught_up) {
            Ok(_) => {
                fired += 1;
                tracing::info!("automation fired: {}", automation.id);
            }
            Err(err) => {
                tracing::error!("automation {} fire failed: {err}", automation.id);
            }
        }
    }
    Ok(fired)
}
