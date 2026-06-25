use crate::config::Config;
use crate::model::AgentState;
use anyhow::{Context, Result};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, warn};

use super::layout::{
    console_shell_command, push_client_terminal_title, shell_quote, sessions_binary,
    terminal_backdrop_shell, workspace_shell_command,
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
}
/// Live stable ids keyed by `@sessions.id` for each tmux window.
pub fn list_live_sessions_session_ids(session: &str) -> Result<std::collections::HashMap<String, u32>> {
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

pub fn select_window_by_sessions_session_id(session: &str, sessions_session_id: &str) -> Result<()> {
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
        "#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_start_command}\t#{pane_id}\t#{pane_pid}\t#{window_active}\t#{pane_dead}\t#{pane_dead_status}\t#{@sessions.id}",
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
    let mut parts = line.splitn(11, '\t');
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
    let agent = managed_agent.unwrap_or("console");
    let shell = command.map(|cmd| {
        let wrapped =
            crate::session::wrap_managed_launch_command(agent, cwd, sessions_session_id, cmd);
        workspace_shell_command(&wrapped)
    });
    let console_shell = console_shell_command();
    let mut args: Vec<&str> = if bootstrap_new_session {
        vec!["new-session", "-d", "-s", session]
    } else {
        vec!["new-window"]
    };
    if !focus && !bootstrap_new_session {
        args.push("-d");
    }
    args.extend_from_slice(&[
        "-P",
        "-F",
        "#{window_index}\t#{pane_id}",
    ]);
    if !bootstrap_new_session {
        args.push("-t");
        args.push(session);
    }
    args.extend_from_slice(&[
        "-n",
        window_name,
        "-c",
        cwd,
        "--",
        "/bin/zsh",
    ]);
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
    let _ = set_managed_window_options(session, index, sessions_session_id, agent);
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
        let mut end = start.saturating_add(TMUX_SEND_KEYS_MAX_LITERAL).min(text.len());
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
    for ordinal in 1..=10 {
        let key = if ordinal == 10 {
            "0".to_string()
        } else {
            ordinal.to_string()
        };
        let script = format!("{bin} focus {ordinal} </dev/null >/dev/null 2>&1");
        let bind = key.clone();
        if let Err(e) = run_tmux(&["bind-key", &bind, "run-shell", &script]) {
            debug!("tmux bind prefix {bind}: {e}");
        }
        let meta_bind = format!("M-{key}");
        if let Err(e) = run_tmux(&["bind-key", "-n", &meta_bind, "run-shell", &script]) {
            debug!("tmux bind {meta_bind}: {e}");
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
        ("terminal-features", "xterm-kitty:RGB"),
        ("terminal-features", "xterm-ghostty:extkeys"),
        ("terminal-features", "ghostty:extkeys"),
        // Kitty/iTerm/Sixel graphics (ratatui-image) wrap sequences in tmux passthrough.
        ("allow-passthrough", "on"),
    ] {
        if let Err(e) = run_tmux(&["set-option", "-t", session, opt, val]) {
            debug!("tmux set-option {opt} for {session}: {e}");
        }
    }
    for override_value in [",xterm-kitty:RGB", ",*:RGB"] {
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
    err
}

pub(crate) fn configure_system_clipboard_bindings_inner() -> Result<()> {
    let _ = run_tmux(&["set-option", "-g", "extended-keys", "on"]);
    let Some(pipe) = clipboard_copy_pipe_command() else {
        return Ok(());
    };
    let paste = clipboard_paste_shell_command()?;

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

    let pass = paste_pass_through_key();
    let tui = is_sessions_tui_pane_format();
    bind_key_or_debug(
        "root",
        "C-v",
        &format!("if-shell -F '{tui}' '{pass}' 'run-shell -b \"{paste}\"'"),
    );
    bind_key_or_debug(
        "root",
        "M-v",
        &format!("if-shell -F '{tui}' 'send-keys M-v'"),
    );
    bind_key_or_debug(
        "root",
        "MouseDown2Pane",
        &format!(
            "if-shell -F \"#{{||:#{{pane_in_mode}},#{{mouse_any_flag}}}}\" {{ send-keys -M }} {{ run-shell -b \"{paste}\" }}"
        ),
    );
    Ok(())
}

fn bind_key_or_debug(table: &str, key: &str, command: &str) {
    if let Err(e) = run_tmux(&["bind-key", "-T", table, key, command]) {
        debug!("tmux bind-key -T {table} {key}: {e}");
    }
}

pub(crate) fn clipboard_copy_pipe_command() -> Option<String> {
    crate::clipboard::copy_pipe_command()
}

pub(crate) fn clipboard_paste_shell_command() -> Result<String> {
    Ok(crate::clipboard::tmux_paste_binding_command())
}

pub(crate) fn is_sessions_tui_pane_format() -> &'static str {
    "#{==:#{pane_current_command},sessions}"
}

pub(crate) fn paste_pass_through_key() -> &'static str {
    "send-keys C-v"
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
    Ok(format!(
        "{}; exec env -u TMUX {} attach-session -t {}",
        terminal_backdrop_shell(),
        shell_quote(&tmux.display().to_string()),
        shell_quote(agents_session)
    ))
}
