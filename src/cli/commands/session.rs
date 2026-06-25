use crate::cli::commands::daemon::send_sync;
use crate::cli::config_with_socket;
use crate::config::Config;
use crate::daemon;
use crate::daemon::persist::load_state_or_empty;
use crate::doctor;
use crate::hooks;
use crate::model::{ClientCommand, Session};
use crate::session::{self, CloseTarget, CloseOutcome};
use crate::telemetry;
use crate::version;
use anyhow::Result;
use std::path::PathBuf;

pub fn run_list(socket: Option<PathBuf>) -> Result<()> {
    let config = config_with_socket(socket);
    let response = send_sync(&config.socket_path, &ClientCommand::List)?;
    println!("{response}");
    Ok(())
}

pub fn run_focus(socket: Option<PathBuf>, index: u32) -> Result<()> {
    let config = config_with_socket(socket);
    send_sync(
        &config.socket_path,
        &ClientCommand::Focus {
            window_index: index,
            tab_index: None,
        },
    )?;
    Ok(())
}

pub fn run_status(socket: Option<PathBuf>, verbose: bool, json: bool) -> Result<()> {
    telemetry::record_feature(telemetry::FeatureId::CliStatus, telemetry::feature::Source::Cli);
    let config = config_with_socket(socket);
    let response = send_sync(&config.socket_path, &ClientCommand::Status { verbose })?;
    if json {
        println!("{response}");
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(&response)?;
    let healthy = value
        .get("healthy")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let count = value
        .get("session_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
    let last_poll = value
        .get("last_poll_at")
        .and_then(|v| v.as_str())
        .unwrap_or("never");
    println!(
        "sessionsd: {}",
        if healthy { "healthy" } else { "unhealthy" }
    );
    let app_version = value
        .get("app_version")
        .and_then(|v| v.as_str())
        .unwrap_or(version::VERSION);
    println!("sessions: {count}");
    println!("app_version: {app_version}");
    println!("state_version: {version}");
    println!("last_poll_at: {last_poll}");
    if let Some(update) = value.get("update") {
        if let Some(av) = update.get("available_version").and_then(|v| v.as_str()) {
            let urgency = update
                .get("urgency")
                .and_then(|v| v.as_str())
                .unwrap_or("none");
            println!("update: {av} ({urgency})");
        }
    }
    if verbose {
        if let Some(metrics) = value.get("metrics") {
            println!("metrics: {}", serde_json::to_string_pretty(metrics)?);
        }
    }
    Ok(())
}

pub fn run_reconcile(socket: Option<PathBuf>) -> Result<()> {
    let config = config_with_socket(socket);
    session::send_restore_complete(&config)
}

pub fn run_refresh(socket: Option<PathBuf>) -> Result<()> {
    let config = config_with_socket(socket);
    if daemon::server::socket_responds(&config.socket_path) {
        let _ = send_sync(&config.socket_path, &ClientCommand::Refresh);
    }
    Ok(())
}

pub fn run_new() -> Result<()> {
    let config = Config::default();
    daemon::tmux::create_console_window(&config)?;
    refresh_after_window_change(&config);
    Ok(())
}

pub fn run_panel_new_session() -> Result<()> {
    let config = Config::default();
    daemon::tmux::toggle_workspace_new_session(&config.tmux_ui_session, &config.tmux_session)?;
    Ok(())
}

pub fn run_panel_open_new_session() -> Result<()> {
    let config = Config::default();
    daemon::tmux::open_workspace_new_session(&config.tmux_ui_session, &config.tmux_session)?;
    Ok(())
}

pub fn run_panel_settings() -> Result<()> {
    let config = Config::default();
    daemon::tmux::toggle_workspace_settings(&config.tmux_ui_session, &config.tmux_session)?;
    Ok(())
}

pub fn run_panel_dismiss() -> Result<()> {
    let config = Config::default();
    daemon::tmux::dismiss_ui_panel_popups(&config.tmux_ui_session, &config.tmux_session)?;
    Ok(())
}

pub fn refresh_after_window_change(config: &Config) {
    let socket_path = config.socket_path.clone();
    std::thread::spawn(move || {
        if daemon::server::socket_responds(&socket_path) {
            let _ = send_sync(&socket_path, &ClientCommand::Refresh);
        }
    });
}

pub fn run_close() -> Result<()> {
    let config = Config::default();
    let (index, _, _) = daemon::tmux::active_window_summary(&config.tmux_session)?;
    let session_id = Session::session_id_from_window(index);
    let outcome = session::close_unified(
        &config,
        CloseTarget {
            session_id: Some(session_id),
            sessions_session_id: None,
            window_index: Some(index),
        },
    )?;
    sync_daemon_after_close(&config, &outcome);
    Ok(())
}

pub fn run_confirm_close() -> Result<()> {
    let config = Config::default();
    daemon::tmux::confirm_close_active_window(&config.tmux_session)
}

pub fn sync_daemon_after_close(config: &Config, outcome: &CloseOutcome) {
    if !daemon::server::socket_responds(&config.socket_path) {
        return;
    }
    let close_cmd = ClientCommand::CloseSession {
        session_id: outcome.session_id.clone(),
    };
    if send_sync(&config.socket_path, &close_cmd).is_err() {
        let _ = send_sync(&config.socket_path, &ClientCommand::Refresh);
    }
}

pub fn run_leave() -> Result<()> {
    daemon::tmux::detach_current_client()
}

pub fn run_down() -> Result<()> {
    let config = Config::default();
    run_down_with_config(&config)
}

/// `sessions down` protocol: sync manifest while daemon/socket or sessionsd snapshot
/// is still available, then kill tmux sessions.
pub(crate) fn run_down_with_config(config: &Config) -> Result<()> {
    flush_and_sync_manifest_before_down(config)?;
    kill_sessions_for_down(config)
}

pub(crate) fn daemon_sessions_for_down_sync(config: &Config) -> Vec<Session> {
    if daemon::server::socket_responds(&config.socket_path) {
        let _ = send_sync(&config.socket_path, &ClientCommand::FlushManifest);
        if let Ok(response) = send_sync(&config.socket_path, &ClientCommand::List) {
            if let Ok(sessions) = serde_json::from_str::<Vec<Session>>(&response) {
                if !sessions.is_empty() {
                    return sessions;
                }
            }
        }
    }
    load_state_or_empty(config).sessions
}

pub(crate) fn flush_and_sync_manifest_before_down(config: &Config) -> Result<()> {
    session::sync_manifest_from_daemon_snapshot(config, &daemon_sessions_for_down_sync(config))
}

pub(crate) fn kill_sessions_for_down(config: &Config) -> Result<()> {
    if daemon::server::socket_responds(&config.socket_path) {
        let _ = send_sync(&config.socket_path, &ClientCommand::PrepareRestore);
    }
    daemon::tmux::kill_session_if_exists(&config.tmux_ui_session)?;
    daemon::tmux::kill_session_if_exists(&config.tmux_session)?;
    Ok(())
}

pub fn run_doctor(json: bool, quiet: bool, repair: bool) -> Result<()> {
    let config = Config::default();
    let _ = telemetry::ensure_sessions_config(&config.home);
    telemetry::record_feature(telemetry::FeatureId::CliDoctor, telemetry::feature::Source::Cli);
    let _ = telemetry::heartbeat::maybe_heartbeat(false);
    if repair {
        let report = doctor::run_repair(&config.home)?;
        if !quiet
            && (!report.tombstoned.is_empty()
                || !report.launch_commands_rewritten.is_empty()
                || !report.agent_session_ids_backfilled.is_empty())
        {
            println!("manifest repair:");
            for sessions_session_id in &report.tombstoned {
                println!("  tombstoned {sessions_session_id}");
            }
            for sessions_session_id in &report.launch_commands_rewritten {
                println!("  rewrote launch_command for {sessions_session_id}");
            }
            for sessions_session_id in &report.agent_session_ids_backfilled {
                println!("  backfilled agent_session_id for {sessions_session_id}");
            }
            println!();
        }
    }
    let checks = doctor::install_checks(&config.home);
    let healthy = doctor::all_ok(&checks);
    if json {
        let payload: Vec<_> = checks
            .iter()
            .map(|check| {
                serde_json::json!({
                    "label": check.label,
                    "ok": check.ok,
                    "detail": check.detail,
                    "fix": check.fix,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if !quiet || !healthy {
        if !quiet {
            println!("sessions install health");
        }
        let _ = doctor::print_report(&checks);
        if !quiet {
            println!();
            if healthy {
                println!("Ready. Start with: sessions up");
            } else {
                println!("Install needs attention — fix the items marked FAIL above.");
            }
        }
    }
    if healthy {
        Ok(())
    } else {
        anyhow::bail!("install health check failed")
    }
}

fn ensure_agent_hooks(config: &Config) {
    let summary = hooks::setup_detected(&config.home);
    for (agent, err) in &summary.failed {
        eprintln!("warning: {agent} hook setup failed: {err}");
    }
}

pub fn run_up() -> Result<()> {
    let config = Config::default();
    let _ = telemetry::ensure_sessions_config(&config.home);
    telemetry::record_feature(telemetry::FeatureId::CliUp, telemetry::feature::Source::Cli);
    let _ = telemetry::heartbeat::maybe_heartbeat(false);
    ensure_agent_hooks(&config);
    session::orchestrate_up(&config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{
        append_entry, close_unified, load_manifest, CloseTarget, LaunchSpec, ManifestSource,
    };
    use chrono::Utc;
    use tempfile::TempDir;

    #[test]
    fn confirm_close_tombstones_manifest() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        let session = Session {
            id: "tmux:win:2".into(),
            kitty_window_id: 2,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 2,
            tmux_session: "agents-nonexistent".into(),
            tmux_pane_id: "%2".into(),
            pane_pid: 0,
            agent_session_id: Some("agent-confirm".into()),
            title: "grok · confirm".into(),
            description: "grok".into(),
            cwd: "/tmp/confirm".into(),
            cwd_label: "~/tmp/confirm".into(),
            project: "grok".into(),
            state: crate::model::AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: true,
            last_event_at: Utc::now(),
            managed: true,
            sessions_session_id: Some("ssn_confirm_close".into()),
            managed_agent: Some("grok".into()),
        };
        append_entry(
            &config,
            LaunchSpec {
                sessions_session_id: "ssn_confirm_close".into(),
                source: ManifestSource::WorkspaceBootstrap,
                cwd: "/tmp/confirm".into(),
                agent: "grok".into(),
                launch_command: "grok --resume tombstone".into(),
                workspace_index: Some(1),
                focus: false,
                window_name: None,
                bootstrap_new_session: false,
                model_id: None,
                user_prompt: None,
            }
            .to_manifest_entry(home),
        )
        .unwrap();
        crate::daemon::persist::save_state(&config, &[session], 1).unwrap();

        let outcome = close_unified(
            &config,
            CloseTarget {
                session_id: Some("tmux:win:2".into()),
                sessions_session_id: Some("ssn_confirm_close".into()),
                window_index: Some(2),
            },
        )
        .unwrap();
        assert_eq!(
            outcome.sessions_session_id.as_deref(),
            Some("ssn_confirm_close")
        );

        let manifest = load_manifest(&config).unwrap();
        let entry = manifest
            .entries
            .iter()
            .find(|entry| entry.sessions_session_id == "ssn_confirm_close")
            .expect("manifest entry");
        assert!(entry.closed, "confirm-close path must tombstone manifest entry");
        assert_eq!(entry.launch_command, "grok --resume tombstone");
    }

    #[test]
    fn run_down_ordering_sync_before_kill() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        config.state_path = crate::paths::state_dir(dir.path()).join("sessionsd.json");
        config.socket_path = dir.path().join("offline.sock");
        config.tmux_session = "agents-nonexistent".into();
        config.tmux_ui_session = "sessions-ui-nonexistent".into();

        crate::session::append_entry(
            &config,
            crate::session::ManifestEntry {
                sessions_session_id: "ssn_order".into(),
                source: crate::session::ManifestSource::Cli,
                workspace_index: None,
                cwd: "/tmp/order".into(),
                cwd_label: "/tmp/order".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: None,
                title: None,
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let session = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: config.tmux_session.clone(),
            tmux_pane_id: "%1".into(),
            pane_pid: 0,
            agent_session_id: Some("agent-order".into()),
            title: "grok · order".into(),
            description: "order".into(),
            cwd: "/tmp/order".into(),
            cwd_label: "/tmp/order".into(),
            project: "grok".into(),
            state: crate::model::AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: true,
            last_event_at: Utc::now(),
            managed: true,
            sessions_session_id: Some("ssn_order".into()),
            managed_agent: Some("grok".into()),
        };
        crate::daemon::persist::save_state(&config, &[session], 1).unwrap();

        let mut phases = Vec::new();
        phases.push("sync");
        flush_and_sync_manifest_before_down(&config).unwrap();
        let synced = crate::session::load_manifest(&config)
            .unwrap()
            .entries
            .iter()
            .find(|entry| entry.sessions_session_id == "ssn_order")
            .and_then(|entry| entry.agent_session_id.clone())
            .expect("manifest synced before kill");
        assert_eq!(synced, "agent-order");

        phases.push("kill");
        kill_sessions_for_down(&config).unwrap();
        assert_eq!(phases, ["sync", "kill"]);
        run_down_with_config(&config).unwrap();
    }

    #[test]
    #[ignore = "integration: fresh ./install.sh + sessions up"]
    fn install_fresh_bootstrap_only() {
        // P2: empty manifest → only workspaces.toml windows after sessions up.
    }
}