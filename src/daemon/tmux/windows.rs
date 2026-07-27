use crate::config::Config;
use crate::model::AgentState;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use tracing::{debug, warn};

use super::layout::{
    console_shell_command, push_client_terminal_title, sessions_binary, shell_quote,
    terminal_backdrop_shell, workspace_shell_command, HOST_TERMINAL_TITLE,
};
use super::popups::bind_ui_panel_keys;
use super::subprocess::{resolve_tmux_binary, run_tmux, session_exists};

#[derive(Debug, Clone)]
pub struct TmuxWindow {
    pub index: u32,
    pub name: String,
    pub cwd: String,
    pub current_command: String,
    pub start_command: String,
    pub pane_id: String,
    pub pane_pid: u32,
    pub active: bool,
    pub pane_dead: bool,
    pub pane_dead_status: Option<i32>,
    /// Stable id from tmux `@sessions.id` when the window is managed.
    pub sessions_session_id: Option<String>,
    /// Detached pre-hydrated spare (`@sessions.pool=1`) — not shown in the sidebar.
    pub pool: bool,
}

/// Window title prefix for warm-pool spares (also set `@sessions.pool=1`).
pub const POOL_WINDOW_NAME_PREFIX: &str = "pool · ";

pub fn is_pool_window_name(name: &str) -> bool {
    name.starts_with(POOL_WINDOW_NAME_PREFIX)
}
/// Live stable ids keyed by `@sessions.id` for each tmux window.
pub fn list_live_sessions_session_ids(
    session: &str,
) -> Result<std::collections::HashMap<String, u32>> {
    let output = run_tmux(&[
        "list-windows",
        "-t",
        session,
        "-F",
        "#{window_index}\t#{@sessions.id}",
    ])?;
    let mut live = std::collections::HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let index = parts.next().and_then(|value| value.parse::<u32>().ok());
        let sessions_session_id = parts
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let (Some(index), Some(sessions_session_id)) = (index, sessions_session_id) {
            live.insert(sessions_session_id, index);
        }
    }
    Ok(live)
}

pub fn select_window_by_sessions_session_id(
    session: &str,
    sessions_session_id: &str,
) -> Result<()> {
    let live = list_live_sessions_session_ids(session)?;
    if let Some(index) = live.get(sessions_session_id) {
        select_window(session, *index)?;
        Ok(())
    } else {
        anyhow::bail!(
            "sessions_session_id {sessions_session_id} not found in tmux session {session}"
        )
    }
}

pub fn list_windows(session: &str) -> Result<Vec<TmuxWindow>> {
    let output = run_tmux(&[
        "list-windows",
        "-t",
        session,
        "-F",
        "#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_start_command}\t#{pane_id}\t#{pane_pid}\t#{window_active}\t#{pane_dead}\t#{pane_dead_status}\t#{@sessions.id}\t#{@sessions.pool}",
    ])?;
    if !output.status.success() {
        warn!(
            "tmux list-windows failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(Vec::new());
    }
    let mut windows = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(win) = parse_window_line(line) else {
            continue;
        };
        windows.push(win);
    }
    Ok(windows)
}

pub(crate) fn parse_window_line(line: &str) -> Option<TmuxWindow> {
    let mut parts = line.splitn(12, '\t');
    let index: u32 = parts.next()?.parse().ok()?;
    let name = parts.next()?.to_string();
    let cwd = parts.next()?.to_string();
    let current_command = parts.next()?.to_string();
    let start_command = parts.next()?.to_string();
    let pane_id = parts.next()?.to_string();
    let pane_pid = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let active = parts.next().unwrap_or("0") == "1";
    let pane_dead = parts.next().unwrap_or("0") == "1";
    let pane_dead_status = parts.next().and_then(|s| s.parse().ok());
    let sessions_session_id = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let pool_raw = parts.next().unwrap_or("").trim();
    let pool = pool_raw == "1" || is_pool_window_name(&name);
    Some(TmuxWindow {
        index,
        name,
        cwd,
        current_command,
        start_command,
        pane_id,
        pane_pid,
        active,
        pane_dead,
        pane_dead_status,
        sessions_session_id,
        pool,
    })
}
/// Prefer the shell process cwd over tmux's `pane_current_path`, which can lag after `cd`.
pub fn effective_pane_cwd(tmux_cwd: &str, pane_pid: u32) -> String {
    crate::process::cwd_for_pane_pid(tmux_cwd, pane_pid).unwrap_or_else(|| tmux_cwd.to_string())
}

pub fn pane_effective_cwd(session: &str, pane_id: &str) -> Result<String> {
    let output = run_tmux(&[
        "display-message",
        "-p",
        "-t",
        &format!("{session}:{pane_id}"),
        "-F",
        "#{pane_current_path}\t#{pane_pid}",
    ])?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut parts = line.splitn(2, '\t');
    let tmux_cwd = parts.next().unwrap_or("").to_string();
    let pane_pid = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok(effective_pane_cwd(&tmux_cwd, pane_pid))
}
pub(crate) fn read_pane_state(state_dir: &Path, pane_id: &str) -> AgentState {
    let path = state_dir.join(format!("pane-{pane_id}.state"));
    std::fs::read_to_string(path)
        .ok()
        .map(|s| AgentState::from_str(s.trim()))
        .unwrap_or(AgentState::Idle)
}

pub fn save_pane_state(state_dir: &Path, pane_id: &str, state: AgentState) -> Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(format!("pane-{pane_id}.state"));
    let payload = state.as_str();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if existing.trim() == payload {
            return Ok(());
        }
    }
    std::fs::write(path, payload)?;
    Ok(())
}
pub fn select_window(session: &str, index: u32) -> Result<()> {
    let target = format!("{session}:{index}");
    run_tmux(&["select-window", "-t", &target])?;
    Ok(())
}

pub fn active_window_details(session: &str) -> Result<(u32, String)> {
    let output = run_tmux(&[
        "display-message",
        "-p",
        "-t",
        session,
        "#{window_index}\t#{pane_current_path}\t#{pane_pid}",
    ])?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut parts = line.splitn(3, '\t');
    let index = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .with_context(|| format!("parse active tmux window index from {:?}", line))?;
    let tmux_cwd = parts.next().unwrap_or("").to_string();
    let pane_pid = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok((index, effective_pane_cwd(&tmux_cwd, pane_pid)))
}

fn window_count(session: &str) -> Result<usize> {
    Ok(list_windows(session)?.len())
}

pub fn create_window(config: &Config) -> Result<u32> {
    create_console_window(config)
}

/// New raw terminal (⌘T) — always starts at ~, not the active pane's cwd.
pub fn create_console_window(config: &Config) -> Result<u32> {
    let home = crate::paths::home();
    create_window_in_cwd(config, &home.display().to_string())
}

pub fn create_window_in_cwd(config: &Config, cwd: &str) -> Result<u32> {
    crate::session::create_console(config, cwd, crate::session::ManifestSource::Cli, true)
        .map(|created| created.index)
}

/// Like create_window_in_cwd but honors the `focus` flag (for background launches from new-session pane).
pub fn create_terminal_window_in_cwd(config: &Config, cwd: &str, focus: bool) -> Result<u32> {
    crate::session::create_console(config, cwd, crate::session::ManifestSource::Cli, focus)
        .map(|created| created.index)
}

fn create_window_in_cwd_with_command(
    config: &Config,
    cwd: &str,
    command: Option<&str>,
) -> Result<u32> {
    create_window_in_cwd_with_command_focus(config, cwd, command, "session", true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedWindow {
    pub index: u32,
    pub pane_id: String,
    pub sessions_session_id: String,
}

pub(crate) fn create_managed_window_in_cwd(
    session: &str,
    cwd: &str,
    managed_agent: Option<&str>,
    command: Option<&str>,
    window_name: &str,
    focus: bool,
    sessions_session_id: &str,
    bootstrap_new_session: bool,
) -> Result<CreatedWindow> {
    create_managed_window_in_cwd_with_pool(
        session,
        cwd,
        managed_agent,
        command,
        window_name,
        focus,
        sessions_session_id,
        bootstrap_new_session,
        false,
    )
}

pub(crate) fn create_managed_window_in_cwd_with_pool(
    session: &str,
    cwd: &str,
    managed_agent: Option<&str>,
    command: Option<&str>,
    window_name: &str,
    focus: bool,
    sessions_session_id: &str,
    bootstrap_new_session: bool,
    pool: bool,
) -> Result<CreatedWindow> {
    let agent = managed_agent.unwrap_or("console");
    let shell = command.map(|cmd| {
        let wrapped =
            crate::session::wrap_managed_launch_command(agent, cwd, sessions_session_id, cmd);
        workspace_shell_command(&wrapped)
    });
    let console_shell = console_shell_command();
    // Pool spares are always detached — never steal focus while hydrating.
    let focus = focus && !pool;
    let mut args: Vec<&str> = if bootstrap_new_session {
        vec!["new-session", "-d", "-s", session]
    } else {
        vec!["new-window"]
    };
    if !focus && !bootstrap_new_session {
        args.push("-d");
    }
    args.extend_from_slice(&["-P", "-F", "#{window_index}\t#{pane_id}"]);
    if !bootstrap_new_session {
        args.push("-t");
        args.push(session);
    }
    args.extend_from_slice(&["-n", window_name, "-c", cwd, "--", "/bin/zsh"]);
    if let Some(ref shell) = shell {
        args.push("-lc");
        args.push(shell);
    } else {
        args.push("-lc");
        args.push(&console_shell);
    }
    let output = run_tmux(&args)?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut parts = line.splitn(2, '\t');
    let index = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .with_context(|| format!("parse new tmux window index from {:?}", line))?;
    let pane_id = parts.next().unwrap_or("").to_string();
    let _ = set_managed_window_options(session, index, sessions_session_id, agent, pool);
    Ok(CreatedWindow {
        index,
        pane_id,
        sessions_session_id: sessions_session_id.to_string(),
    })
}

fn set_managed_window_options(
    session: &str,
    window_index: u32,
    sessions_session_id: &str,
    agent: &str,
    pool: bool,
) -> Result<()> {
    let target = format!("{session}:{window_index}");
    let _ = run_tmux(&[
        "set-window-option",
        "-t",
        &target,
        "@sessions.id",
        sessions_session_id,
    ]);
    let _ = run_tmux(&["set-window-option", "-t", &target, "@sessions.agent", agent]);
    let _ = run_tmux(&["set-window-option", "-t", &target, "@sessions.managed", "1"]);
    if pool {
        let _ = run_tmux(&["set-window-option", "-t", &target, "@sessions.pool", "1"]);
    } else {
        let _ = run_tmux(&["set-window-option", "-u", "-t", &target, "@sessions.pool"]);
    }
    Ok(())
}

/// Clear pool flag and rename after a warm window is claimed for the user.
pub fn claim_pool_window(
    session: &str,
    window_index: u32,
    sessions_session_id: &str,
    agent: &str,
    window_name: &str,
    focus: bool,
) -> Result<()> {
    set_managed_window_options(session, window_index, sessions_session_id, agent, false)?;
    rename_window(session, window_index, window_name)?;
    if focus {
        select_window(session, window_index)?;
    }
    Ok(())
}

fn create_window_in_cwd_with_command_focus(
    config: &Config,
    cwd: &str,
    command: Option<&str>,
    window_name: &str,
    focus: bool,
) -> Result<u32> {
    let launch_command = command.unwrap_or("");
    let mut spec = crate::session::launch_spec_for_agent(
        cwd.to_string(),
        "session",
        Some(launch_command.to_string()),
        crate::session::ManifestSource::Cli,
        focus,
    );
    if !window_name.is_empty() {
        spec.window_name = Some(window_name.to_string());
    }
    crate::session::create_unified(config, spec).map(|created| created.index)
}

/// Active workspace window index, tmux window name, and cwd.
pub fn active_window_summary(session: &str) -> Result<(u32, String, String)> {
    let output = run_tmux(&[
        "display-message",
        "-p",
        "-t",
        session,
        "#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_pid}",
    ])?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut parts = line.splitn(4, '\t');
    let index = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .with_context(|| format!("parse active tmux window index from {:?}", line))?;
    let name = parts.next().unwrap_or("session").to_string();
    let tmux_cwd = parts.next().unwrap_or("").to_string();
    let pane_pid = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok((index, name, effective_pane_cwd(&tmux_cwd, pane_pid)))
}

pub fn close_active_window(session: &str) -> Result<()> {
    if window_count(session)? <= 1 {
        anyhow::bail!("refusing to close the last remaining session window");
    }
    let (index, _) = active_window_details(session)?;
    let target = format!("{session}:{index}");
    run_tmux(&["kill-window", "-t", &target])?;
    Ok(())
}

pub fn confirm_close_active_window(session: &str) -> Result<()> {
    if window_count(session)? <= 1 {
        anyhow::bail!("refusing to close the last remaining session window");
    }
    let bin = shell_quote(&sessions_binary().display().to_string());
    let script = format!("{bin} close </dev/null >/dev/null 2>&1");
    let command = format!("run-shell -b \"{script}\"");
    run_tmux(&[
        "confirm-before",
        "-t",
        session,
        "-p",
        "Close current session? (Enter = yes, n = no)",
        &command,
    ])?;
    Ok(())
}

pub fn confirm_close_window(session: &str, window_index: u32) -> Result<()> {
    if window_count(session)? <= 1 {
        anyhow::bail!("refusing to close the last remaining session window");
    }
    let target = format!("{session}:{window_index}");
    let command = format!("kill-window -t {target}");
    run_tmux(&[
        "confirm-before",
        "-t",
        session,
        "-p",
        &format!("Close session {window_index}? (Enter = yes, n = no)"),
        &command,
    ])?;
    Ok(())
}

pub fn close_window(session: &str, window_index: u32) -> Result<()> {
    if window_count(session)? <= 1 {
        anyhow::bail!("refusing to close the last remaining session window");
    }
    if !list_windows(session)?
        .iter()
        .any(|window| window.index == window_index)
    {
        return Ok(());
    }
    let target = format!("{session}:{window_index}");
    run_tmux(&["kill-window", "-t", &target])?;
    Ok(())
}

pub fn kill_session_if_exists(session: &str) -> Result<()> {
    if session_exists(session) {
        let _ = run_tmux(&["detach-client", "-s", session]);
        run_tmux(&["kill-session", "-t", session])?;
    }
    Ok(())
}

pub fn detach_current_client() -> Result<()> {
    let _ = push_client_terminal_title();
    run_tmux(&["detach-client"])?;
    Ok(())
}

/// One row from `tmux list-clients`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxClient {
    pub tty: String,
    pub session: String,
    pub pid: u32,
    pub term: String,
}

/// Parse `list-clients -F '#{client_tty}\t#{client_session}\t#{client_pid}\t#{client_termname}'`.
pub fn parse_tmux_client_line(line: &str) -> Option<TmuxClient> {
    let mut parts = line.splitn(4, '\t');
    let tty = parts.next()?.trim();
    let session = parts.next()?.trim();
    if tty.is_empty() || session.is_empty() {
        return None;
    }
    let pid = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let term = parts.next().unwrap_or("").trim().to_string();
    Some(TmuxClient {
        tty: tty.to_string(),
        session: session.to_string(),
        pid,
        term,
    })
}

pub fn list_tmux_clients() -> Result<Vec<TmuxClient>> {
    let output = run_tmux(&[
        "list-clients",
        "-F",
        "#{client_tty}\t#{client_session}\t#{client_pid}\t#{client_termname}",
    ])?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_tmux_client_line)
        .collect())
}

/// TTY of the sessions-ui workspace pane (index 1) — this is the *intended*
/// nested `tmux attach -t agents` client, not a stray host attachment.
pub fn ui_workspace_pane_tty(ui_session: &str) -> Option<String> {
    if !session_exists(ui_session) {
        return None;
    }
    let target = format!("{ui_session}:ui.1");
    let output = run_tmux(&["display-message", "-p", "-t", &target, "#{pane_tty}"]).ok()?;
    let tty = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if tty.is_empty() {
        None
    } else {
        Some(tty)
    }
}

/// Clients attached to `agents` that are **not** the nested workspace attach
/// inside `sessions-ui` (matched by pane TTY).
///
/// Direct `tmux attach -t agents` from a host terminal shows up here and
/// steals focus/active-window from the sidebar layout.
pub fn stray_agents_clients(ui_session: &str, agents_session: &str) -> Vec<TmuxClient> {
    let nested_tty = ui_workspace_pane_tty(ui_session);
    list_tmux_clients()
        .unwrap_or_default()
        .into_iter()
        .filter(|client| client.session == agents_session)
        .filter(|client| match nested_tty.as_deref() {
            // UI not up: every agents client is "bare" (stray for our architecture).
            None => true,
            Some(workspace_tty) => client.tty != workspace_tty,
        })
        .collect()
}

/// Pure filter used by tests — same rules as [`stray_agents_clients`].
pub fn filter_stray_agents_clients(
    clients: &[TmuxClient],
    agents_session: &str,
    nested_workspace_tty: Option<&str>,
) -> Vec<TmuxClient> {
    clients
        .iter()
        .filter(|client| client.session == agents_session)
        .filter(|client| match nested_workspace_tty {
            None => true,
            Some(workspace_tty) => client.tty != workspace_tty,
        })
        .cloned()
        .collect()
}

/// Detach host-level (or otherwise non-nested) clients on the agents session.
///
/// Returns the TTYs that were detached. Never detaches the nested workspace
/// client whose TTY matches `sessions-ui:ui.1`.
pub fn detach_stray_agents_clients(ui_session: &str, agents_session: &str) -> Result<Vec<String>> {
    let strays = stray_agents_clients(ui_session, agents_session);
    let mut detached = Vec::new();
    for client in strays {
        match run_tmux(&["detach-client", "-t", &client.tty]) {
            Ok(_) => {
                debug!(
                    "detached stray agents client tty={} pid={} term={}",
                    client.tty, client.pid, client.term
                );
                detached.push(client.tty);
            }
            Err(err) => {
                warn!("failed to detach stray agents client {}: {err}", client.tty);
            }
        }
    }
    Ok(detached)
}

pub fn rename_window(session: &str, index: u32, name: &str) -> Result<()> {
    let target = format!("{session}:{index}");
    rename_window_target(&target, name)
}

fn rename_window_named(session: &str, window_name: &str, title: &str) -> Result<()> {
    let target = format!("{session}:{window_name}");
    rename_window_target(&target, title)
}

fn rename_window_target(target: &str, name: &str) -> Result<()> {
    if let Err(e) = run_tmux(&["rename-window", "-t", target, name]) {
        debug!("tmux rename-window failed for {target}: {e}");
    }
    Ok(())
}

pub fn send_bell(session: &str, index: u32) -> Result<()> {
    let target = format!("{session}:{index}");
    let output = run_tmux(&["send-keys", "-t", &target, "\u{07}"])?;
    if !output.status.success() {
        debug!(
            "tmux bell failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub fn send_keys_to_active_window(session: &str, keys: &[&str]) -> Result<()> {
    let (index, _) = active_window_details(session)?;
    send_keys_to_window(session, index, keys)
}

pub fn send_keys_to_window(session: &str, window_index: u32, keys: &[&str]) -> Result<()> {
    let target = format!("{session}:{window_index}");
    let mut args = vec!["send-keys", "-t", &target.as_str()];
    args.extend(keys.iter().copied());
    run_tmux(&args)?;
    Ok(())
}

/// tmux truncates each `send-keys -l` payload to 1024 bytes.
pub const TMUX_SEND_KEYS_MAX_LITERAL: usize = 1024;

pub(crate) fn literal_send_chunks(text: &str) -> impl Iterator<Item = &str> {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= text.len() {
            return None;
        }
        let mut end = start
            .saturating_add(TMUX_SEND_KEYS_MAX_LITERAL)
            .min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = text[start..]
                .char_indices()
                .nth(1)
                .map(|(idx, _)| start + idx)
                .unwrap_or(text.len());
        }
        let chunk = &text[start..end];
        start = end;
        Some(chunk)
    })
}

pub fn send_literal_to_window(session: &str, window_index: u32, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let target = format!("{session}:{window_index}");
    for chunk in literal_send_chunks(text) {
        run_tmux(&["send-keys", "-l", "-t", &target, chunk])?;
    }
    Ok(())
}

/// Read the stable `@sessions.id` tmux option for a window.
pub fn window_sessions_session_id(session: &str, window_index: u32) -> Option<String> {
    let output = run_tmux(&[
        "display-message",
        "-p",
        "-t",
        &format!("{session}:{window_index}"),
        "-F",
        "#{@sessions.id}",
    ])
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

pub fn pane_sessions_session_id(session: &str, pane_id: &str) -> Option<String> {
    let window_index = pane_to_window_index(session, pane_id)?;
    window_sessions_session_id(session, window_index)
}

pub fn pane_to_window_index(session: &str, pane_id: &str) -> Option<u32> {
    let output = run_tmux(&[
        "list-panes",
        "-t",
        session,
        "-F",
        "#{pane_id}\t#{window_index}",
    ])
    .ok()?;
    if !output.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let pid = parts.next()?;
        if pid == pane_id {
            return parts.next()?.parse().ok();
        }
    }
    None
}

pub fn window_to_pane_id(session: &str, window_index: u32) -> Option<String> {
    let output = run_tmux(&[
        "list-panes",
        "-t",
        &format!("{session}:{window_index}"),
        "-F",
        "#{pane_id}",
    ])
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let pane_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!pane_id.is_empty()).then_some(pane_id)
}

pub fn bootstrap_session(config: &Config) -> Result<()> {
    crate::session::bootstrap_workspaces(config)
}

pub(crate) fn instant_key_bind_script(sessions_bin: &str, agent: &str) -> String {
    let refresh = format!("{sessions_bin} refresh </dev/null >/dev/null 2>&1");
    format!(
        "{sessions_bin} create-instant {agent} </dev/null >/dev/null 2>&1 \\; run-shell -b {refresh}"
    )
}

pub(crate) fn bind_instant_session_keys(session: &str, sessions_bin: &str) -> Result<()> {
    let bindings: &[(&str, &str)] = &[
        ("M-t", "console"),
        ("M-T", "console"),
        ("M-g", "grok"),
        ("M-G", "grok"),
        ("M-c", "codex"),
        ("M-C", "codex"),
        ("M-o", "opencode"),
        ("M-O", "opencode"),
    ];
    for (key, agent) in bindings {
        let script = instant_key_bind_script(sessions_bin, agent);
        if let Err(e) = run_tmux(&["bind-key", "-n", key, "run-shell", "-b", &script]) {
            debug!("tmux bind instant {key}: {e}");
        }
    }
    let _ = session;
    Ok(())
}

pub(crate) fn bind_workspace_keys(session: &str, sessions_bin: &Path) -> Result<()> {
    let bin = shell_quote(&sessions_bin.display().to_string());
    // Prefix 1–9/0 and Meta 1–9/0 → ordered sidebar focus (`sessions focus N`).
    // Ghostty maps ⌘1–⌘0 to Meta digits via bin/setup-ghostty.sh so these fire
    // while a workspace agent pane has keyboard focus.
    // run-shell -b: do not block the tmux client while the CLI runs (matches
    // click snappiness; window switch happens inside `sessions focus`).
    for ordinal in 1..=10 {
        let key = if ordinal == 10 {
            "0".to_string()
        } else {
            ordinal.to_string()
        };
        let script = format!("{bin} focus {ordinal} </dev/null >/dev/null 2>&1");
        let bind = key.clone();
        if let Err(e) = run_tmux(&["bind-key", &bind, "run-shell", "-b", &script]) {
            debug!("tmux bind prefix {bind}: {e}");
        }
        let meta_bind = format!("M-{key}");
        if let Err(e) = run_tmux(&["bind-key", "-n", &meta_bind, "run-shell", "-b", &script]) {
            debug!("tmux bind {meta_bind}: {e}");
        }
    }
    // Meta+Shift+[ / ] (M-{ / M-}) → resize sidebar. Plain [ / ] are handled in
    // the bar when the sidebar has focus; these work from the workspace pane.
    // Avoid M-[ / M-] — ESC+[ is the CSI prefix and collides with arrow keys.
    for (key, direction) in [("M-{", "narrower"), ("M-}", "wider")] {
        let script = format!("{bin} resize-sidebar {direction} </dev/null >/dev/null 2>&1");
        if let Err(e) = run_tmux(&["bind-key", "-n", key, "run-shell", "-b", &script]) {
            debug!("tmux bind {key}: {e}");
        }
    }
    for (bind, cmd) in [("q", "confirm-close"), ("b", "leave"), ("m", "__mouse__")] {
        if cmd == "__mouse__" {
            continue;
        }
        let script = format!("{bin} {cmd} </dev/null >/dev/null 2>&1");
        if let Err(e) = run_tmux(&["bind-key", bind, "run-shell", &script]) {
            debug!("tmux bind prefix {bind}: {e}");
        }
    }
    let _ = run_tmux(&["unbind-key", "-T", "prefix", "n"]);
    // Keep M-n/M-N — panel open-new-session (⌘+N). Do not bind S-n/S-N (Shift+N).
    for old_bind in [
        "M-k", "M-K", "S-k", "S-K", "S-t", "S-T", "S-g", "S-G", "S-c", "S-C", "S-o", "S-O", "S-n",
        "S-N",
    ] {
        let _ = run_tmux(&["unbind-key", "-n", old_bind]);
    }
    bind_instant_session_keys(session, &bin)?;
    for (bind, cmd) in [
        ("M-q", "confirm-close"),
        ("M-b", "leave"),
        ("M-Escape", "leave"),
    ] {
        let script = format!("{bin} {cmd} </dev/null >/dev/null 2>&1");
        if let Err(e) = run_tmux(&["bind-key", "-n", bind, "run-shell", &script]) {
            debug!("tmux bind {bind}: {e}");
        }
    }
    let toggle_mouse = format!(
        "if-shell -F '#{{mouse}}' 'set-option -t {session} mouse off' 'set-option -t {session} mouse on'"
    );
    if let Err(e) = run_tmux(&["bind-key", "m", &toggle_mouse]) {
        debug!("tmux bind prefix m: {e}");
    }
    if let Err(e) = run_tmux(&["bind-key", "-n", "M-m", &toggle_mouse]) {
        debug!("tmux bind M-m: {e}");
    }
    let config = Config::default();
    bind_ui_panel_keys(&config.tmux_ui_session, session, sessions_bin)?;
    Ok(())
}

pub(crate) fn configure_prefix(session: &str) -> Result<()> {
    let _ = run_tmux(&["set-option", "-t", session, "prefix", "C-g"]);
    let _ = run_tmux(&["unbind-key", "-t", session, "C-b"]);
    let _ = run_tmux(&[
        "bind-key",
        "-T",
        "prefix",
        "-t",
        session,
        "C-g",
        "send-prefix",
    ]);
    let _ = run_tmux(&["set-option", "-t", session, "status", "off"]);
    Ok(())
}

pub(crate) fn configure_terminal_capabilities(session: &str) {
    for (opt, val) in [
        ("default-terminal", "tmux-256color"),
        // Kitty/iTerm/Sixel graphics (ratatui-image) wrap sequences in tmux passthrough.
        ("allow-passthrough", "on"),
    ] {
        if let Err(e) = run_tmux(&["set-option", "-t", session, opt, val]) {
            debug!("tmux set-option {opt} for {session}: {e}");
        }
    }
    // Append features (plain set-option overwrites; only the last entry would stick).
    // Leading comma matches tmux's terminal-features append style.
    for feature in [
        ",xterm-kitty:RGB",
        ",xterm-ghostty:extkeys",
        ",ghostty:extkeys",
        // Cursor / VS Code integrated terminal (xterm.js reports xterm-256color).
        ",xterm-256color:RGB",
        ",xterm*:RGB",
        // Nested workspace client is `tmux attach` with TERM=tmux-256color. Without
        // clipboard here, Grok's OSC 52 (native select→copy) is accepted into the
        // agents paste-buffer but never re-emitted through sessions-ui to the host.
        ",tmux*:clipboard",
        ",tmux-256color:clipboard",
    ] {
        if let Err(e) = run_tmux(&[
            "set-option",
            "-as",
            "-t",
            session,
            "terminal-features",
            feature,
        ]) {
            debug!("tmux terminal-features {feature} for {session}: {e}");
        }
        // Also set globally — nested-client feature matching uses the server table.
        if let Err(e) = run_tmux(&["set-option", "-gas", "terminal-features", feature]) {
            debug!("tmux terminal-features -g {feature}: {e}");
        }
    }
    for override_value in [",xterm-kitty:RGB", ",xterm-256color:RGB", ",*:RGB"] {
        if let Err(e) = run_tmux(&[
            "set-option",
            "-as",
            "-t",
            session,
            "terminal-overrides",
            override_value,
        ]) {
            debug!("tmux terminal-overrides {override_value} for {session}: {e}");
        }
    }
    // Agent/CI shells often export NO_COLOR=1. When `sessions up` starts the
    // tmux server from that env, every pane inherits monochrome forever.
    scrub_tmux_monochrome_env();
}

/// Clear global tmux env kill-switches so new panes get color.
fn scrub_tmux_monochrome_env() {
    for key in crate::color_env::TMUX_UNSET_KEYS {
        let _ = run_tmux(&["set-environment", "-gu", key]);
    }
    for (key, val) in crate::color_env::TMUX_SET_PAIRS {
        let _ = run_tmux(&["set-environment", "-g", key, val]);
    }
}

/// Clipboard + paste options so tmux stays out of the way unless mouse mode is toggled on.
pub(crate) fn configure_terminal_clipboard(session: &str) -> Result<()> {
    configure_terminal_capabilities(session);
    for (opt, val) in [
        ("focus-events", "on"),
        ("set-clipboard", "on"),
        ("extended-keys", "on"),
    ] {
        if let Err(e) = run_tmux(&["set-option", "-t", session, opt, val]) {
            debug!("tmux set-option {opt} for {session}: {e}");
        }
    }
    configure_system_clipboard_bindings()?;
    Ok(())
}

/// Route tmux copy/paste through the OS clipboard (pbcopy/pbpaste, xclip, wl-copy).
pub(crate) fn configure_system_clipboard_bindings() -> Result<()> {
    static DONE: std::sync::Once = std::sync::Once::new();
    let mut err = Ok(());
    DONE.call_once(|| {
        err = configure_system_clipboard_bindings_inner();
    });
    // Always reinstall — Once would skip the OSC 52→OS bridge after first configure
    // in a long-lived process (daemon reconfigure without restart).
    install_osc52_os_clipboard_hook();
    err
}

pub(crate) fn configure_system_clipboard_bindings_inner() -> Result<()> {
    let _ = run_tmux(&["set-option", "-g", "extended-keys", "on"]);
    let Some(pipe) = clipboard_copy_pipe_command() else {
        install_osc52_os_clipboard_hook();
        return Ok(());
    };

    for (table, key) in [
        ("copy-mode-vi", "y"),
        ("copy-mode-vi", "Enter"),
        ("copy-mode-vi", "C-j"),
        ("copy-mode-vi", "MouseDragEnd1Pane"),
        ("copy-mode-vi", "MouseDragEnd2Pane"),
        ("copy-mode", "M-w"),
        ("copy-mode", "C-w"),
        ("copy-mode", "MouseDragEnd1Pane"),
        ("copy-mode", "MouseDragEnd2Pane"),
    ] {
        let cmd = format!("send-keys -X copy-pipe-and-cancel {pipe}");
        bind_key_or_debug(table, key, &cmd);
    }

    for (table, select) in [
        ("copy-mode-vi", "select-word"),
        ("copy-mode-vi", "select-line"),
        ("copy-mode", "select-word"),
        ("copy-mode", "select-line"),
    ] {
        let key = if select == "select-word" {
            "DoubleClick1Pane"
        } else {
            "TripleClick1Pane"
        };
        let cmd = format!(
            "send-keys -X {select} \\; run-shell -d 0.3 \\; send-keys -X copy-pipe-and-cancel {pipe}"
        );
        bind_key_or_debug(table, key, &cmd);
    }

    // Multi-arg if-shell (like panel keys) — never nest bash -lc quotes inside one
    // bind-key string; that used to make C-v fail with "syntax error" at bind time.
    //
    // Paste path: load OS clipboard via sessions, then `paste-buffer -p` as a *tmux*
    // command in the same key binding so it targets the pane that received the key.
    // A lone external `tmux paste-buffer` pastes into the wrong client when both
    // sessions-ui and agents are attached.
    let paste_cmd = clipboard_paste_key_command();
    let tui = is_sessions_tui_pane_format();
    let pass_c_v = paste_pass_through_key("C-v");
    let pass_m_v = paste_pass_through_key("M-v");
    bind_if_shell_or_debug("C-v", tui, &pass_c_v, &paste_cmd);
    // M-v is Meta/Option; on macOS hosts in this project Meta is often Command, so
    // ⌘V reaches here for agent panes (right) and is passed through for the bar.
    bind_if_shell_or_debug("M-v", tui, &pass_m_v, &paste_cmd);
    // Middle-click: select the clicked pane first (same as tmux defaults), then paste.
    let middle = format!("select-pane -t = \\; {paste_cmd}");
    bind_if_shell_or_debug(
        "MouseDown2Pane",
        "#{||:#{pane_in_mode},#{mouse_any_flag}}",
        "send-keys -M",
        &middle,
    );
    install_osc52_os_clipboard_hook();
    Ok(())
}

/// Bridge app OSC 52 (Grok native select→copy) to the OS clipboard.
///
/// Nested `sessions-ui → tmux attach → agents` does not re-emit OSC 52 to the host
/// reliably: `set-clipboard on` stores a paste-buffer, then the chain stops.
/// `pane-set-clipboard` fires when the buffer is set from OSC 52 — pipe that to
/// pbcopy/xclip/wl-copy so Grok/OpenCode copy works without tmux copy-mode.
pub(crate) fn install_osc52_os_clipboard_hook() {
    let Some(cmd) = osc52_os_clipboard_hook_command() else {
        return;
    };
    if let Err(e) = run_tmux(&["set-hook", "-g", "pane-set-clipboard", &cmd]) {
        debug!("tmux set-hook pane-set-clipboard: {e}");
    }
}

/// `run-shell 'tmux save-buffer - | pbcopy'` (or xclip/wl-copy).
pub(crate) fn osc52_os_clipboard_hook_command() -> Option<String> {
    let copy = if Path::new("/usr/bin/pbcopy").is_file() {
        "pbcopy"
    } else if Path::new("/usr/bin/xclip").is_file() {
        "xclip -selection clipboard"
    } else if Path::new("/usr/bin/wl-copy").is_file() {
        "wl-copy"
    } else {
        return None;
    };
    // Silent: apps already show their own “Copied” UI; avoid double toast.
    Some(format!("run-shell 'tmux save-buffer - | {copy}'"))
}

fn bind_key_or_debug(table: &str, key: &str, command: &str) {
    if let Err(e) = run_tmux(&["bind-key", "-T", table, key, command]) {
        debug!("tmux bind-key -T {table} {key}: {e}");
    }
}

/// `bind-key -T root KEY if-shell -F COND TRUE FALSE` with separate argv (quote-safe).
fn bind_if_shell_or_debug(key: &str, cond: &str, true_cmd: &str, false_cmd: &str) {
    if let Err(e) = run_tmux(&[
        "bind-key", "-T", "root", key, "if-shell", "-F", cond, true_cmd, false_cmd,
    ]) {
        debug!("tmux bind-key -T root {key} if-shell: {e}");
    }
}

pub(crate) fn clipboard_copy_pipe_command() -> Option<String> {
    crate::clipboard::copy_pipe_command()
}

/// Full key-binding paste: load OS clipboard, then paste-buffer in key pane context.
pub(crate) fn clipboard_paste_key_command() -> String {
    // `\;` keeps both commands in the if-shell false branch (tmux multi-command).
    format!(
        "run-shell \"{}\" \\; paste-buffer -p",
        clipboard_paste_load_shell_command()
    )
}

/// `run-shell "…paste-tmux -t #{pane_id}…"` — subprocess paste with explicit target.
pub(crate) fn clipboard_paste_run_shell_command() -> String {
    format!("run-shell \"{}\"", clipboard_paste_shell_command())
}

pub(crate) fn clipboard_paste_load_shell_command() -> String {
    crate::clipboard::tmux_paste_load_shell_command(sessions_binary().as_path())
}

pub(crate) fn clipboard_paste_shell_command() -> String {
    crate::clipboard::tmux_paste_binding_command(sessions_binary().as_path())
}

/// Root-table paste binding for tests (`if-shell` true/false as one display string).
#[cfg(test)]
pub(crate) fn clipboard_paste_root_binding(key: &str) -> String {
    let pass = paste_pass_through_key(key);
    let tui = is_sessions_tui_pane_format();
    let paste = clipboard_paste_key_command();
    format!("if-shell -F '{tui}' '{pass}' '{paste}'")
}

pub(crate) fn is_sessions_tui_pane_format() -> &'static str {
    "#{==:#{pane_current_command},sessions}"
}

pub(crate) fn paste_pass_through_key(key: &str) -> String {
    format!("send-keys {key}")
}

pub fn write_session_env_tmux(
    config: &crate::config::Config,
    agent_session_id: &str,
    tmux_pane_id: Option<&str>,
    window_index: Option<u32>,
    tmux_session: &str,
    sessions_session_id: Option<&str>,
    managed_agent: Option<&str>,
) -> Result<()> {
    std::fs::create_dir_all(config.grok_state_dir())?;
    let path = config.session_env_path(agent_session_id);
    let mut lines = Vec::new();
    if let Some(pane) = tmux_pane_id {
        lines.push(format!("TMUX_PANE={pane}"));
        lines.push(format!("TMUX_PANE_ID={pane}"));
    }
    if let Some(idx) = window_index {
        lines.push(format!("SESSIONS_WINDOW_INDEX={idx}"));
    }
    lines.push(format!("TMUX_SESSION={tmux_session}"));
    if let Some(ssn) = sessions_session_id {
        lines.push(format!("SESSIONS_SESSION_ID={ssn}"));
    }
    if let Some(agent) = managed_agent {
        lines.push(format!("SESSIONS_AGENT={agent}"));
    }
    let payload = lines.join("\n") + "\n";
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if existing == payload {
            return Ok(());
        }
    }
    std::fs::write(path, payload)?;
    Ok(())
}

pub fn attach_session(session: &str) -> Result<()> {
    nested_attach_session(session)
}

/// Attach inside a tmux pane (sidebar layout). Without clearing TMUX, attach hijacks the whole client.
pub fn nested_attach_session(session: &str) -> Result<()> {
    let tmux = resolve_tmux_binary()?;
    let status = Command::new(&tmux)
        .env_remove("TMUX")
        .args(["attach-session", "-t", session])
        .status()
        .with_context(|| format!("{} attach-session -t {session}", tmux.display()))?;
    if !status.success() {
        anyhow::bail!("tmux attach exited with {}", status);
    }
    Ok(())
}

pub fn nested_attach_shell_command(agents_session: &str) -> Result<String> {
    let tmux = resolve_tmux_binary()?;
    // exec -a sessions so Cursor/VS Code process-title tabs say "sessions"
    // instead of "tmux" for the nested agents client (same as outer attach).
    Ok(format!(
        "{}; exec -a {} env -u TMUX {} attach-session -t {}",
        terminal_backdrop_shell(),
        shell_quote(HOST_TERMINAL_TITLE),
        shell_quote(&tmux.display().to_string()),
        shell_quote(agents_session)
    ))
}
