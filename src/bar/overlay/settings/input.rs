//! Settings overlay input handling.

use super::point_in_rect;
use super::render::draw_screen;
use super::state::*;
use crate::bar::mouse_cursor;
use crate::config::Config;
use crate::hooks;
use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::io;
use std::sync::mpsc::Receiver;

pub(super) fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
    )?;
    Ok(Terminal::new(CrosstermBackend::new(io::stdout()))?)
}

pub(super) fn teardown_terminal() -> Result<()> {
    let _ = mouse_cursor::reset_mouse_cursor();
    crossterm::execute!(
        io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}
pub(super) fn event_loop(
    config: &Config,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    hook_summary: &mut String,
    selected: &mut usize,
    panel_hover: &mut PanelHover,
) -> Result<()> {
    let mut targets = PanelTargets::default();
    let mut overlay: Option<SettingsOverlay> = None;
    let mut overlay_rx: Option<Receiver<OverlayMsg>> = None;
    loop {
        let rows = build_rows(config, hook_summary);
        let size = terminal.size()?;
        let pane = Rect::new(0, 0, size.width, size.height);
        if let (Some(overlay_state), Some(rx)) = (overlay.as_mut(), overlay_rx.as_ref()) {
            let was_running = overlay_state.running;
            drain_overlay(rx, overlay_state, pane);
            if was_running && overlay_state.finished && overlay_state.title == "Agent hooks" {
                *hook_summary = hooks::integrations_summary(&config.home);
            }
        }
        terminal.draw(|frame| {
            targets = draw_screen(frame, &rows, *selected, panel_hover, overlay.as_ref());
        })?;
        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(overlay_state) = overlay.as_mut() {
                        if handle_overlay_key(overlay_state, key, pane) {
                            if key.kind != KeyEventKind::Release
                                && key.code == KeyCode::Esc
                                && !overlay_state.running
                            {
                                overlay = None;
                                overlay_rx = None;
                            }
                            continue;
                        }
                    }
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    match key.code {
                        KeyCode::Esc => return Ok(()),
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            let Some(row) = rows.get(*selected) else {
                                return Ok(());
                            };
                            if let Some(slot) = group_launch_slot_index(row) {
                                let _ = cycle_group_launch_slot(&config.home, slot);
                                continue;
                            }
                            if toggle_new_session_preselect_row(&config.home, row) {
                                continue;
                            }
                            if row_opens_overlay(row) {
                                if let Some((next_overlay, rx)) = open_row_overlay(config, row) {
                                    overlay = Some(next_overlay);
                                    overlay_rx = Some(rx);
                                }
                            } else {
                                return Ok(());
                            }
                        }
                        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(())
                        }
                        KeyCode::Char(',') if key.modifiers.contains(KeyModifiers::SUPER) => {
                            return Ok(())
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            move_selection(&rows, selected, -1);
                            panel_hover.row = None;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            move_selection(&rows, selected, 1);
                            panel_hover.row = None;
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    if overlay.is_some() {
                        continue;
                    }
                    if handle_mouse(
                        config,
                        hook_summary,
                        mouse,
                        &targets,
                        &rows,
                        selected,
                        panel_hover,
                        &mut overlay,
                        &mut overlay_rx,
                    )? {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
    }
}

pub(super) fn handle_mouse(
    config: &Config,
    _hook_summary: &mut String,
    mouse: MouseEvent,
    targets: &PanelTargets,
    rows: &[SettingsRow],
    selected: &mut usize,
    panel_hover: &mut PanelHover,
    overlay: &mut Option<SettingsOverlay>,
    overlay_rx: &mut Option<Receiver<OverlayMsg>>,
) -> Result<bool> {
    let col = mouse.column;
    let row = mouse.row;

    match mouse.kind {
        MouseEventKind::Moved => {
            let next_cta = point_in_rect(col, row, targets.cta);
            if panel_hover.cta != next_cta {
                panel_hover.cta = next_cta;
            }
            let next_close = point_in_rect(col, row, targets.close);
            if panel_hover.close != next_close {
                panel_hover.close = next_close;
            }
            let next_row = row_at_mouse(&targets.row_rects, col, row);
            if panel_hover.row != next_row {
                panel_hover.row = next_row;
            }
            sync_panel_mouse_cursor(panel_hover);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if point_in_rect(col, row, targets.close) {
                return Ok(true);
            }
            if point_in_rect(col, row, targets.cta) {
                return Ok(true);
            }
            if let Some(idx) = row_at_mouse(&targets.row_rects, col, row) {
                *selected = idx;
                panel_hover.row = Some(idx);
                if let Some(row) = rows.get(idx) {
                    if let Some(slot) = group_launch_slot_index(row) {
                        let _ = cycle_group_launch_slot(&config.home, slot);
                    } else if toggle_new_session_preselect_row(&config.home, row) {
                        // toggled
                    } else if row_opens_overlay(row) {
                        if let Some((next_overlay, rx)) = open_row_overlay(config, row) {
                            *overlay = Some(next_overlay);
                            *overlay_rx = Some(rx);
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(false)
}

fn list_row_rects(list_area: Rect, rows: &[SettingsRow]) -> Vec<(usize, Rect)> {
    let mut rects = Vec::new();
    let mut y = list_area.y;
    for line in build_list_layout(rows) {
        if y >= list_area.y.saturating_add(list_area.height) {
            break;
        }
        match line {
            ListLine::Gap => {}
            ListLine::Row(idx) if rows[idx].kind != RowKind::Section => {
                rects.push((
                    idx,
                    Rect {
                        x: list_area.x,
                        y,
                        width: list_area.width,
                        height: 1,
                    },
                ));
            }
            ListLine::Row(_) => {}
        }
        y = y.saturating_add(1);
    }
    rects
}

pub(crate) fn row_at_mouse(row_rects: &[(usize, Rect)], col: u16, row: u16) -> Option<usize> {
    row_rects
        .iter()
        .find(|(_, rect)| point_in_rect(col, row, *rect))
        .map(|(idx, _)| *idx)
}
