//! Keyboard + mouse handling for the automations panel.

use super::render::{dropdown_click_index, ClickTargets};
use super::state::{
    AutomationsAction, AutomationsState, EditorFocus, ListFilter, Mode, PanelHover,
};
use crate::bar::path_picker::{PathPopupEntry, PathPopupKind, HEADER_ROWS};
use crate::bar::settings::point_in_rect;
use crate::config::Config;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

pub fn handle_key(
    state: &mut AutomationsState,
    config: &Config,
    key: KeyEvent,
) -> Result<AutomationsAction> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        return Ok(AutomationsAction::Close);
    }

    match state.mode {
        Mode::List => handle_list(state, config, key),
        Mode::Editor => handle_editor(state, config, key),
    }
}

pub fn handle_mouse(
    state: &mut AutomationsState,
    config: &Config,
    mouse: MouseEvent,
    targets: &ClickTargets,
    hover: &mut PanelHover,
) -> Result<AutomationsAction> {
    update_hover(state, mouse, targets, hover);

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            handle_click(state, config, mouse.column, mouse.row, targets)
        }
        MouseEventKind::ScrollDown => {
            if state.mode == Mode::List {
                let max = state.list_len().saturating_sub(1);
                if state.selected < max {
                    state.selected += 1;
                }
            } else if state.dropdown_open {
                nudge_dropdown(state, 1);
            } else if state.editor_focus == EditorFocus::Prompt {
                state.prompt_scroll = state.prompt_scroll.saturating_add(1);
            }
            Ok(AutomationsAction::Unchanged)
        }
        MouseEventKind::ScrollUp => {
            if state.mode == Mode::List {
                state.selected = state.selected.saturating_sub(1);
            } else if state.dropdown_open {
                nudge_dropdown(state, -1);
            } else if state.editor_focus == EditorFocus::Prompt {
                state.prompt_scroll = state.prompt_scroll.saturating_sub(1);
            }
            Ok(AutomationsAction::Unchanged)
        }
        _ => Ok(AutomationsAction::Unchanged),
    }
}

fn nudge_dropdown(state: &mut AutomationsState, delta: i32) {
    match state.editor_focus {
        EditorFocus::Cwd => {
            state.path.cycle(delta);
        }
        EditorFocus::Agent => {
            let n = state.agent_choices().len() as i32;
            if n == 0 {
                return;
            }
            let next = (state.agent_idx as i32 + delta).clamp(0, n - 1);
            state.agent_idx = next as usize;
            state.model_idx = 0;
        }
        EditorFocus::Model => {
            let n = state.selected_agent().models.len() as i32;
            if n == 0 {
                return;
            }
            let next = (state.model_idx as i32 + delta).clamp(0, n - 1);
            state.model_idx = next as usize;
        }
        EditorFocus::Schedule => {
            let n = AutomationsState::schedule_presets().len() as i32;
            let next = (state.schedule_idx as i32 + delta).clamp(0, n - 1);
            state.schedule_idx = next as usize;
        }
        _ => {}
    }
}

fn update_hover(
    state: &AutomationsState,
    mouse: MouseEvent,
    targets: &ClickTargets,
    hover: &mut PanelHover,
) {
    *hover = PanelHover::default();
    let (c, r) = (mouse.column, mouse.row);
    hover.close = point_in_rect(c, r, targets.close);
    if state.mode == Mode::List {
        for (f, rect) in &targets.filters {
            if point_in_rect(c, r, *rect) {
                hover.filter = Some(*f);
            }
        }
        for (i, rect) in targets.rows.iter().enumerate() {
            if point_in_rect(c, r, *rect) {
                hover.row = Some(state.list_scroll + i);
            }
        }
        hover.new_btn = point_in_rect(c, r, targets.new_btn);
        hover.run_btn = point_in_rect(c, r, targets.run_btn);
        hover.pause_btn = point_in_rect(c, r, targets.pause_btn);
        hover.edit_btn = point_in_rect(c, r, targets.edit_btn);
    } else {
        hover.save_btn = point_in_rect(c, r, targets.save_btn);
        hover.save_run_btn = point_in_rect(c, r, targets.save_run_btn);
        hover.cancel_btn = point_in_rect(c, r, targets.cancel_btn);
        if state.dropdown_open {
            let (popup, count, selected) = match state.editor_focus {
                EditorFocus::Cwd => {
                    let entries = state.path.build_popup();
                    (targets.cwd_popup, entries.len(), state.path.highlight)
                }
                EditorFocus::Agent => (
                    targets.agent_popup,
                    state.agent_choices().len(),
                    state.agent_idx,
                ),
                EditorFocus::Model => (
                    targets.model_popup,
                    state.selected_agent().models.len(),
                    state.model_idx,
                ),
                EditorFocus::Schedule => (
                    targets.schedule_popup,
                    AutomationsState::schedule_presets().len(),
                    state.schedule_idx,
                ),
                _ => (ratatui::layout::Rect::default(), 0, 0),
            };
            if state.editor_focus == EditorFocus::Cwd {
                hover.dropdown_item = path_popup_click_index(
                    popup,
                    c,
                    r,
                    &state.path.build_popup(),
                    state.path.highlight,
                );
            } else {
                hover.dropdown_item = dropdown_click_index(popup, c, r, count, selected);
            }
        }
    }
}

fn handle_click(
    state: &mut AutomationsState,
    config: &Config,
    col: u16,
    row: u16,
    targets: &ClickTargets,
) -> Result<AutomationsAction> {
    if point_in_rect(col, row, targets.close) {
        return if state.mode == Mode::Editor {
            state.cancel_editor();
            Ok(AutomationsAction::Unchanged)
        } else {
            Ok(AutomationsAction::Close)
        };
    }

    match state.mode {
        Mode::List => {
            for (f, rect) in &targets.filters {
                if point_in_rect(col, row, *rect) {
                    state.filter = *f;
                    state.selected = 0;
                    state.list_scroll = 0;
                    return Ok(AutomationsAction::Unchanged);
                }
            }
            for (i, rect) in targets.rows.iter().enumerate() {
                if point_in_rect(col, row, *rect) {
                    state.selected = state.list_scroll + i;
                    if state.filter != ListFilter::Runs {
                        if let Some(a) = state
                            .filtered_items()
                            .get(state.selected)
                            .map(|a| (*a).clone())
                        {
                            state.open_edit(&a);
                        }
                    } else if let Some(run) = state.runs.get(state.selected) {
                        if let Some(wi) = run.window_index {
                            let _ = crate::daemon::tmux::select_window(&config.tmux_session, wi);
                            let _ = crate::daemon::tmux::restore_workspace_attach(
                                &config.tmux_ui_session,
                                &config.tmux_session,
                            );
                            return Ok(AutomationsAction::Close);
                        }
                    }
                    return Ok(AutomationsAction::Unchanged);
                }
            }
            if point_in_rect(col, row, targets.new_btn) {
                state.open_create();
            } else if point_in_rect(col, row, targets.run_btn) {
                state.run_selected(config)?;
            } else if point_in_rect(col, row, targets.pause_btn) {
                state.toggle_pause(config)?;
            } else if point_in_rect(col, row, targets.edit_btn) && state.filter != ListFilter::Runs
            {
                if let Some(a) = state
                    .filtered_items()
                    .get(state.selected)
                    .map(|a| (*a).clone())
                {
                    state.open_edit(&a);
                }
            }
        }
        Mode::Editor => {
            if state.dropdown_open {
                match state.editor_focus {
                    EditorFocus::Cwd => {
                        let entries = state.path.build_popup();
                        if path_popup_header_click(targets.cwd_popup, col, row) {
                            state.path.begin_edit();
                            return Ok(AutomationsAction::Unchanged);
                        }
                        if let Some(idx) = path_popup_click_index(
                            targets.cwd_popup,
                            col,
                            row,
                            &entries,
                            state.path.highlight,
                        ) {
                            state.path.highlight = idx;
                            let _ = state.confirm_path_selection();
                            return Ok(AutomationsAction::Unchanged);
                        }
                    }
                    EditorFocus::Agent => {
                        if let Some(idx) = dropdown_click_index(
                            targets.agent_popup,
                            col,
                            row,
                            state.agent_choices().len(),
                            state.agent_idx,
                        ) {
                            state.agent_idx = idx;
                            state.model_idx = 0;
                            state.dropdown_open = false;
                            return Ok(AutomationsAction::Unchanged);
                        }
                    }
                    EditorFocus::Model => {
                        let n = state.selected_agent().models.len();
                        if let Some(idx) =
                            dropdown_click_index(targets.model_popup, col, row, n, state.model_idx)
                        {
                            state.model_idx = idx;
                            state.dropdown_open = false;
                            return Ok(AutomationsAction::Unchanged);
                        }
                    }
                    EditorFocus::Schedule => {
                        let n = AutomationsState::schedule_presets().len();
                        if let Some(idx) = dropdown_click_index(
                            targets.schedule_popup,
                            col,
                            row,
                            n,
                            state.schedule_idx,
                        ) {
                            state.schedule_idx = idx;
                            state.dropdown_open = false;
                            return Ok(AutomationsAction::Unchanged);
                        }
                    }
                    _ => {}
                }
            }

            if point_in_rect(col, row, targets.name_field) {
                state.set_focus(EditorFocus::Name);
            } else if point_in_rect(col, row, targets.cwd_field)
                || point_in_rect(col, row, targets.cwd_popup)
            {
                state.set_focus(EditorFocus::Cwd);
                state.dropdown_open = true;
                state.path.open_menu();
            } else if point_in_rect(col, row, targets.agent_field)
                || point_in_rect(col, row, targets.agent_popup)
            {
                state.editor_focus = EditorFocus::Agent;
                state.dropdown_open = true;
            } else if point_in_rect(col, row, targets.model_field)
                || point_in_rect(col, row, targets.model_popup)
            {
                state.editor_focus = EditorFocus::Model;
                state.dropdown_open = true;
            } else if point_in_rect(col, row, targets.schedule_field)
                || point_in_rect(col, row, targets.schedule_popup)
            {
                state.editor_focus = EditorFocus::Schedule;
                state.dropdown_open = true;
            } else if point_in_rect(col, row, targets.prompt_field) {
                state.set_focus(EditorFocus::Prompt);
            } else if point_in_rect(col, row, targets.save_btn) {
                state.save(config, false)?;
            } else if point_in_rect(col, row, targets.save_run_btn) {
                state.save(config, true)?;
            } else if point_in_rect(col, row, targets.cancel_btn) {
                state.cancel_editor();
            } else {
                state.dropdown_open = false;
            }
        }
    }
    Ok(AutomationsAction::Unchanged)
}

fn handle_list(
    state: &mut AutomationsState,
    config: &Config,
    key: KeyEvent,
) -> Result<AutomationsAction> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return Ok(AutomationsAction::Close),
        KeyCode::Char('n') => state.open_create(),
        KeyCode::Char('e') => {
            if state.filter != ListFilter::Runs {
                if let Some(a) = state
                    .filtered_items()
                    .get(state.selected)
                    .map(|a| (*a).clone())
                {
                    state.open_edit(&a);
                }
            }
        }
        KeyCode::Char('r') => state.run_selected(config)?,
        KeyCode::Char('p') => state.toggle_pause(config)?,
        KeyCode::Char('d')
            if key.modifiers.contains(KeyModifiers::SHIFT)
                || key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            state.delete_selected(config)?;
        }
        KeyCode::Char('m') => state.mark_all_read(config)?,
        KeyCode::Tab => {
            state.filter = state.filter.cycle();
            state.selected = 0;
            state.list_scroll = 0;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.selected = state.selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = state.list_len().saturating_sub(1);
            if state.selected < max {
                state.selected += 1;
            }
        }
        KeyCode::Enter => {
            if state.filter == ListFilter::Runs {
                if let Some(run) = state.runs.get(state.selected) {
                    if let Some(wi) = run.window_index {
                        let _ = crate::daemon::tmux::select_window(&config.tmux_session, wi);
                        let _ = crate::daemon::tmux::restore_workspace_attach(
                            &config.tmux_ui_session,
                            &config.tmux_session,
                        );
                        return Ok(AutomationsAction::Close);
                    }
                    state.status = "run has no live window".into();
                }
            } else if let Some(a) = state
                .filtered_items()
                .get(state.selected)
                .map(|a| (*a).clone())
            {
                state.open_edit(&a);
            }
        }
        KeyCode::Char('1') => {
            state.filter = ListFilter::All;
            state.selected = 0;
            state.list_scroll = 0;
        }
        KeyCode::Char('2') => {
            state.filter = ListFilter::Active;
            state.selected = 0;
            state.list_scroll = 0;
        }
        KeyCode::Char('3') => {
            state.filter = ListFilter::Paused;
            state.selected = 0;
            state.list_scroll = 0;
        }
        KeyCode::Char('4') => {
            state.filter = ListFilter::Runs;
            state.selected = 0;
            state.list_scroll = 0;
        }
        _ => {}
    }
    Ok(AutomationsAction::Unchanged)
}

fn handle_editor(
    state: &mut AutomationsState,
    config: &Config,
    key: KeyEvent,
) -> Result<AutomationsAction> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('s')) {
        state.save(config, false)?;
        return Ok(AutomationsAction::Unchanged);
    }

    if state.dropdown_open && state.editor_focus.is_dropdown() {
        if state.editor_focus == EditorFocus::Cwd {
            return handle_path_dropdown(state, key);
        }
        match key.code {
            KeyCode::Esc => {
                state.dropdown_open = false;
                return Ok(AutomationsAction::Unchanged);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                nudge_dropdown(state, -1);
                return Ok(AutomationsAction::Unchanged);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                nudge_dropdown(state, 1);
                return Ok(AutomationsAction::Unchanged);
            }
            KeyCode::Enter => {
                state.dropdown_open = false;
                state.editor_focus = next_focus(state.editor_focus, false);
                return Ok(AutomationsAction::Unchanged);
            }
            _ => return Ok(AutomationsAction::Unchanged),
        }
    }

    if key.code == KeyCode::Esc {
        state.cancel_editor();
        return Ok(AutomationsAction::Unchanged);
    }

    if key.code == KeyCode::Tab {
        if state.editor_focus == EditorFocus::Cwd && !key.modifiers.contains(KeyModifiers::SHIFT) {
            // Tab on path field: complete or open menu / cycle
            if !state.dropdown_open {
                state.dropdown_open = true;
                state.path.open_menu();
            }
            if state.path.tab_complete() {
                return Ok(AutomationsAction::Unchanged);
            }
            state.path.cycle(1);
            return Ok(AutomationsAction::Unchanged);
        }
        state.set_focus(next_focus(
            state.editor_focus,
            key.modifiers.contains(KeyModifiers::SHIFT),
        ));
        return Ok(AutomationsAction::Unchanged);
    }
    if key.code == KeyCode::Up && !matches!(state.editor_focus, EditorFocus::Prompt) {
        state.set_focus(next_focus(state.editor_focus, true));
        return Ok(AutomationsAction::Unchanged);
    }
    if key.code == KeyCode::Down && !matches!(state.editor_focus, EditorFocus::Prompt) {
        state.set_focus(next_focus(state.editor_focus, false));
        return Ok(AutomationsAction::Unchanged);
    }

    match state.editor_focus {
        EditorFocus::Name => edit_line(&mut state.name, &mut state.name_cursor, key),
        EditorFocus::Cwd => match key.code {
            KeyCode::Enter => {
                if state.dropdown_open {
                    if state.confirm_path_selection() {
                        state.editor_focus = next_focus(EditorFocus::Cwd, false);
                    }
                } else {
                    state.dropdown_open = true;
                    state.path.open_menu();
                }
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.path.insert_char(ch);
                state.dropdown_open = true;
            }
            KeyCode::Backspace => {
                state.path.backspace();
                state.dropdown_open = true;
            }
            KeyCode::Delete => {
                state.path.forward_delete();
                state.dropdown_open = true;
            }
            KeyCode::Left => state.path.move_cursor(-1),
            KeyCode::Right => state.path.move_cursor(1),
            _ => {}
        },
        EditorFocus::Agent | EditorFocus::Model | EditorFocus::Schedule => {
            if key.code == KeyCode::Enter {
                state.dropdown_open = true;
            }
        }
        EditorFocus::Prompt => edit_prompt(state, key),
        EditorFocus::Save => {
            if key.code == KeyCode::Enter {
                state.save(config, false)?;
            }
        }
        EditorFocus::SaveRun => {
            if key.code == KeyCode::Enter {
                state.save(config, true)?;
            }
        }
        EditorFocus::Cancel => {
            if key.code == KeyCode::Enter {
                state.cancel_editor();
            }
        }
    }

    Ok(AutomationsAction::Unchanged)
}

fn next_focus(current: EditorFocus, reverse: bool) -> EditorFocus {
    let order = [
        EditorFocus::Name,
        EditorFocus::Cwd,
        EditorFocus::Agent,
        EditorFocus::Model,
        EditorFocus::Schedule,
        EditorFocus::Prompt,
        EditorFocus::Save,
        EditorFocus::SaveRun,
        EditorFocus::Cancel,
    ];
    let idx = order.iter().position(|f| *f == current).unwrap_or(0);
    if reverse {
        order[(idx + order.len() - 1) % order.len()]
    } else {
        order[(idx + 1) % order.len()]
    }
}

fn edit_line(buf: &mut String, cursor: &mut usize, key: KeyEvent) {
    *cursor = (*cursor).min(buf.len());
    match key.code {
        KeyCode::Backspace => {
            if *cursor > 0 {
                let mut c = *cursor;
                while c > 0 && !buf.is_char_boundary(c - 1) {
                    c -= 1;
                }
                if c > 0 {
                    buf.remove(c - 1);
                    *cursor = c - 1;
                }
            }
        }
        KeyCode::Left => {
            if *cursor > 0 {
                let mut c = *cursor - 1;
                while c > 0 && !buf.is_char_boundary(c) {
                    c -= 1;
                }
                *cursor = c;
            }
        }
        KeyCode::Right => {
            if *cursor < buf.len() {
                let mut c = *cursor + 1;
                while c < buf.len() && !buf.is_char_boundary(c) {
                    c += 1;
                }
                *cursor = c;
            }
        }
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = buf.len(),
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            buf.insert(*cursor, ch);
            *cursor += ch.len_utf8();
        }
        _ => {}
    }
}

fn edit_prompt(state: &mut AutomationsState, key: KeyEvent) {
    match key.code {
        KeyCode::Backspace => {
            if state.prompt_cursor > 0 {
                let mut c = state.prompt_cursor;
                while c > 0 && !state.prompt.is_char_boundary(c - 1) {
                    c -= 1;
                }
                if c > 0 {
                    state.prompt.remove(c - 1);
                    state.prompt_cursor = c - 1;
                }
            }
        }
        KeyCode::Left => {
            if state.prompt_cursor > 0 {
                let mut c = state.prompt_cursor - 1;
                while c > 0 && !state.prompt.is_char_boundary(c) {
                    c -= 1;
                }
                state.prompt_cursor = c;
            }
        }
        KeyCode::Right => {
            if state.prompt_cursor < state.prompt.len() {
                let mut c = state.prompt_cursor + 1;
                while c < state.prompt.len() && !state.prompt.is_char_boundary(c) {
                    c += 1;
                }
                state.prompt_cursor = c;
            }
        }
        KeyCode::Up => {
            state.prompt_scroll = state.prompt_scroll.saturating_sub(1);
        }
        KeyCode::Down => {
            state.prompt_scroll = state.prompt_scroll.saturating_add(1);
        }
        KeyCode::Enter => {
            state.prompt.insert(state.prompt_cursor, '\n');
            state.prompt_cursor += 1;
        }
        KeyCode::Home => {
            let before = &state.prompt[..state.prompt_cursor];
            state.prompt_cursor = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        }
        KeyCode::End => {
            let after = &state.prompt[state.prompt_cursor..];
            if let Some(i) = after.find('\n') {
                state.prompt_cursor += i;
            } else {
                state.prompt_cursor = state.prompt.len();
            }
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.prompt.insert(state.prompt_cursor, c);
            state.prompt_cursor += c.len_utf8();
        }
        _ => {}
    }
}

fn handle_path_dropdown(state: &mut AutomationsState, key: KeyEvent) -> Result<AutomationsAction> {
    match key.code {
        KeyCode::Esc => {
            state.dropdown_open = false;
            state.path.close_menu();
        }
        KeyCode::Up => state.path.cycle(-1),
        KeyCode::Down => state.path.cycle(1),
        KeyCode::Char('k') if !state.path.is_typing() => state.path.cycle(-1),
        KeyCode::Char('j') if !state.path.is_typing() => state.path.cycle(1),
        KeyCode::Enter => {
            if state.confirm_path_selection() {
                state.editor_focus = next_focus(EditorFocus::Cwd, false);
            }
        }
        KeyCode::Tab => {
            if !state.path.tab_complete() {
                state.path.cycle(1);
            }
        }
        KeyCode::Left => state.path.move_cursor(-1),
        KeyCode::Right => state.path.move_cursor(1),
        KeyCode::Backspace => state.path.backspace(),
        KeyCode::Delete => state.path.forward_delete(),
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.path.insert_char(ch);
        }
        _ => {}
    }
    Ok(AutomationsAction::Unchanged)
}

fn path_popup_header_click(popup: Rect, col: u16, row: u16) -> bool {
    if popup.width == 0 || popup.height == 0 {
        return false;
    }
    // Inner header row (below top border)
    let header_y = popup.y.saturating_add(1);
    row == header_y && col >= popup.x && col < popup.x.saturating_add(popup.width)
}

fn path_list_window(count: usize, selected: usize, max_visible: usize) -> (usize, usize) {
    if count == 0 || max_visible == 0 {
        return (0, 0);
    }
    let visible = count.min(max_visible);
    if count <= visible {
        return (0, count);
    }
    let start = selected
        .saturating_sub(visible / 2)
        .min(count.saturating_sub(visible));
    (start, visible)
}

fn path_popup_click_index(
    popup: Rect,
    col: u16,
    row: u16,
    entries: &[PathPopupEntry],
    highlight: usize,
) -> Option<usize> {
    if popup.width == 0 || popup.height < 3 || !point_in_rect(col, row, popup) {
        return None;
    }
    // list starts after top border + header row
    let list_top = popup.y.saturating_add(1 + HEADER_ROWS);
    if row < list_top {
        return None;
    }
    let list_row = (row - list_top) as usize;
    let max_visible = popup.height.saturating_sub(2 + HEADER_ROWS) as usize;
    let (start, visible) = path_list_window(entries.len(), highlight, max_visible);
    if list_row >= visible {
        return None;
    }
    let idx = start + list_row;
    entries.get(idx).and_then(|e| {
        if e.kind == PathPopupKind::Section {
            None
        } else {
            Some(idx)
        }
    })
}
