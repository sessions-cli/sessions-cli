mod agents;
mod layout;
mod poll;
mod popups;
mod subprocess;
mod windows;

#[cfg(test)]
mod tests;

pub use agents::*;
pub use layout::*;
pub use poll::*;
pub use popups::*;
pub use subprocess::*;
pub use windows::*;

#[cfg(test)]
pub(crate) use layout::{
    auto_window_switch_disable_options, is_sidebar_start_command, ui_click_binding,
    ui_drag_binding, ui_up_binding, ui_wheel_binding, UiPaneInfo, WheelDirection,
    PANE_BORDER_STYLE, UI_PANE_BORDER_LINES, UI_PANE_BORDER_STYLE, WINDOW_STYLE,
};
#[cfg(test)]
pub(crate) use popups::{
    panel_key_session_guard, workspace_pane_is_new_session_panel, workspace_pane_is_panel_mode,
    workspace_panel_wrapper,
};
#[cfg(test)]
pub(crate) use windows::{
    clipboard_copy_pipe_command, clipboard_paste_key_command, clipboard_paste_load_shell_command,
    clipboard_paste_root_binding, clipboard_paste_run_shell_command, clipboard_paste_shell_command,
    instant_key_bind_script, is_sessions_tui_pane_format, literal_send_chunks,
    osc52_os_clipboard_hook_command, parse_window_line, paste_pass_through_key,
};
