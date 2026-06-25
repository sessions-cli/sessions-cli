use anyhow::{Context, Result};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

use super::popups::{bind_ui_panel_keys, ensure_ui_workspace_pane};
use super::subprocess::{ensure_tmux_available, resolve_tmux_binary, run_tmux, session_exists};
use super::windows::{configure_prefix, configure_terminal_clipboard, nested_attach_shell_command};

/// Sessions chrome base — terminal OSC 11 backdrop and tmux window bg.
const SESSIONS_BACKDROP_BG: &str = "#000000";
/// Host terminal tab title — must not show `tmux attach-session -t sessions-ui`.
pub const HOST_TERMINAL_TITLE: &str = "sessions";
pub(crate) const WINDOW_STYLE: &str = "bg=#000000";
/// sessions-ui: single-cell separator between sidebar and workspace.
pub(crate) const UI_PANE_BORDER_LINES: &str = "single";
pub(crate) const UI_PANE_BORDER_STYLE: &str = "fg=#4a4a4a,bg=#000000";
const UI_PANE_ACTIVE_BORDER_STYLE: &str = "fg=#6a6a6a,bg=#000000";
/// agents: invisible separator — `spaces` on tmux 3.6; `none` on newer tmux.
const PANE_BORDER_LINES: [&str; 2] = ["none", "spaces"];
pub(crate) const PANE_BORDER_STYLE: &str = "fg=#000000,bg=#000000";
const SIDEBAR_BOOTSTRAP_WIDTH: u16 = 55;
const WORKSPACE_MIN_WIDTH: u16 = 48;
/// Per-pane scrollback depth (tmux default is 2000; consoles easily exceed that).
const HISTORY_LIMIT: u32 = 50_000;

fn configure_session_scrollback(session: &str) {
    let _ = run_tmux(&[
        "set-option",
        "-t",
        session,
        "history-limit",
        &HISTORY_LIMIT.to_string(),
    ]);
}

fn client_width_for_target(target: &str) -> u16 {
    for format in ["#{client_width}", "#{window_width}"] {
        let output = run_tmux(&["display-message", "-p", "-t", target, format]);
        if let Ok(out) = output {
            if out.status.success() {
                let width = String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .parse::<u16>()
                    .unwrap_or(0);
                if width > 0 {
                    return width;
                }
            }
        }
    }
    120
}

fn pane_width_at(target: &str) -> Option<u16> {
    pane_dimension_at(target, "#{pane_width}")
}

fn pane_height_at(target: &str) -> Option<u16> {
    pane_dimension_at(target, "#{pane_height}")
}

fn pane_dimension_at(target: &str, field: &str) -> Option<u16> {
    let output = run_tmux(&["display-message", "-p", "-t", target, field]).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

pub fn current_pane_width() -> Option<u16> {
    if std::env::var("TMUX").is_err() {
        return None;
    }
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    if pane.is_empty() {
        return None;
    }
    pane_width_at(&pane)
}

/// Active pane index (`0` = sidebar, `1` = workspace) in the sessions-ui window.
pub fn ui_window_active_pane_index(ui_session: &str) -> Option<u32> {
    let target = format!("{ui_session}:ui");
    let output = run_tmux(&["display-message", "-p", "-t", &target, "#{pane_index}"]).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Whether this process's tmux pane is the active pane in its window.
pub fn current_pane_is_active() -> Option<bool> {
    if std::env::var("TMUX").is_err() {
        return None;
    }
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    if pane.is_empty() {
        return None;
    }
    let output = run_tmux(&["display-message", "-p", "-t", &pane, "#{pane_active}"]).ok()?;
    if !output.status.success() {
        return None;
    }
    match String::from_utf8_lossy(&output.stdout).trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// Forward graphics-protocol escapes (Kitty, iTerm2, Sixel) through tmux to the host terminal.
pub fn enable_pane_graphics_passthrough(target: Option<&str>) {
    if std::env::var("TMUX").is_err() {
        return;
    }
    let args: Vec<&str> = match target {
        Some(t) if !t.is_empty() => vec!["set-option", "-p", "-t", t, "allow-passthrough", "on"],
        _ => vec!["set-option", "-p", "allow-passthrough", "on"],
    };
    if let Err(e) = run_tmux(&args) {
        debug!("tmux allow-passthrough: {e}");
    }
}

fn enable_session_panes_graphics_passthrough(session: &str) {
    let Ok(output) = run_tmux(&["list-panes", "-t", session, "-F", "#{pane_id}"]) else {
        return;
    };
    for pane_id in String::from_utf8_lossy(&output.stdout).lines() {
        let pane_id = pane_id.trim();
        if pane_id.is_empty() {
            continue;
        }
        enable_pane_graphics_passthrough(Some(pane_id));
    }
}

pub fn clamp_sidebar_width(desired: u16, client_width: u16) -> u16 {
    let max_allowed = client_width.saturating_sub(WORKSPACE_MIN_WIDTH);
    desired.clamp(22, max_allowed.max(22))
}

pub fn current_pane_client_width() -> Option<u16> {
    if std::env::var("TMUX").is_err() {
        return None;
    }
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    if pane.is_empty() {
        return None;
    }
    Some(client_width_for_target(&pane))
}

pub fn resize_pane_width_at(target: &str, desired: u16) -> Result<()> {
    let client_width = client_width_for_target(target);
    let width = clamp_sidebar_width(desired, client_width);
    if pane_width_at(target) == Some(width) {
        return Ok(());
    }
    resize_pane_width_at_fast(target, width)
}

/// Single tmux round-trip resize — used during live divider drags.
pub fn resize_pane_width_at_fast(target: &str, width: u16) -> Result<()> {
    let output = run_tmux(&["resize-pane", "-t", target, "-x", &width.to_string()])?;
    if !output.status.success() {
        anyhow::bail!(
            "resize-pane failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub fn resize_current_pane_width(desired: u16) -> Result<()> {
    if std::env::var("TMUX").is_err() {
        return Ok(());
    }
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    if pane.is_empty() {
        return Ok(());
    }
    resize_pane_width_at(&pane, desired)
}

pub fn resize_current_pane_width_fast(desired: u16, client_width: u16) -> Result<()> {
    if std::env::var("TMUX").is_err() {
        return Ok(());
    }
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    if pane.is_empty() {
        return Ok(());
    }
    let width = clamp_sidebar_width(desired, client_width);
    resize_pane_width_at_fast(&pane, width)
}

/// Grow or shrink the current pane by a column delta — smoother than absolute `-x`
/// when the pointer is pinned at the pane edge during a divider drag.
pub fn resize_current_pane_by(delta: i16) -> Result<()> {
    if delta == 0 || std::env::var("TMUX").is_err() {
        return Ok(());
    }
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    if pane.is_empty() {
        return Ok(());
    }
    let (flag, amount) = if delta > 0 {
        ("-R", delta as u16)
    } else {
        ("-L", (-delta) as u16)
    };
    let output = run_tmux(&["resize-pane", "-t", &pane, flag, &amount.to_string()])?;
    if !output.status.success() {
        anyhow::bail!(
            "resize-pane failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn resize_sidebar_pane(ui_session: &str) -> Result<()> {
    resize_pane_width_at(&format!("{ui_session}:ui.0"), SIDEBAR_BOOTSTRAP_WIDTH)
}

/// Left sidebar + right tmux attach — Kitty-free equivalent of agents-ui.session.
pub fn bootstrap_ui_session(
    ui_session: &str,
    agents_session: &str,
    sessions_bin: &Path,
) -> Result<()> {
    ensure_tmux_available()?;
    if !session_exists(agents_session) {
        anyhow::bail!("tmux session {agents_session} missing; run sessions tmux bootstrap first");
    }
    if session_exists(ui_session) {
        if repair_ui_sidebar(ui_session, sessions_bin)? {
            configure_ui_session(ui_session, agents_session)?;
            return Ok(());
        }
        if repair_ui_workspace(ui_session, agents_session)? {
            configure_ui_session(ui_session, agents_session)?;
            return Ok(());
        }
        if !ui_session_needs_repair(ui_session)? {
            configure_ui_session(ui_session, agents_session)?;
            return Ok(());
        }
        let _ = run_tmux(&["detach-client", "-s", ui_session]);
        run_tmux(&["kill-session", "-t", ui_session])?;
    }

    let attach_cmd = nested_attach_shell_command(agents_session)?;
    let bar_cmd = format!(
        "exec {} bar",
        shell_quote(&sessions_bin.display().to_string())
    );
    let target = format!("{ui_session}:ui");

    let output = run_tmux(&[
        "new-session",
        "-d",
        "-s",
        ui_session,
        "-n",
        "ui",
        "--",
        "/bin/zsh",
        "-lc",
        &attach_cmd,
    ])?;
    if !output.status.success() {
        anyhow::bail!(
            "tmux new-session (ui) failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = run_tmux(&[
        "split-window",
        "-h",
        "-b",
        "-t",
        &target,
        "-l",
        &SIDEBAR_BOOTSTRAP_WIDTH.to_string(),
        "--",
        "/bin/zsh",
        "-lc",
        &bar_cmd,
    ])?;
    if !output.status.success() {
        anyhow::bail!(
            "tmux split-window (sidebar) failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    resize_sidebar_pane(ui_session)?;
    let _ = run_tmux(&["select-pane", "-t", &format!("{target}.1")]);
    configure_ui_session(ui_session, agents_session)?;
    Ok(())
}

/// Mouse-focus + native-feeling scrollback (2026-06-11).
///
/// UI session: wheel only focuses the pane under the cursor and forwards via
/// send-keys -M. Because the agents session has mouse on, the nested client has
/// mouse tracking enabled on the workspace pane tty, so that forward always lands
/// inside the nested client, which dispatches it against these same root bindings
/// (one tmux server, session-name guards).
///
/// Agents session: panes whose program grabbed the mouse (grok, vim, htop —
/// mouse_any_flag) or that are already in a mode get the event via send-keys -M.
/// Everything else (claude, zsh) scrolls the agents pane's real history via
/// copy-mode -eH: -H hides the [pos/size] indicator, -e exits when wheel-down
/// reaches the bottom — so it reads as plain terminal scrollback, not tmux.
///
/// Do not proxy from the UI session into agents panes with `-t agents:=` — that
/// earlier experiment caused 0/0 indicators and jumpy panes. The wheel must be
/// handled by the nested client in its own event context.
fn configure_ui_mouse_focus(ui_session: &str, agents_session: &str) -> Result<()> {
    // Clear stale copy-mode left by prior scroll hacks (shows as 0/0 in the pane corner).
    let _ = run_tmux(&["copy-mode", "-q", "-t", ui_session]);
    let wheel_up = ui_wheel_binding(ui_session, agents_session, WheelDirection::Up);
    let wheel_down = ui_wheel_binding(ui_session, agents_session, WheelDirection::Down);
    let drag = ui_drag_binding(ui_session, agents_session);
    let up = ui_up_binding(ui_session, agents_session);
    let click = ui_click_binding(ui_session);
    let _ = run_tmux(&["bind-key", "-T", "root", "WheelUpPane", &wheel_up]);
    let _ = run_tmux(&["bind-key", "-T", "root", "WheelDownPane", &wheel_down]);
    let _ = run_tmux(&["bind-key", "-T", "root", "MouseDrag1Pane", &drag]);
    let _ = run_tmux(&["bind-key", "-T", "root", "MouseUp1Pane", &up]);
    let _ = run_tmux(&["bind-key", "-T", "root", "MouseDown1Pane", &click]);
    // Restore tmux-native border resize (a prior custom binding may still be loaded).
    let _ = run_tmux(&["unbind-key", "-T", "root", "MouseDrag1Border"]);
    let _ = run_tmux(&["bind-key", "-T", "root", "MouseDrag1Border", "resize-pane", "-M"]);
    Ok(())
}

/// Forward sidebar mouse to ratatui; panel dismiss is handled in the bar app.
pub(crate) fn ui_click_binding(ui_session: &str) -> String {
    let on_sidebar = format!(
        "#{{&&:#{{==:#{{session_name}},{ui_session}}},#{{==:#{{pane_index}},0}}}}"
    );
    let engage_sidebar = format!("if-shell -F '{on_sidebar}' 'select-pane -t =' ''");
    format!("{engage_sidebar} \\; send-keys -M -t =")
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WheelDirection {
    Up,
    Down,
}

/// Root drag binding — sidebar drags must stay on the mousedown pane (no
/// `select-pane -t =`), otherwise divider resize loses mouse-up once the
/// pointer crosses into the workspace pane.
pub(crate) fn ui_drag_binding(ui_session: &str, agents_session: &str) -> String {
    let in_ui = format!("#{{==:#{{session_name}},{ui_session}}}");
    let in_agents = format!("#{{==:#{{session_name}},{agents_session}}}");
    let pane_owns_drag = "#{||:#{pane_in_mode},#{mouse_any_flag}}";
    let agents_branch = format!(
        "if-shell -F -t = '{pane_owns_drag}' {{ send-keys -M -t = }} {{ copy-mode -M -t = }}"
    );
    format!(
        "if-shell -F '{in_ui}' 'send-keys -M' {{ if-shell -F '{in_agents}' '{{ {agents_branch} }}' }}"
    )
}

/// Root mouse-up binding — same pane-stickiness as drag so divider resize can end.
pub(crate) fn ui_up_binding(ui_session: &str, agents_session: &str) -> String {
    let in_ui = format!("#{{==:#{{session_name}},{ui_session}}}");
    let in_agents = format!("#{{==:#{{session_name}},{agents_session}}}");
    let pane_owns_up = "#{||:#{pane_in_mode},#{mouse_any_flag}}";
    let agents_branch = format!("if-shell -F -t = '{pane_owns_up}' 'send-keys -M -t ='");
    format!(
        "if-shell -F '{in_ui}' 'send-keys -M' {{ if-shell -F '{in_agents}' '{{ {agents_branch} }}' }}"
    )
}

/// Root wheel binding shared by every session on the server (hence the
/// session-name guards). See configure_ui_mouse_focus for the design notes.
pub(crate) fn ui_wheel_binding(
    ui_session: &str,
    agents_session: &str,
    direction: WheelDirection,
) -> String {
    let in_ui = format!("#{{==:#{{session_name}},{ui_session}}}");
    let in_agents = format!("#{{==:#{{session_name}},{agents_session}}}");
    let pane_owns_wheel = "#{||:#{pane_in_mode},#{mouse_any_flag}}";
    let agents_branch = match direction {
        WheelDirection::Up => format!(
            "if-shell -F -t = '{pane_owns_wheel}' {{ send-keys -M -t = }} {{ copy-mode -eH -t = ; send-keys -M -t = }}"
        ),
        // At the bottom with no mode active there is nothing to scroll down into.
        WheelDirection::Down => format!("if-shell -F -t = '{pane_owns_wheel}' {{ send-keys -M -t = }}"),
    };
    format!(
        "if-shell -F '{in_ui}' {{ select-pane -t = ; send-keys -M }} {{ if-shell -F '{in_agents}' {{ {agents_branch} }} }}"
    )
}

fn configure_ui_session(ui_session: &str, agents_session: &str) -> Result<()> {
    configure_prefix(ui_session)?;
    configure_session_scrollback(ui_session);
    configure_session_scrollback(agents_session);
    configure_terminal_clipboard(ui_session)?;
    configure_terminal_clipboard(agents_session)?;
    enable_session_panes_graphics_passthrough(ui_session);
    // Mouse must be on for the terminal to deliver click events to tmux at all.
    // Sidebar bindings forward clicks into the sessions bar; the right pane still
    // uses native Cmd+C/V via focus-events + set-clipboard.
    let _ = run_tmux(&["set-option", "-t", ui_session, "mouse", "on"]);
    let _ = run_tmux(&["set-option", "-t", ui_session, "set-titles", "on"]);
    let _ = run_tmux(&[
        "set-option",
        "-t",
        ui_session,
        "set-titles-string",
        HOST_TERMINAL_TITLE,
    ]);
    configure_client_title_hooks(ui_session);
    let _ = push_client_terminal_title();
    let _ = run_tmux(&["set-option", "-t", ui_session, "pane-border-status", "off"]);
    apply_ui_pane_borders(ui_session)?;
    apply_sessions_window_style(ui_session)?;
    let _ = run_tmux(&[
        "set-window-option",
        "-t",
        &format!("{ui_session}:ui"),
        "remain-on-exit",
        "on",
    ]);
    let toggle_mouse = format!(
        "if-shell -F '#{{mouse}}' 'set-option -t {ui_session} mouse off; set-option -t {agents_session} mouse off' 'set-option -t {ui_session} mouse on; set-option -t {agents_session} mouse on'"
    );
    let _ = run_tmux(&["bind-key", "m", &toggle_mouse]);
    let _ = run_tmux(&["bind-key", "-T", "root", "-n", "M-m", &toggle_mouse]);
    configure_ui_mouse_focus(ui_session, agents_session)?;
    bind_ui_panel_keys(ui_session, agents_session, sessions_binary().as_path())?;
    ensure_ui_workspace_pane(ui_session, agents_session)?;
    Ok(())
}

/// Focus the pane this process is running inside (sidebar bar).
pub fn select_own_pane() -> Result<()> {
    if std::env::var("TMUX").is_err() {
        return Ok(());
    }
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    if pane.is_empty() {
        return Ok(());
    }
    run_tmux(&["select-pane", "-t", &pane])?;
    Ok(())
}

pub(crate) fn ui_workspace_target(ui_session: &str) -> String {
    format!("{ui_session}:ui.1")
}
pub(crate) fn pane_current_command(target: &str) -> Result<String> {
    pane_format_field(target, "#{pane_current_command}")
}

pub(crate) fn pane_start_command(target: &str) -> Result<String> {
    pane_format_field(target, "#{pane_start_command}")
}

pub(crate) fn pane_format_field(target: &str, field: &str) -> Result<String> {
    let output = run_tmux(&["display-message", "-p", "-t", target, field])?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
pub fn configure_workspace_session(session: &str) -> Result<()> {
    configure_prefix(session)?;
    configure_session_scrollback(session);
    configure_terminal_clipboard(session)?;
    enable_session_panes_graphics_passthrough(session);
    // Clear stale copy-mode wheel bindings left by earlier experiments.
    // These interfere with the root-table wheel bindings that enter copy-mode
    // with -eH — the stale ones fire after copy-mode entry and can produce
    // parse errors due to stale -F -t = syntax on if-shell. The defaults
    // (select-pane \; send-keys -X -N 5 scroll-up / scroll-down) work fine.
    let _ = run_tmux(&["unbind-key", "-T", "copy-mode", "WheelUpPane"]);
    let _ = run_tmux(&["unbind-key", "-T", "copy-mode", "WheelDownPane"]);
    let _ = run_tmux(&["unbind-key", "-T", "copy-mode-vi", "WheelUpPane"]);
    let _ = run_tmux(&["unbind-key", "-T", "copy-mode-vi", "WheelDownPane"]);
    // Mouse on so the nested client enables mouse tracking on the workspace pane
    // tty. Without it the UI session's send-keys -M is dropped unless the inner
    // program requests the mouse itself, and wheel scrollback never engages.
    let _ = run_tmux(&["set-option", "-t", session, "mouse", "on"]);
    let _ = run_tmux(&["set-option", "-t", session, "pane-border-status", "off"]);
    apply_invisible_pane_borders(session)?;
    let _ = run_tmux(&["set-window-option", "-t", session, "window-size", "latest"]);
    apply_sessions_window_style(session)?;
    Ok(())
}

fn ui_sidebar_target(ui_session: &str) -> String {
    format!("{ui_session}:ui.0")
}

fn ui_sidebar_bar_command(sessions_bin: &Path) -> String {
    format!(
        "exec {} bar",
        shell_quote(&sessions_bin.display().to_string())
    )
}

pub(crate) fn is_sidebar_start_command(command: &str) -> bool {
    command.contains("sessions bar")
}

pub(crate) fn workspace_pane_exists(ui_session: &str) -> Result<bool> {
    Ok(list_ui_panes(ui_session)?
        .iter()
        .any(|pane| pane.index == 1))
}

pub(crate) fn list_ui_panes(ui_session: &str) -> Result<Vec<UiPaneInfo>> {
    let output = run_tmux(&[
        "list-panes",
        "-t",
        ui_session,
        "-F",
        "#{pane_index}\t#{pane_id}\t#{pane_dead}\t#{pane_start_command}",
    ])?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(UiPaneInfo::parse)
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiPaneInfo {
    pub(crate) index: u32,
    pub(crate) pane_id: String,
    pub(crate) dead: bool,
    pub(crate) start_command: String,
}

impl UiPaneInfo {
    pub(crate) fn parse(line: &str) -> Option<Self> {
        let mut parts = line.splitn(4, '\t');
        let index = parts.next()?.parse().ok()?;
        let pane_id = parts.next()?.to_string();
        let dead = parts.next()? == "1";
        let start_command = parts.next()?.to_string();
        Some(Self {
            index,
            pane_id,
            dead,
            start_command,
        })
    }
}

fn spawn_workspace_pane(ui_session: &str, agents_session: &str, split_target: &str) -> Result<()> {
    let attach = nested_attach_shell_command(agents_session)?;
    run_tmux(&[
        "split-window",
        "-h",
        "-t",
        split_target,
        "--",
        "/bin/zsh",
        "-lc",
        &attach,
    ])?;
    resize_sidebar_pane(ui_session)?;
    run_tmux(&["select-pane", "-t", &ui_workspace_target(ui_session)])?;
    Ok(())
}

fn spawn_sidebar_pane(ui_session: &str, sessions_bin: &Path, split_target: &str) -> Result<()> {
    let bar_cmd = ui_sidebar_bar_command(sessions_bin);
    run_tmux(&[
        "split-window",
        "-h",
        "-b",
        "-t",
        split_target,
        "-l",
        &SIDEBAR_BOOTSTRAP_WIDTH.to_string(),
        "--",
        "/bin/zsh",
        "-lc",
        &bar_cmd,
    ])?;
    resize_sidebar_pane(ui_session)?;
    run_tmux(&["select-pane", "-t", &ui_workspace_target(ui_session)])?;
    Ok(())
}

/// Recreate or respawn the workspace pane when ui.1 is missing or dead.
pub(crate) fn repair_ui_workspace(ui_session: &str, agents_session: &str) -> Result<bool> {
    let panes = list_ui_panes(ui_session)?;
    if panes.len() == 2 {
        let workspace = panes.iter().find(|pane| pane.index == 1);
        if let Some(workspace) = workspace {
            if workspace.dead {
                let attach = nested_attach_shell_command(agents_session)?;
                run_tmux(&[
                    "respawn-pane",
                    "-k",
                    "-t",
                    &workspace.pane_id,
                    "/bin/zsh",
                    "-lc",
                    &attach,
                ])?;
                run_tmux(&["select-pane", "-t", &ui_workspace_target(ui_session)])?;
                return Ok(true);
            }
        }
        return Ok(false);
    }

    if panes.len() == 1 {
        let pane = &panes[0];
        if is_sidebar_start_command(&pane.start_command) {
            spawn_workspace_pane(ui_session, agents_session, &pane.pane_id)?;
            return Ok(true);
        }
        if pane.start_command.contains("workspace-wrap")
            || pane.start_command.contains("attach-session")
        {
            spawn_sidebar_pane(ui_session, &sessions_binary(), &pane.pane_id)?;
            return Ok(true);
        }
    }

    Ok(false)
}

/// Respawn a dead sidebar pane without tearing down the attach pane.
fn repair_ui_sidebar(ui_session: &str, sessions_bin: &Path) -> Result<bool> {
    let output = run_tmux(&[
        "list-panes",
        "-t",
        ui_session,
        "-F",
        "#{pane_index}\t#{pane_id}\t#{pane_dead}",
    ])?;
    let pane_output = String::from_utf8_lossy(&output.stdout);
    let panes: Vec<_> = pane_output.lines().filter(|l| !l.is_empty()).collect();
    if panes.len() != 2 {
        return Ok(false);
    }
    let dead_sidebar = panes.iter().find_map(|line| {
        let mut parts = line.splitn(3, '\t');
        let index = parts.next()?;
        let pane_id = parts.next()?;
        let dead = parts.next()?;
        (index == "0" && dead == "1").then_some(pane_id.to_string())
    });
    let Some(pane_id) = dead_sidebar else {
        return Ok(false);
    };

    let bar_cmd = ui_sidebar_bar_command(sessions_bin);
    run_tmux(&[
        "respawn-pane",
        "-k",
        "-t",
        &pane_id,
        "/bin/zsh",
        "-lc",
        &bar_cmd,
    ])?;
    resize_sidebar_pane(ui_session)?;
    let _ = run_tmux(&["select-pane", "-t", &format!("{ui_session}:ui.1")]);
    Ok(true)
}

fn ui_session_needs_repair(ui_session: &str) -> Result<bool> {
    let output = run_tmux(&[
        "list-panes",
        "-t",
        ui_session,
        "-F",
        "#{pane_id}\t#{pane_dead}",
    ])?;
    let pane_output = String::from_utf8_lossy(&output.stdout);
    let panes: Vec<_> = pane_output.lines().filter(|l| !l.is_empty()).collect();
    if panes.len() != 2 {
        return Ok(true);
    }
    Ok(panes.iter().any(|line| line.ends_with("\t1")))
}

pub fn attach_ui_session(ui_session: &str) -> Result<()> {
    write_host_terminal_backdrop()?;
    let tmux = resolve_tmux_binary()?;
    let script = format!(
        "{title_printf} && exec -a {title} {tmux} attach-session -t {session}",
        title_printf = host_terminal_title_bash_printf(),
        title = shell_quote(HOST_TERMINAL_TITLE),
        tmux = shell_quote(&tmux.display().to_string()),
        session = shell_quote(ui_session),
    );
    Err(Command::new("/bin/bash")
        .arg("-c")
        .arg(script)
        .exec()
        .into())
}

/// OSC 11 on the host terminal — hides emulator padding around the tmux client.
pub fn write_host_terminal_backdrop() -> Result<()> {
    use std::io::Write;
    write!(std::io::stdout(), "{}", host_terminal_backdrop_sequence())?;
    std::io::stdout().flush()?;
    Ok(())
}

pub fn host_terminal_title_sequence() -> String {
    format!("\x1b]0;{HOST_TERMINAL_TITLE}\x07\x1b]2;{HOST_TERMINAL_TITLE}\x07")
}

pub fn host_terminal_title_bash_printf() -> String {
    format!(
        "printf '\\033]0;{0}\\007\\033]2;{0}\\007'",
        HOST_TERMINAL_TITLE
    )
}

pub fn push_client_terminal_title_script() -> String {
    format!(
        "{title_printf} >\"$(tmux display-message -p '#{{client_tty}}')\"",
        title_printf = host_terminal_title_bash_printf()
    )
}

pub fn push_client_terminal_title() -> Result<()> {
    run_tmux(&["run-shell", "-b", &push_client_terminal_title_script()])?;
    Ok(())
}

fn configure_client_title_hooks(ui_session: &str) {
    let hook = format!(
        "run-shell -b {}",
        shell_quote(&push_client_terminal_title_script())
    );
    for event in ["client-attached", "pane-focus-in"] {
        let _ = run_tmux(&["set-hook", "-t", ui_session, event, &hook]);
    }
}

pub fn host_terminal_backdrop_sequence() -> String {
    format!(
        "{}\x1b]11;{SESSIONS_BACKDROP_BG}\x07\x1b[49m",
        host_terminal_title_sequence(),
    )
}

pub fn sessions_binary() -> PathBuf {
    crate::paths::resolve_binary(&crate::paths::home())
}

pub(crate) fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        return s.into();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn tmux_window_name(index: usize, title: &str) -> String {
    let slug: String = title
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
    if slug.is_empty() {
        format!("ws-{}", index + 1)
    } else {
        format!("ws-{}-{slug}", index + 1)
    }
}

fn try_tmux_option(session: &str, option: &str, value: &str) -> bool {
    run_tmux(&["set-option", "-t", session, option, value]).is_ok()
}

fn apply_ui_pane_borders(session: &str) -> Result<()> {
    let _ = run_tmux(&[
        "set-option",
        "-t",
        session,
        "pane-border-lines",
        UI_PANE_BORDER_LINES,
    ]);
    let _ = run_tmux(&[
        "set-option",
        "-t",
        session,
        "pane-border-style",
        UI_PANE_BORDER_STYLE,
    ]);
    let _ = run_tmux(&[
        "set-option",
        "-t",
        session,
        "pane-active-border-style",
        UI_PANE_ACTIVE_BORDER_STYLE,
    ]);
    Ok(())
}

fn apply_invisible_pane_borders(session: &str) -> Result<()> {
    for lines in PANE_BORDER_LINES {
        if try_tmux_option(session, "pane-border-lines", lines) {
            break;
        }
    }
    let _ = run_tmux(&[
        "set-option",
        "-t",
        session,
        "pane-border-style",
        PANE_BORDER_STYLE,
    ]);
    let _ = run_tmux(&[
        "set-option",
        "-t",
        session,
        "pane-active-border-style",
        PANE_BORDER_STYLE,
    ]);
    Ok(())
}

fn apply_sessions_window_style(session: &str) -> Result<()> {
    let _ = run_tmux(&[
        "set-window-option",
        "-t",
        session,
        "window-style",
        WINDOW_STYLE,
    ]);
    let _ = run_tmux(&[
        "set-window-option",
        "-t",
        session,
        "window-active-style",
        WINDOW_STYLE,
    ]);
    Ok(())
}

/// OSC 11 + black fill — workspace panes should not show the host terminal theme.
pub(crate) fn terminal_backdrop_shell() -> String {
    format!("printf '\\033]11;{SESSIONS_BACKDROP_BG}\\033\\\\\\033[49m\\033[40m\\033[2J\\033[H'")
}

/// Raw console windows: paint black backdrop before the login shell so theme padding cannot show through.
pub(crate) fn console_shell_command() -> String {
    format!("{}; exec /bin/zsh -l", terminal_backdrop_shell())
}

pub(crate) fn workspace_shell_command(command: &str) -> String {
    // Passed as a single argv to `zsh -lc` (not via a parent shell), so do not
    // re-escape quotes — launch commands already include shell quoting for prompts.
    format!("{command} || exec /bin/zsh -l")
}
