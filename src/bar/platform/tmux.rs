//! Bar-facing tmux facade — pure delegation to `daemon::tmux`.

pub use crate::daemon::tmux::{
    active_window_summary, confirm_close_active_window, create_agent_window,
    create_agent_window_in_cwd, create_console_window, create_window_in_cwd,
    current_pane_is_active, current_pane_width, detach_current_client, pane_effective_cwd,
    resize_current_pane_width, restore_workspace_attach, run_tmux, save_pane_state, select_own_pane,
    select_window, spawn_open_workspace_mcps, spawn_open_workspace_new_session,
    spawn_open_workspace_skills, spawn_toggle_workspace_settings, ui_window_active_pane_index,
    workspace_pane_panel_state, workspace_pane_running_settings, write_host_terminal_backdrop,
};