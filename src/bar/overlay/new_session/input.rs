//! New-session keyboard/mouse input handling.

use super::render::{
    draw_screen, dropdown_popup_click_index, point_in_prompt, sync_panel_mouse_cursor,
    workspace_popup_click_index, workspace_popup_header_click, workspace_popup_row_from_mouse,
    ClickTargets,
};
use super::state::{
    path_entry_char, Focus, LaunchMode, LaunchOutcome, NewSessionState, PanelHover,
};
use crate::agents;
use crate::bar::keys::{has_command_modifier, has_paste_modifier};
use crate::bar::settings::point_in_rect;
use crate::config::Config;
use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewSessionAction {
    Unchanged,
    Close,
    Launched,
}

pub fn apply_paste(state: &mut NewSessionState, text: &str) {
    if !text.is_empty() {
        state.apply_paste(text);
    }
}

pub fn handle_key(
    state: &mut NewSessionState,
    config: &Config,
    key: event::KeyEvent,
) -> Result<NewSessionAction> {
    if key.kind == KeyEventKind::Release {
        return Ok(NewSessionAction::Unchanged);
    }
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    if key.code == KeyCode::Enter && has_command_modifier(key.modifiers) && shift {
        let _ = state.launch(config, LaunchMode::Background)?;
        return Ok(NewSessionAction::Unchanged);
    }
    if (key.code == KeyCode::Char('b') || key.code == KeyCode::Char('B'))
        && has_command_modifier(key.modifiers)
    {
        let _ = state.launch(config, LaunchMode::Background)?;
        return Ok(NewSessionAction::Unchanged);
    }
    if (key.code == KeyCode::Char('f') || key.code == KeyCode::Char('F'))
        && has_command_modifier(key.modifiers)
    {
        if state.launch(config, LaunchMode::Open)? == LaunchOutcome::Opened {
            return Ok(NewSessionAction::Launched);
        }
        return Ok(NewSessionAction::Unchanged);
    }
    if key.code == KeyCode::Enter
        && has_paste_modifier(key.modifiers)
        && state.focus != Focus::Prompt
    {
        if state.launch(config, LaunchMode::Open)? == LaunchOutcome::Opened {
            return Ok(NewSessionAction::Launched);
        }
        return Ok(NewSessionAction::Unchanged);
    }
    match key.code {
        KeyCode::Esc => return Ok(NewSessionAction::Close),
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(NewSessionAction::Close);
        }
        KeyCode::Char('c') | KeyCode::Char('C')
            if has_paste_modifier(key.modifiers) && state.focus == Focus::Prompt =>
        {
            state.copy_prompt_selection();
        }
        KeyCode::Char('c')
            if key
                .modifiers
                .intersects(KeyModifiers::SUPER | KeyModifiers::META | KeyModifiers::ALT) =>
        {
            return Ok(NewSessionAction::Close);
        }
        KeyCode::BackTab => {
            let next = state.prev_focus();
            state.set_focus(next);
            if next == Focus::Workspace {
                state.sync_popup_highlight_to_workspace_idx();
            }
        }
        KeyCode::Tab if shift => {
            let next = state.prev_focus();
            state.set_focus(next);
            if next == Focus::Workspace {
                state.sync_popup_highlight_to_workspace_idx();
            }
        }
        KeyCode::Tab if state.focus == Focus::Workspace => {
            if state.is_typing_path() {
                if !state.tab_complete_workspace() {
                    state.cycle_workspace_popup(1);
                }
            } else {
                state.cycle_workspace_popup(1);
            }
        }
        KeyCode::Tab if state.focus == Focus::Agent => {
            state.cycle_agent(1);
        }
        KeyCode::Tab if state.focus == Focus::Model => {
            state.cycle_model(1);
        }
        KeyCode::Tab => {
            let next = state.next_focus();
            state.set_focus(next);
            if next == Focus::Workspace {
                state.sync_popup_highlight_to_workspace_idx();
            }
        }
        KeyCode::Backspace
            if state.focus == Focus::Prompt && has_command_modifier(key.modifiers) =>
        {
            state.clear_prompt(state.prompt_field_width);
        }
        KeyCode::Backspace if state.focus == Focus::Prompt => {
            if !state.prompt_delete_selection(state.prompt_field_width) {
                state.prompt_backspace(state.prompt_field_width);
            }
        }
        KeyCode::Delete if state.focus == Focus::Prompt => {
            if !state.prompt_delete_selection(state.prompt_field_width) {
                state.prompt_forward_delete(state.prompt_field_width);
            }
        }
        KeyCode::Char('v') | KeyCode::Char('V') if has_paste_modifier(key.modifiers) => {
            if let Some(raw) = crate::clipboard::paste().ok().filter(|t| !t.is_empty()) {
                state.apply_paste(&raw);
            }
        }
        KeyCode::Char('a') | KeyCode::Char('A')
            if has_paste_modifier(key.modifiers) && state.focus == Focus::Prompt =>
        {
            state.select_prompt_all(state.prompt_field_width);
        }
        KeyCode::Char('x') | KeyCode::Char('X')
            if has_paste_modifier(key.modifiers) && state.focus == Focus::Prompt =>
        {
            state.cut_prompt_selection(state.prompt_field_width);
        }
        KeyCode::Char(c)
            if state.focus == Focus::Prompt
                && !key.modifiers.intersects(
                    KeyModifiers::CONTROL
                        | KeyModifiers::SUPER
                        | KeyModifiers::META
                        | KeyModifiers::ALT,
                ) =>
        {
            state.insert_prompt_char(c, state.prompt_field_width);
        }
        KeyCode::Backspace if state.focus == Focus::Workspace => {
            state.on_workspace_backspace();
        }
        KeyCode::Delete if state.focus == Focus::Workspace && state.is_typing_path() => {
            state.on_workspace_forward_delete();
        }
        KeyCode::Home if state.focus == Focus::Workspace && state.is_typing_path() => {
            state.workspace_path_cursor = 0;
        }
        KeyCode::End if state.focus == Focus::Workspace && state.is_typing_path() => {
            state.workspace_path_cursor = state.workspace_path_input.chars().count();
        }
        KeyCode::Char(c) if state.focus == Focus::Workspace && path_entry_char(c) => {
            state.on_workspace_type(c);
        }
        KeyCode::Char('d')
            if state.focus != Focus::Prompt
                && (state.focus != Focus::Workspace || !state.is_typing_path()) =>
        {
            state.set_default_for_focus(config);
        }
        KeyCode::Up | KeyCode::Char('k') => match state.focus {
            Focus::Prompt => {
                state.clear_prompt_selection();
                state.move_prompt_cursor_vertical(-1, state.prompt_field_width);
            }
            Focus::Workspace | Focus::Agent | Focus::Model => {
                state.cycle_focused_dropdown(-1);
            }
            _ => {}
        },
        KeyCode::Down | KeyCode::Char('j') => match state.focus {
            Focus::Prompt => {
                state.clear_prompt_selection();
                state.move_prompt_cursor_vertical(1, state.prompt_field_width);
            }
            Focus::Workspace | Focus::Agent | Focus::Model => {
                state.cycle_focused_dropdown(1);
            }
            _ => {}
        },
        KeyCode::Left | KeyCode::Char('h') => match state.focus {
            Focus::Prompt => {
                state.clear_prompt_selection();
                state.move_prompt_cursor(-1, state.prompt_field_width);
            }
            Focus::Workspace if state.is_typing_path() => state.move_workspace_path_cursor(-1),
            Focus::Workspace | Focus::Agent | Focus::Model => {
                state.cycle_focused_dropdown(-1);
            }
            Focus::BackgroundButton => state.set_focus(Focus::ForegroundButton),
            _ => {}
        },
        KeyCode::Right | KeyCode::Char('l') => match state.focus {
            Focus::Prompt => {
                state.clear_prompt_selection();
                state.move_prompt_cursor(1, state.prompt_field_width);
            }
            Focus::Workspace if state.is_typing_path() => state.move_workspace_path_cursor(1),
            Focus::Workspace | Focus::Agent | Focus::Model => {
                state.cycle_focused_dropdown(1);
            }
            Focus::ForegroundButton => state.set_focus(Focus::BackgroundButton),
            _ => {}
        },
        KeyCode::Enter if state.focus == Focus::Prompt && has_paste_modifier(key.modifiers) => {
            if state.launch(config, LaunchMode::Open)? == LaunchOutcome::Opened {
                return Ok(NewSessionAction::Launched);
            }
        }
        KeyCode::Enter if state.focus == Focus::Prompt => {
            state.insert_prompt_char('\n', state.prompt_field_width);
        }
        KeyCode::Enter => match state.focus {
            Focus::Agent => {
                if let Some(action) = state.try_launch_console_foreground(config)? {
                    return Ok(action);
                }
                state.confirm_agent_enter();
                if state.selected_agent().id == "console" {
                    state.set_focus(Focus::Workspace);
                } else {
                    state.set_focus(Focus::Model);
                }
            }
            Focus::Model => {
                state.confirm_model_enter();
                state.set_focus(Focus::Workspace);
            }
            Focus::Workspace => {
                if state.confirm_workspace_enter() {
                    if state.selected_agent().id == "console" {
                        state.set_focus(Focus::ForegroundButton);
                    } else {
                        state.focus_prompt_from_keyboard(state.prompt_field_width);
                    }
                }
            }
            Focus::Prompt => {}
            Focus::ForegroundButton => {
                if state.launch(config, LaunchMode::Open)? == LaunchOutcome::Opened {
                    return Ok(NewSessionAction::Launched);
                }
            }
            Focus::BackgroundButton => {
                let _ = state.launch(config, LaunchMode::Background)?;
            }
        },
        _ => {}
    }
    Ok(NewSessionAction::Unchanged)
}

pub fn handle_mouse_event(
    mouse: MouseEvent,
    targets: &ClickTargets,
    panel_hover: &mut PanelHover,
    state: &mut NewSessionState,
    config: &Config,
) -> Result<NewSessionAction> {
    handle_mouse(mouse, targets, panel_hover, state, config)
}
const DRAFT_AUTOSAVE_INTERVAL: Duration = Duration::from_secs(2);

pub(in crate::bar::overlay::new_session) fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &Config,
    state: &mut NewSessionState,
    panel_hover: &mut PanelHover,
) -> Result<NewSessionAction> {
    let mut click_targets = ClickTargets::default();
    let mut last_draft_save = Instant::now();
    loop {
        terminal.draw(|frame| {
            click_targets = draw_screen(frame, state, panel_hover);
        })?;

        if last_draft_save.elapsed() >= DRAFT_AUTOSAVE_INTERVAL {
            let _ = state.save_draft(config);
            last_draft_save = Instant::now();
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Paste(text) => {
                    apply_paste(state, &text);
                }
                Event::Key(key) => match handle_key(state, config, key)? {
                    NewSessionAction::Close => {
                        let _ = state.save_draft(config);
                        return Ok(NewSessionAction::Close);
                    }
                    NewSessionAction::Launched => return Ok(NewSessionAction::Launched),
                    NewSessionAction::Unchanged => {}
                },
                Event::Mouse(mouse) => {
                    match handle_mouse_event(mouse, &click_targets, panel_hover, state, config)? {
                        NewSessionAction::Close => {
                            let _ = state.save_draft(config);
                            return Ok(NewSessionAction::Close);
                        }
                        NewSessionAction::Launched => return Ok(NewSessionAction::Launched),
                        NewSessionAction::Unchanged => {}
                    }
                }
                Event::Resize(_, _) => continue,
                _ => {}
            }
        }
    }
}

pub(crate) fn handle_mouse(
    mouse: MouseEvent,
    targets: &ClickTargets,
    panel_hover: &mut PanelHover,
    state: &mut NewSessionState,
    config: &Config,
) -> Result<NewSessionAction> {
    let col = mouse.column;
    let row = mouse.row;

    match mouse.kind {
        MouseEventKind::Moved => {
            let on_foreground = point_in_rect(col, row, targets.foreground_button);
            let on_background = point_in_rect(col, row, targets.background_button);
            panel_hover.foreground_button = on_foreground;
            panel_hover.background_button = on_background;
            panel_hover.close = point_in_rect(col, row, targets.close);
            panel_hover.workspace_popup_row = if point_in_rect(col, row, targets.workspace_popup) {
                workspace_popup_row_from_mouse(
                    targets.workspace_popup,
                    col,
                    row,
                    &state.build_workspace_popup(),
                    state.workspace_popup_highlight,
                )
            } else {
                None
            };
            if state.prompt_drag_selecting {
                state.update_prompt_drag_selection(
                    targets.prompt_field,
                    col,
                    row,
                    state.prompt_field_width,
                );
            }
            sync_panel_mouse_cursor(panel_hover, col, row, targets);
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if state.prompt_drag_selecting {
                state.update_prompt_drag_selection(
                    targets.prompt_field,
                    col,
                    row,
                    state.prompt_field_width,
                );
            }
        }
        MouseEventKind::ScrollUp => {
            if point_in_prompt(col, row, targets) {
                state.set_focus(Focus::Prompt);
                state.scroll_prompt_lines(-1, state.prompt_field_width);
            } else if point_in_rect(col, row, targets.workspace)
                || point_in_rect(col, row, targets.workspace_popup)
            {
                state.focus_workspace();
                state.cycle_workspace_popup(-1);
            } else if point_in_rect(col, row, targets.agent)
                || point_in_rect(col, row, targets.agent_popup)
            {
                state.set_focus(Focus::Agent);
                state.cycle_agent(-1);
            } else if point_in_rect(col, row, targets.model)
                || point_in_rect(col, row, targets.model_popup)
            {
                state.set_focus(Focus::Model);
                state.cycle_model(-1);
            }
        }
        MouseEventKind::ScrollDown => {
            if point_in_prompt(col, row, targets) {
                state.set_focus(Focus::Prompt);
                state.scroll_prompt_lines(1, state.prompt_field_width);
            } else if point_in_rect(col, row, targets.workspace)
                || point_in_rect(col, row, targets.workspace_popup)
            {
                state.focus_workspace();
                state.cycle_workspace_popup(1);
            } else if point_in_rect(col, row, targets.agent)
                || point_in_rect(col, row, targets.agent_popup)
            {
                state.set_focus(Focus::Agent);
                state.cycle_agent(1);
            } else if point_in_rect(col, row, targets.model)
                || point_in_rect(col, row, targets.model_popup)
            {
                state.set_focus(Focus::Model);
                state.cycle_model(1);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if point_in_rect(col, row, targets.close) {
                return Ok(NewSessionAction::Close);
            }
            if point_in_rect(col, row, targets.foreground_button) {
                state.set_focus(Focus::ForegroundButton);
                if state.launch(config, LaunchMode::Open)? == LaunchOutcome::Opened {
                    return Ok(NewSessionAction::Launched);
                }
            } else if point_in_rect(col, row, targets.background_button) {
                state.set_focus(Focus::BackgroundButton);
                let _ = state.launch(config, LaunchMode::Background)?;
            } else if point_in_rect(col, row, targets.workspace_popup) {
                state.focus_workspace();
                if workspace_popup_header_click(targets.workspace_popup, col, row) {
                    state.begin_workspace_path_edit();
                } else if let Some(idx) = workspace_popup_click_index(
                    targets.workspace_popup,
                    col,
                    row,
                    &state.build_workspace_popup(),
                    state.workspace_popup_highlight,
                ) {
                    state.workspace_popup_highlight = idx;
                    state.apply_workspace_popup_selection();
                    state.confirm_workspace_selection(false);
                }
            } else if point_in_rect(col, row, targets.workspace)
                || point_in_rect(col, row, targets.workspace_field)
            {
                state.focus_workspace();
                state.begin_workspace_path_edit();
            } else if point_in_rect(col, row, targets.agent_popup) {
                state.set_focus(Focus::Agent);
                if let Some(idx) = dropdown_popup_click_index(
                    targets.agent_popup,
                    col,
                    row,
                    agents::AGENTS.len(),
                    state.agent_idx,
                ) {
                    state.agent_idx = idx;
                    state.sync_model_idx();
                    state.agent_confirmed = true;
                    if state.selected_agent().id == "console"
                        && matches!(state.focus, Focus::Model | Focus::Prompt)
                    {
                        state.set_focus(Focus::ForegroundButton);
                    }
                }
            } else if point_in_rect(col, row, targets.agent)
                || point_in_rect(col, row, targets.agent_field)
            {
                state.set_focus(Focus::Agent);
            } else if point_in_rect(col, row, targets.model_popup) {
                state.set_focus(Focus::Model);
                if let Some(idx) = dropdown_popup_click_index(
                    targets.model_popup,
                    col,
                    row,
                    state.model_count(),
                    state.model_idx,
                ) {
                    state.model_idx = idx;
                    state.model_confirmed = true;
                }
            } else if point_in_rect(col, row, targets.model)
                || point_in_rect(col, row, targets.model_field)
            {
                state.set_focus(Focus::Model);
            } else if point_in_rect(col, row, targets.prompt_field) {
                state.handle_prompt_body_click(
                    targets.prompt_field,
                    col,
                    row,
                    state.prompt_field_width,
                );
            } else if point_in_rect(col, row, targets.prompt) {
                state.focus_prompt_from_keyboard(state.prompt_field_width);
            }
        }
        MouseEventKind::Up(MouseButton::Left) if state.prompt_drag_selecting => {
            state.finish_prompt_drag_selection();
        }
        _ => {}
    }
    Ok(NewSessionAction::Unchanged)
}
