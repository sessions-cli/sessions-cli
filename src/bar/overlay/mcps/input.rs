//! Keyboard + mouse handling for the MCP management panel.

use super::render::ClickTargets;
use super::state::{ActionButton, FocusZone, McpsAction, McpsState, PanelHover};
use crate::bar::settings::point_in_rect;
use crate::config::Config;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

pub fn handle_key(state: &mut McpsState, config: &Config, key: KeyEvent) -> Result<McpsAction> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        return Ok(McpsAction::Close);
    }

    // Setup dialog captures input first.
    if state.setup.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') if key.modifiers.is_empty() => {
                let _ = state.handle_setup_esc(config);
                return Ok(McpsAction::Unchanged);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let _ = state.handle_setup_enter(config);
                return Ok(McpsAction::Unchanged);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(s) = state.setup.as_mut() {
                    s.scroll = s.scroll.saturating_sub(1);
                }
                return Ok(McpsAction::Unchanged);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(s) = state.setup.as_mut() {
                    s.scroll = s.scroll.saturating_add(1);
                }
                return Ok(McpsAction::Unchanged);
            }
            _ => return Ok(McpsAction::Unchanged),
        }
    }

    // Search mode captures typing.
    if state.search_open {
        return handle_search_key(state, config, key);
    }

    match key.code {
        KeyCode::Esc => return Ok(McpsAction::Close),
        KeyCode::Char('q') if key.modifiers.is_empty() => return Ok(McpsAction::Close),
        KeyCode::Char('/') if key.modifiers.is_empty() => {
            state.open_search(config);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Tab => {
            state.cycle_focus();
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::BackTab => {
            state.focus = match state.focus {
                FocusZone::Table => FocusZone::Actions,
                FocusZone::Drift => FocusZone::Table,
                FocusZone::Actions => FocusZone::Drift,
                FocusZone::Search => FocusZone::Table,
            };
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char('o') if key.modifiers.is_empty() => {
            state.open_obot(config);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char('r') if key.modifiers.is_empty() => {
            state.activate_action(ActionButton::Refresh, config);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char('s') if key.modifiers.is_empty() => {
            state.run_sync(config, false);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char('d') if key.modifiers.is_empty() => {
            state.run_sync(config, true);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char('?') if key.modifiers.is_empty() => {
            state.status =
                "Keys: / search · j/k move · h/l agent · space toggle · o catalog · r refresh · s sync · d dry-run · u setup · esc close"
                    .into();
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char('u') if key.modifiers.is_empty() => {
            // Re-open setup dialog even after skip.
            state.setup = Some(crate::companions::SetupDialog::prompt(
                crate::companions::CompanionKind::Obot,
            ));
            return Ok(McpsAction::Unchanged);
        }
        _ => {}
    }

    match state.focus {
        FocusZone::Table => handle_table_key(state, key),
        FocusZone::Drift => handle_drift_key(state, key),
        FocusZone::Actions => handle_actions_key(state, config, key),
        FocusZone::Search => handle_search_key(state, config, key),
    }
}

fn handle_search_key(state: &mut McpsState, config: &Config, key: KeyEvent) -> Result<McpsAction> {
    match key.code {
        KeyCode::Esc => {
            state.close_search();
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char('q') if key.modifiers.is_empty() && state.search_query.is_empty() => {
            state.close_search();
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Enter => {
            state.activate_search_selection(config);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Up => {
            state.move_search(-1);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Down => {
            state.move_search(1);
            return Ok(McpsAction::Unchanged);
        }
        // Ctrl-n / Ctrl-p for next/prev without hijacking letter keys while typing.
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_search(1);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_search(-1);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Backspace => {
            state.pop_search_char();
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.catalog_loaded = false;
            state.load_catalog(config);
            return Ok(McpsAction::Unchanged);
        }
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            // Type into query (including letters that are shortcuts outside search).
            state.push_search_char(c);
            return Ok(McpsAction::Unchanged);
        }
        _ => {}
    }
    Ok(McpsAction::Unchanged)
}

fn handle_table_key(state: &mut McpsState, key: KeyEvent) -> Result<McpsAction> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => state.move_row(-1),
        KeyCode::Down | KeyCode::Char('j') => state.move_row(1),
        KeyCode::Left | KeyCode::Char('h') => state.move_agent(-1),
        KeyCode::Right | KeyCode::Char('l') => state.move_agent(1),
        KeyCode::Char(' ') | KeyCode::Enter => state.toggle_selected_cell(),
        _ => {}
    }
    Ok(McpsAction::Unchanged)
}

fn handle_drift_key(state: &mut McpsState, key: KeyEvent) -> Result<McpsAction> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => state.move_drift(-1),
        KeyCode::Down | KeyCode::Char('j') => state.move_drift(1),
        KeyCode::Enter | KeyCode::Char(' ') => {
            // Jump to related server if we later encode row refs in drift.
        }
        _ => {}
    }
    Ok(McpsAction::Unchanged)
}

fn handle_actions_key(state: &mut McpsState, config: &Config, key: KeyEvent) -> Result<McpsAction> {
    match key.code {
        KeyCode::Left | KeyCode::Char('h') => {
            state.action_focus = state.action_focus.cycle(-1);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            state.action_focus = state.action_focus.cycle(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.action_focus = state.action_focus.cycle(-1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.action_focus = state.action_focus.cycle(1);
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            state.activate_action(state.action_focus, config);
        }
        _ => {}
    }
    Ok(McpsAction::Unchanged)
}

pub fn handle_mouse(
    state: &mut McpsState,
    config: &Config,
    mouse: MouseEvent,
    targets: &ClickTargets,
    hover: &mut PanelHover,
) -> Result<McpsAction> {
    // Setup dialog is keyboard-first; ignore mouse so clicks don't hit the panel beneath.
    if state.setup.is_some() {
        *hover = PanelHover::default();
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            // Treat click as confirm (same as Enter).
            let _ = state.handle_setup_enter(config);
        }
        return Ok(McpsAction::Unchanged);
    }

    update_hover(mouse, targets, hover);

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            handle_click(state, config, mouse.column, mouse.row, targets)
        }
        MouseEventKind::ScrollDown => {
            if state.search_open {
                state.move_search(1);
            } else if state.focus == FocusZone::Drift {
                state.move_drift(1);
            } else {
                state.focus = FocusZone::Table;
                state.move_row(1);
            }
            Ok(McpsAction::Unchanged)
        }
        MouseEventKind::ScrollUp => {
            if state.search_open {
                state.move_search(-1);
            } else if state.focus == FocusZone::Drift {
                state.move_drift(-1);
            } else {
                state.focus = FocusZone::Table;
                state.move_row(-1);
            }
            Ok(McpsAction::Unchanged)
        }
        _ => Ok(McpsAction::Unchanged),
    }
}

fn update_hover(mouse: MouseEvent, targets: &ClickTargets, hover: &mut PanelHover) {
    let c = mouse.column;
    let r = mouse.row;
    hover.close = point_in_rect(c, r, targets.close);
    hover.open_obot = point_in_rect(c, r, targets.open_obot);
    hover.search = point_in_rect(c, r, targets.search);
    hover.refresh = point_in_rect(c, r, targets.refresh);
    hover.sync_all = point_in_rect(c, r, targets.sync_all);
    hover.dry_run = point_in_rect(c, r, targets.dry_run);
    hover.row = None;
    hover.agent_col = None;
    hover.search_row = None;
    for (abs_idx, rect) in &targets.search_rows {
        if point_in_rect(c, r, *rect) {
            hover.search_row = Some(*abs_idx);
            return;
        }
    }
    for (row, col, rect) in &targets.cells {
        if point_in_rect(c, r, *rect) {
            hover.row = Some(*row);
            hover.agent_col = Some(*col);
            return;
        }
    }
    for (i, rect) in targets.rows.iter().enumerate() {
        if point_in_rect(c, r, *rect) {
            let _ = i;
            hover.row = targets
                .cells
                .iter()
                .find(|(_, _, cell)| cell.y == rect.y)
                .map(|(row, _, _)| *row)
                .or(Some(i));
            break;
        }
    }
}

fn handle_click(
    state: &mut McpsState,
    config: &Config,
    col: u16,
    row: u16,
    targets: &ClickTargets,
) -> Result<McpsAction> {
    if point_in_rect(col, row, targets.close) {
        if state.search_open {
            state.close_search();
            return Ok(McpsAction::Unchanged);
        }
        return Ok(McpsAction::Close);
    }
    if point_in_rect(col, row, targets.open_obot) {
        state.activate_action(ActionButton::OpenObot, config);
        return Ok(McpsAction::Unchanged);
    }
    if point_in_rect(col, row, targets.search) {
        state.activate_action(ActionButton::Search, config);
        return Ok(McpsAction::Unchanged);
    }
    if point_in_rect(col, row, targets.refresh) {
        state.activate_action(ActionButton::Refresh, config);
        return Ok(McpsAction::Unchanged);
    }
    if point_in_rect(col, row, targets.sync_all) {
        state.activate_action(ActionButton::SyncAll, config);
        return Ok(McpsAction::Unchanged);
    }
    if point_in_rect(col, row, targets.dry_run) {
        state.activate_action(ActionButton::DryRun, config);
        return Ok(McpsAction::Unchanged);
    }
    // Search result rows: index is absolute search_results index stored in targets.
    for (abs_idx, rect) in &targets.search_rows {
        if point_in_rect(col, row, *rect) {
            state.search_selected = *abs_idx;
            state.activate_search_selection(config);
            return Ok(McpsAction::Unchanged);
        }
    }
    for (r, c, rect) in &targets.cells {
        if point_in_rect(col, row, *rect) {
            state.toggle_cell(*r, *c);
            return Ok(McpsAction::Unchanged);
        }
    }
    for rect in &targets.rows {
        if point_in_rect(col, row, *rect) {
            if let Some((r, _, _)) = targets.cells.iter().find(|(_, _, cell)| cell.y == rect.y) {
                state.selected_row = *r;
                state.focus = FocusZone::Table;
            }
            return Ok(McpsAction::Unchanged);
        }
    }
    Ok(McpsAction::Unchanged)
}
