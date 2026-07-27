use anyhow::Result;
use std::path::Path;
use tracing::debug;

use super::layout::{
    enable_pane_graphics_passthrough, pane_current_command, pane_start_command,
    repair_ui_workspace, sessions_binary, shell_quote, ui_workspace_target, workspace_pane_exists,
};
use super::subprocess::run_tmux;
use super::windows::nested_attach_shell_command;

/// Which workspace-pane overlay is currently running (if any).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkspacePanelState {
    pub settings: bool,
    pub new_session: bool,
    pub automations: bool,
    pub mcps: bool,
    pub skills: bool,
}

impl WorkspacePanelState {
    pub fn any(self) -> bool {
        self.settings || self.new_session || self.automations || self.mcps || self.skills
    }
}

fn workspace_settings_wrapper(agents_session: &str) -> Result<String> {
    workspace_panel_wrapper(agents_session, "settings")
}

fn workspace_new_session_wrapper(agents_session: &str) -> Result<String> {
    workspace_panel_wrapper(agents_session, "new-session")
}

fn workspace_automations_wrapper(agents_session: &str) -> Result<String> {
    workspace_panel_wrapper(agents_session, "automations")
}

fn workspace_mcps_wrapper(agents_session: &str) -> Result<String> {
    workspace_panel_wrapper(agents_session, "mcps")
}

fn workspace_skills_wrapper(agents_session: &str) -> Result<String> {
    workspace_panel_wrapper(agents_session, "skills")
}

/// True while the workspace pane is running the new-session launcher (current or legacy command).
pub(crate) fn workspace_pane_is_new_session_panel(
    start_command: &str,
    current_command: &str,
) -> bool {
    workspace_pane_is_panel_mode(start_command, current_command, "new-session")
        || workspace_pane_is_panel_mode(start_command, current_command, "new-chat")
}

pub(crate) fn workspace_panel_wrapper(agents_session: &str, subcommand: &str) -> Result<String> {
    let sessions_bin = sessions_binary();
    let attach = nested_attach_shell_command(agents_session)?;
    Ok(format!(
        "{} {subcommand}; {}",
        shell_quote(&sessions_bin.display().to_string()),
        attach
    ))
}
/// True while the workspace pane is running a sessions overlay (before attach resumes).
pub(crate) fn workspace_pane_is_panel_mode(
    start_command: &str,
    current_command: &str,
    subcommand: &str,
) -> bool {
    let marker = format!("sessions {subcommand}");
    if current_command.contains("sessions") && current_command.contains(subcommand) {
        return true;
    }
    start_command.contains(&marker) && current_command != "tmux"
}

/// Queue a workspace-pane new-session toggle via tmux run-shell (safe from inside the sidebar).
pub fn spawn_toggle_workspace_new_session() -> Result<()> {
    spawn_panel_toggle("new-session")
}

/// Queue an open-only new-session panel (never toggle-close).
pub fn spawn_open_workspace_new_session() -> Result<()> {
    spawn_panel_command("open-new-session")
}

pub fn spawn_open_workspace_automations() -> Result<()> {
    spawn_panel_command("automations")
}

pub fn spawn_open_workspace_mcps() -> Result<()> {
    spawn_panel_command("open-mcps")
}

pub fn spawn_open_workspace_skills() -> Result<()> {
    spawn_panel_command("open-skills")
}

/// Queue a workspace-pane settings toggle via tmux run-shell (safe from inside the sidebar).
pub fn spawn_toggle_workspace_settings() -> Result<()> {
    spawn_panel_toggle("settings")
}

fn spawn_panel_toggle(panel: &str) -> Result<()> {
    spawn_panel_command(panel)
}

fn spawn_panel_command(panel: &str) -> Result<()> {
    let bin = shell_quote(&sessions_binary().display().to_string());
    let script = format!("{bin} panel {panel} </dev/null >/dev/null 2>&1");
    run_tmux(&["run-shell", "-b", &script]).map(|_| ())
}

pub(crate) fn bind_ui_panel_keys(
    ui_session: &str,
    agents_session: &str,
    sessions_bin: &Path,
) -> Result<()> {
    let bin = shell_quote(&sessions_bin.display().to_string());
    let in_sessions = panel_key_session_guard(ui_session, agents_session);
    // M- is Meta/Command in this setup; S- is Shift (not ⌘+N).
    for stale in ["S-n", "S-N"] {
        let _ = run_tmux(&["unbind-key", "-T", "root", "-n", stale]);
    }
    let bindings: &[(&str, &str)] = &[
        ("M-n", "open-new-session"),
        ("M-N", "open-new-session"),
        ("M-a", "automations"),
        ("M-A", "automations"),
        ("M-m", "open-mcps"),
        ("M-M", "open-mcps"),
        ("M-k", "open-skills"),
        ("M-K", "open-skills"),
        ("M-Comma", "settings"),
    ];
    for (key, panel) in bindings {
        // Bind if-shell directly (not nested inside run-shell -b). The true branch must be a
        // tmux command name; use run-shell -b so the panel script runs via sh(1).
        let panel_script = format!("{bin} panel {panel} </dev/null >/dev/null 2>&1");
        let true_cmd = format!("run-shell -b \"{panel_script}\"");
        if let Err(e) = run_tmux(&[
            "bind-key",
            "-T",
            "root",
            "-n",
            key,
            "if-shell",
            "-F",
            &in_sessions,
            &true_cmd,
        ]) {
            debug!("tmux bind ui panel {key}: {e}");
        }
    }
    Ok(())
}

/// tmux root bindings for panel shortcuts — not when the sidebar bar pane has focus
/// (ui.0); the ratatui bar handles ⌘+N there. Workspace pane and agents session keep
/// the binding so ⌘+N works while the pointer is over the sidebar without focus.
pub(crate) fn panel_key_session_guard(ui_session: &str, agents_session: &str) -> String {
    format!(
        "#{{||:#{{==:#{{session_name}},{agents_session}}},#{{&&:#{{==:#{{session_name}},{ui_session}}},#{{!=:#{{pane_index}},0}}}}}}"
    )
}

/// Whether the workspace pane is running settings, new-session, automations, or mcps panels.
pub fn workspace_pane_panel_state(ui_session: &str) -> Result<WorkspacePanelState> {
    // One display-message for both fields. A failed call covers missing session
    // (replaces the previous has-session + 2× display-message probe storm).
    let target = ui_workspace_target(ui_session);
    let Ok(output) = run_tmux(&[
        "display-message",
        "-p",
        "-t",
        &target,
        "#{pane_start_command}\t#{pane_current_command}",
    ]) else {
        return Ok(WorkspacePanelState::default());
    };
    if !output.status.success() {
        return Ok(WorkspacePanelState::default());
    }
    let body = String::from_utf8_lossy(&output.stdout);
    let body = body.trim_end_matches(['\n', '\r']);
    let (start, current) = body
        .split_once('\t')
        .map(|(s, c)| (s.to_string(), c.to_string()))
        .unwrap_or_else(|| (body.to_string(), String::new()));
    Ok(WorkspacePanelState {
        settings: workspace_pane_is_panel_mode(&start, &current, "settings"),
        new_session: workspace_pane_is_new_session_panel(&start, &current),
        automations: workspace_pane_is_panel_mode(&start, &current, "automations"),
        mcps: workspace_pane_is_panel_mode(&start, &current, "mcps"),
        skills: workspace_pane_is_panel_mode(&start, &current, "skills"),
    })
}

pub fn workspace_pane_running_settings(ui_session: &str) -> Result<bool> {
    Ok(workspace_pane_panel_state(ui_session)?.settings)
}

pub fn workspace_pane_running_new_session(ui_session: &str) -> Result<bool> {
    Ok(workspace_pane_panel_state(ui_session)?.new_session)
}

pub fn workspace_pane_running_automations(ui_session: &str) -> Result<bool> {
    Ok(workspace_pane_panel_state(ui_session)?.automations)
}

pub fn workspace_pane_running_mcps(ui_session: &str) -> Result<bool> {
    Ok(workspace_pane_panel_state(ui_session)?.mcps)
}

pub fn workspace_pane_running_skills(ui_session: &str) -> Result<bool> {
    Ok(workspace_pane_panel_state(ui_session)?.skills)
}

/// Cancel legacy tmux display-popups from older panel models.
fn cancel_popup_on_all_clients() -> Result<()> {
    let mut closed = false;
    if let Ok(output) = run_tmux(&["list-clients", "-F", "#{client_name}"]) {
        for client in String::from_utf8_lossy(&output.stdout).lines() {
            let client = client.trim();
            if client.is_empty() {
                continue;
            }
            if run_tmux(&["display-popup", "-C", "-c", client]).is_ok() {
                closed = true;
            }
        }
    }
    if !closed {
        let _ = run_tmux(&["display-popup", "-C"]);
    }
    Ok(())
}

pub fn close_ui_panel_popup() -> Result<()> {
    cancel_popup_on_all_clients()
}

/// Close an open workspace panel and restore the nested agents attach.
pub fn dismiss_ui_panel_popups(ui_session: &str, agents_session: &str) -> Result<()> {
    let _ = cancel_popup_on_all_clients();
    restore_workspace_attach_if_panel_respawn(ui_session, agents_session)
}

pub fn close_workspace_new_session_popup() -> Result<()> {
    cancel_popup_on_all_clients()
}

pub fn restore_workspace_attach(ui_session: &str, agents_session: &str) -> Result<()> {
    if !workspace_pane_exists(ui_session)? {
        repair_ui_workspace(ui_session, agents_session)?;
        return Ok(());
    }
    let target = ui_workspace_target(ui_session);
    let attach = nested_attach_shell_command(agents_session)?;
    run_tmux(&[
        "respawn-pane",
        "-k",
        "-t",
        &target,
        "/bin/zsh",
        "-lc",
        &attach,
    ])?;
    run_tmux(&["select-pane", "-t", &target])?;
    Ok(())
}

fn restore_workspace_attach_if_panel_respawn(ui_session: &str, agents_session: &str) -> Result<()> {
    if !workspace_pane_exists(ui_session)? {
        return Ok(());
    }
    let target = ui_workspace_target(ui_session);
    let start = pane_start_command(&target)?;
    let current = pane_current_command(&target)?;
    if workspace_pane_is_panel_mode(&start, &current, "settings")
        || workspace_pane_is_new_session_panel(&start, &current)
        || workspace_pane_is_panel_mode(&start, &current, "automations")
        || workspace_pane_is_panel_mode(&start, &current, "mcps")
        || workspace_pane_is_panel_mode(&start, &current, "skills")
    {
        restore_workspace_attach(ui_session, agents_session)?;
    }
    Ok(())
}

pub fn open_workspace_settings(ui_session: &str, agents_session: &str) -> Result<()> {
    let target = ui_workspace_target(ui_session);
    let wrapper = workspace_settings_wrapper(agents_session)?;
    run_tmux(&[
        "respawn-pane",
        "-k",
        "-t",
        &target,
        "/bin/zsh",
        "-lc",
        &wrapper,
    ])?;
    run_tmux(&["select-pane", "-t", &target])?;
    Ok(())
}

pub fn open_workspace_new_session(ui_session: &str, agents_session: &str) -> Result<()> {
    if workspace_pane_running_new_session(ui_session)? {
        return Ok(());
    }
    if workspace_pane_running_settings(ui_session)?
        || workspace_pane_running_automations(ui_session)?
        || workspace_pane_running_mcps(ui_session)?
        || workspace_pane_running_skills(ui_session)?
    {
        restore_workspace_attach(ui_session, agents_session)?;
    }
    let target = ui_workspace_target(ui_session);
    enable_pane_graphics_passthrough(Some(&target));
    let wrapper = workspace_new_session_wrapper(agents_session)?;
    run_tmux(&[
        "respawn-pane",
        "-k",
        "-t",
        &target,
        "/bin/zsh",
        "-lc",
        &wrapper,
    ])?;
    run_tmux(&["select-pane", "-t", &target])?;
    Ok(())
}

pub fn open_workspace_automations(ui_session: &str, agents_session: &str) -> Result<()> {
    if workspace_pane_running_automations(ui_session)? {
        return Ok(());
    }
    if workspace_pane_running_settings(ui_session)?
        || workspace_pane_running_new_session(ui_session)?
        || workspace_pane_running_mcps(ui_session)?
        || workspace_pane_running_skills(ui_session)?
    {
        restore_workspace_attach(ui_session, agents_session)?;
    }
    let target = ui_workspace_target(ui_session);
    enable_pane_graphics_passthrough(Some(&target));
    let wrapper = workspace_automations_wrapper(agents_session)?;
    run_tmux(&[
        "respawn-pane",
        "-k",
        "-t",
        &target,
        "/bin/zsh",
        "-lc",
        &wrapper,
    ])?;
    run_tmux(&["select-pane", "-t", &target])?;
    Ok(())
}

pub fn open_workspace_mcps(ui_session: &str, agents_session: &str) -> Result<()> {
    if workspace_pane_running_mcps(ui_session)? {
        return Ok(());
    }
    if workspace_pane_running_settings(ui_session)?
        || workspace_pane_running_new_session(ui_session)?
        || workspace_pane_running_automations(ui_session)?
        || workspace_pane_running_skills(ui_session)?
    {
        restore_workspace_attach(ui_session, agents_session)?;
    }
    let target = ui_workspace_target(ui_session);
    enable_pane_graphics_passthrough(Some(&target));
    let wrapper = workspace_mcps_wrapper(agents_session)?;
    run_tmux(&[
        "respawn-pane",
        "-k",
        "-t",
        &target,
        "/bin/zsh",
        "-lc",
        &wrapper,
    ])?;
    run_tmux(&["select-pane", "-t", &target])?;
    Ok(())
}

pub fn open_workspace_skills(ui_session: &str, agents_session: &str) -> Result<()> {
    if workspace_pane_running_skills(ui_session)? {
        return Ok(());
    }
    if workspace_pane_running_settings(ui_session)?
        || workspace_pane_running_new_session(ui_session)?
        || workspace_pane_running_automations(ui_session)?
        || workspace_pane_running_mcps(ui_session)?
    {
        restore_workspace_attach(ui_session, agents_session)?;
    }
    let target = ui_workspace_target(ui_session);
    enable_pane_graphics_passthrough(Some(&target));
    let wrapper = workspace_skills_wrapper(agents_session)?;
    run_tmux(&[
        "respawn-pane",
        "-k",
        "-t",
        &target,
        "/bin/zsh",
        "-lc",
        &wrapper,
    ])?;
    run_tmux(&["select-pane", "-t", &target])?;
    Ok(())
}

/// Toggle settings in the workspace pane; restores agent attach when closed.
pub fn toggle_workspace_settings(ui_session: &str, agents_session: &str) -> Result<bool> {
    if workspace_pane_running_settings(ui_session)? {
        restore_workspace_attach(ui_session, agents_session)?;
        Ok(false)
    } else {
        open_workspace_settings(ui_session, agents_session)?;
        Ok(true)
    }
}

/// Toggle new-session in the workspace pane; restores agent attach when closed.
pub fn toggle_workspace_new_session(ui_session: &str, agents_session: &str) -> Result<bool> {
    if workspace_pane_running_new_session(ui_session)? {
        restore_workspace_attach(ui_session, agents_session)?;
        Ok(false)
    } else {
        open_workspace_new_session(ui_session, agents_session)?;
        Ok(true)
    }
}
pub(crate) fn ensure_ui_workspace_pane(ui_session: &str, agents_session: &str) -> Result<()> {
    if !workspace_pane_exists(ui_session)? {
        repair_ui_workspace(ui_session, agents_session)?;
        return Ok(());
    }
    let target = ui_workspace_target(ui_session);
    let start = pane_start_command(&target)?;
    let current = pane_current_command(&target)?;
    if workspace_pane_is_panel_mode(&start, &current, "settings") {
        return Ok(());
    }
    if start.contains("screenrc-workspace") || start.contains("workspace-wrap") {
        restore_workspace_attach(ui_session, agents_session)?;
    }
    Ok(())
}
