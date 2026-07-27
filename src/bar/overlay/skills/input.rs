//! Keyboard + mouse for the skills panel.

use super::render::ClickTargets;
use super::state::{ActionId, FocusSection, PanelHover, SkillsAction, SkillsState};
use crate::bar::mouse_cursor::{self, MouseCursorShape};
use crate::bar::settings::point_in_rect;
use crate::config::Config;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

pub fn handle_key(state: &mut SkillsState, config: &Config, key: KeyEvent) -> Result<SkillsAction> {
    // Match MCPs: exit cleanly and let the workspace wrapper re-attach.
    // Do not call restore_workspace_attach here — that respawn-pane -k races the process.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        return Ok(SkillsAction::Close);
    }

    if state.setup.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') if key.modifiers.is_empty() => {
                let _ = state.handle_setup_esc(config);
                return Ok(SkillsAction::Unchanged);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let _ = state.handle_setup_enter(config);
                return Ok(SkillsAction::Unchanged);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(s) = state.setup.as_mut() {
                    s.scroll = s.scroll.saturating_sub(1);
                }
                return Ok(SkillsAction::Unchanged);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(s) = state.setup.as_mut() {
                    s.scroll = s.scroll.saturating_add(1);
                }
                return Ok(SkillsAction::Unchanged);
            }
            _ => return Ok(SkillsAction::Unchanged),
        }
    }

    match key.code {
        KeyCode::Esc => return Ok(SkillsAction::Close),
        KeyCode::Char('q') if key.modifiers.is_empty() => return Ok(SkillsAction::Close),
        KeyCode::Tab => {
            state.focus = match state.focus {
                FocusSection::Actions => FocusSection::Library,
                FocusSection::Library => FocusSection::Drift,
                FocusSection::Drift => FocusSection::Actions,
            };
        }
        KeyCode::BackTab => {
            state.focus = match state.focus {
                FocusSection::Actions => FocusSection::Drift,
                FocusSection::Library => FocusSection::Actions,
                FocusSection::Drift => FocusSection::Library,
            };
        }
        KeyCode::Up | KeyCode::Char('k') if key.modifiers == KeyModifiers::NONE => {
            match state.focus {
                FocusSection::Actions => {
                    if state.action_idx > 0 {
                        state.action_idx -= 1;
                    }
                }
                FocusSection::Library => {
                    if state.selected > 0 {
                        state.selected -= 1;
                        ensure_visible(state);
                    }
                }
                FocusSection::Drift => {}
            }
        }
        KeyCode::Down | KeyCode::Char('j') if key.modifiers == KeyModifiers::NONE => {
            match state.focus {
                FocusSection::Actions => {
                    if state.action_idx + 1 < ActionId::ALL.len() {
                        state.action_idx += 1;
                    }
                }
                FocusSection::Library => {
                    let n = state.library_len();
                    if n > 0 && state.selected + 1 < n {
                        state.selected += 1;
                        ensure_visible(state);
                    }
                }
                FocusSection::Drift => {}
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if state.focus == FocusSection::Actions {
                let action = ActionId::ALL[state.action_idx];
                state.run_action(config, action);
            }
        }
        KeyCode::Char('i') if key.modifiers.is_empty() => {
            state.run_action(config, ActionId::Init);
        }
        KeyCode::Char('s') if key.modifiers.is_empty() => {
            state.run_action(config, ActionId::Sync);
        }
        KeyCode::Char('u') if key.modifiers.is_empty() => {
            state.run_action(config, ActionId::Ui);
        }
        KeyCode::Char('a') if key.modifiers.is_empty() => {
            state.run_action(config, ActionId::Audit);
        }
        KeyCode::Char('r') if key.modifiers.is_empty() => {
            state.run_action(config, ActionId::Reload);
        }
        KeyCode::Char('U') => {
            state.setup = Some(crate::companions::SetupDialog::prompt(
                crate::companions::CompanionKind::Skillshare,
            ));
        }
        _ => {}
    }
    Ok(SkillsAction::Unchanged)
}

pub fn handle_mouse(
    state: &mut SkillsState,
    config: &Config,
    mouse: MouseEvent,
    targets: &ClickTargets,
    hover: &mut PanelHover,
) -> Result<SkillsAction> {
    if state.setup.is_some() {
        *hover = PanelHover::default();
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            let _ = state.handle_setup_enter(config);
        }
        return Ok(SkillsAction::Unchanged);
    }

    *hover = PanelHover::default();
    let col = mouse.column;
    let row = mouse.row;

    if point_in_rect(col, row, targets.close) {
        hover.close = true;
    }
    for (action, rect) in &targets.actions {
        if point_in_rect(col, row, *rect) {
            hover.action = Some(*action);
        }
    }
    for (vis_idx, rect) in targets.rows.iter().enumerate() {
        if point_in_rect(col, row, *rect) {
            hover.row = Some(state.list_scroll + vis_idx);
        }
    }

    sync_mouse_cursor(hover);

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if hover.close {
                return Ok(SkillsAction::Close);
            }
            if let Some(action) = hover.action {
                state.focus = FocusSection::Actions;
                if let Some(idx) = ActionId::ALL.iter().position(|a| *a == action) {
                    state.action_idx = idx;
                }
                state.run_action(config, action);
                return Ok(SkillsAction::Unchanged);
            }
            if let Some(r) = hover.row {
                state.focus = FocusSection::Library;
                state.selected = r;
                ensure_visible(state);
            }
        }
        MouseEventKind::ScrollUp => {
            if state.selected > 0 {
                state.selected -= 1;
                ensure_visible(state);
            }
        }
        MouseEventKind::ScrollDown => {
            let n = state.library_len();
            if n > 0 && state.selected + 1 < n {
                state.selected += 1;
                ensure_visible(state);
            }
        }
        _ => {}
    }
    Ok(SkillsAction::Unchanged)
}

fn sync_mouse_cursor(hover: &PanelHover) {
    let shape = if hover.close || hover.action.is_some() {
        MouseCursorShape::Pointer
    } else {
        MouseCursorShape::Default
    };
    let _ = mouse_cursor::set_mouse_cursor(shape);
}

fn ensure_visible(state: &mut SkillsState) {
    const VIEW: usize = 8;
    if state.selected < state.list_scroll {
        state.list_scroll = state.selected;
    } else if state.selected >= state.list_scroll + VIEW {
        state.list_scroll = state.selected + 1 - VIEW;
    }
}
