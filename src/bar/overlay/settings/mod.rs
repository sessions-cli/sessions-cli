//! Settings overlay (Ratatui panel).

mod input;
mod render;
mod state;

use crate::bar::art_canvas;
use crate::bar::panel_popup::PANEL_SECTION_PAD;
use input::{event_loop, handle_mouse, setup_terminal, teardown_terminal};
use state::{
    drain_overlay, first_selectable, handle_overlay_key, move_selection, open_row_overlay,
    panel_content_height, row_opens_overlay, OverlayMsg, SettingsOverlay,
};

use crate::config::Config;
use crate::hooks;
use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyEventKind, KeyModifiers, MouseEvent};
use ratatui::layout::Rect;
use std::sync::mpsc::Receiver;

pub use render::draw_screen;
pub use state::{build_rows, PanelHover, PanelTargets, SettingsRow};

pub fn run_settings(config: &Config) -> Result<()> {
    let mut hook_summary = hooks::integrations_summary(&config.home);
    let rows = build_rows(config, &hook_summary);
    let mut selected = first_selectable(&rows);
    let mut panel_hover = PanelHover::default();
    let _ = crate::daemon::tmux::write_host_terminal_backdrop();
    let mut terminal = setup_terminal()?;
    let result = event_loop(
        config,
        &mut terminal,
        &mut hook_summary,
        &mut selected,
        &mut panel_hover,
    );
    teardown_terminal()?;
    result
}

/// Card-sized popup for `sessions settings` over the workspace pane.
pub fn popup_size(config: &Config, workspace_w: u16, workspace_h: u16) -> (u16, u16) {
    let hook_summary = hooks::integrations_summary(&config.home);
    let rows = build_rows(config, &hook_summary);
    let width = art_canvas::pane_fraction_width(workspace_w).saturating_add(PANEL_SECTION_PAD);
    // Leave a few rows of margin so tall panels can sit vertically centered in the pane.
    let max_height = workspace_h.saturating_sub(4).max(1);
    let height = panel_content_height(&rows)
        .saturating_add(PANEL_SECTION_PAD)
        .min(max_height);
    (width.max(1), height.max(1))
}
pub fn point_in_rect(x: u16, y: u16, rect: Rect) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    x >= rect.x
        && y >= rect.y
        && x < rect.x.saturating_add(rect.width)
        && y < rect.y.saturating_add(rect.height)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsAction {
    Unchanged,
    Close,
}

pub struct SettingsPanel {
    pub hook_summary: String,
    pub selected: usize,
    pub hover: PanelHover,
    targets: PanelTargets,
    overlay: Option<SettingsOverlay>,
    overlay_rx: Option<Receiver<OverlayMsg>>,
}

impl SettingsPanel {
    pub fn new(config: &Config) -> Self {
        let hook_summary = hooks::integrations_summary(&config.home);
        let rows = build_rows(config, &hook_summary);
        Self {
            hook_summary,
            selected: first_selectable(&rows),
            hover: PanelHover::default(),
            targets: PanelTargets::default(),
            overlay: None,
            overlay_rx: None,
        }
    }

    pub fn rows(&self, config: &Config) -> Vec<SettingsRow> {
        build_rows(config, &self.hook_summary)
    }

    pub fn draw(&mut self, frame: &mut ratatui::Frame<'_>, config: &Config) {
        let rows = self.rows(config);
        let pane = frame.area();
        if let (Some(overlay), Some(rx)) = (self.overlay.as_mut(), self.overlay_rx.as_ref()) {
            let was_running = overlay.running;
            drain_overlay(rx, overlay, pane);
            if was_running && overlay.finished && overlay.title == "Agent hooks" {
                self.hook_summary = hooks::integrations_summary(&config.home);
            }
        }
        self.targets = draw_screen(
            frame,
            &rows,
            self.selected,
            &self.hover,
            self.overlay.as_ref(),
        );
    }

    pub fn handle_key(&mut self, config: &Config, key: event::KeyEvent) -> Result<SettingsAction> {
        let pane = Rect::new(0, 0, 80, 24);
        if let Some(overlay) = self.overlay.as_mut() {
            if handle_overlay_key(overlay, key, pane) {
                if key.kind != KeyEventKind::Release && key.code == KeyCode::Esc && !overlay.running
                {
                    self.overlay = None;
                    self.overlay_rx = None;
                }
                return Ok(SettingsAction::Unchanged);
            }
        }
        if key.kind == KeyEventKind::Release {
            return Ok(SettingsAction::Unchanged);
        }
        let rows = self.rows(config);
        match key.code {
            KeyCode::Esc => return Ok(SettingsAction::Close),
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(SettingsAction::Close);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let Some(row) = rows.get(self.selected) else {
                    return Ok(SettingsAction::Close);
                };
                if row_opens_overlay(row) {
                    if let Some((next_overlay, rx)) = open_row_overlay(config, row) {
                        self.overlay = Some(next_overlay);
                        self.overlay_rx = Some(rx);
                    }
                } else {
                    return Ok(SettingsAction::Close);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                move_selection(&rows, &mut self.selected, -1);
                self.hover.row = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                move_selection(&rows, &mut self.selected, 1);
                self.hover.row = None;
            }
            _ => {}
        }
        Ok(SettingsAction::Unchanged)
    }

    pub fn handle_mouse(&mut self, config: &Config, mouse: MouseEvent) -> Result<SettingsAction> {
        if self.overlay.is_some() {
            return Ok(SettingsAction::Unchanged);
        }
        let rows = self.rows(config);
        if handle_mouse(
            config,
            &mut self.hook_summary,
            mouse,
            &self.targets,
            &rows,
            &mut self.selected,
            &mut self.hover,
            &mut self.overlay,
            &mut self.overlay_rx,
        )? {
            return Ok(SettingsAction::Close);
        }
        Ok(SettingsAction::Unchanged)
    }
}

#[cfg(test)]
mod tests {
    use super::input::row_at_mouse;
    use super::render::row_backdrop;
    use super::*;
    use crate::bar::ui::{BG_HIGHLIGHT, BG_HOVER_SELECTED, BG_PANEL, BG_SELECTED};
    use crate::telemetry::SessionsConfig;
    use crate::version::VERSION;
    use state::{
        build_list_layout, list_line_count, list_row_rects, settings_layout, update_rows, ListLine,
        OverlayMsg, PanelTargets, RowKind, CTA_TOP_GAP, HINT_TOP_GAP, OVERLAY_HINT_ROWS,
    };
    use std::sync::mpsc;

    #[test]
    fn build_rows_includes_sections_and_shortcuts() {
        let config = Config::default();
        let hook_summary = hooks::integrations_summary(&config.home);
        let rows = build_rows(&config, &hook_summary);
        assert!(rows
            .iter()
            .any(|row| row.label == "Installed version" && row.detail == VERSION));
        assert!(rows.iter().any(|row| row.label == "General"));
        assert!(rows.iter().any(|row| row.label == "Agent hooks"));
        assert!(rows.iter().any(|row| row.label == "New session"));
        assert!(rows
            .iter()
            .any(|row| row.label == "Preselect agent & model"));
        assert!(!rows.iter().any(|row| row.label == "Updates"));
        assert!(!rows.iter().any(|row| row.label == "Integrations"));
        assert!(!rows.iter().any(|row| row.label == "Sidebar width"));
        assert!(!rows.iter().any(|row| row.label == "Daemon poll interval"));
        assert!(rows.iter().any(|row| row.label == "Shortcuts"));
        assert!(rows
            .iter()
            .any(|row| row.label == "⌘," && row.detail == "toggle settings"));
        assert!(rows.iter().any(|row| {
            row.label == "⌘1–9, ⌘0" && row.detail == "focus by number (anywhere)"
        }));
        assert!(
            rows.iter()
                .filter(|row| row.kind == RowKind::Shortcut)
                .count()
                >= 10
        );
    }

    #[test]
    fn move_selection_skips_section_headers() {
        let config = Config::default();
        let hook_summary = hooks::integrations_summary(&config.home);
        let rows = build_rows(&config, &hook_summary);
        let mut selected = first_selectable(&rows);
        let first = selected;
        move_selection(&rows, &mut selected, -1);
        assert_eq!(selected, first);
        move_selection(&rows, &mut selected, 1);
        assert_ne!(rows[selected].kind, RowKind::Section);
    }

    #[test]
    fn point_in_rect_detects_cta_target() {
        let rect = Rect::new(10, 4, 14, 3);
        assert!(point_in_rect(10, 4, rect));
        assert!(point_in_rect(23, 6, rect));
        assert!(!point_in_rect(9, 4, rect));
        assert!(!point_in_rect(24, 4, rect));
    }

    #[test]
    fn panel_column_matches_new_chat_form_width() {
        let pane = Rect::new(0, 0, 80, 24);
        let column = art_canvas::panel_column_rect(pane);
        let art = art_canvas::art_canvas_rect(pane);
        assert_eq!(column.width, art.width);
        assert_eq!(column.x, art.x);
    }

    #[test]
    fn list_row_rects_skip_sections_and_gaps() {
        let config = Config::default();
        let hook_summary = hooks::integrations_summary(&config.home);
        let rows = build_rows(&config, &hook_summary);
        // Tall enough for Readiness + General + Notifications sections.
        let list_area = Rect::new(5, 10, 40, 80);
        let rects = list_row_rects(list_area, &rows);
        assert!(!rects.is_empty());
        assert!(rects
            .iter()
            .all(|(idx, _)| rows[*idx].kind != RowKind::Section));
        assert!(
            rows.iter().any(|row| row.label == "Readiness"),
            "settings should include install readiness section"
        );
        let notes_idx = rows
            .iter()
            .position(|row| row.label == "Notes directory")
            .unwrap();
        let completion_idx = rows
            .iter()
            .position(|row| row.label == "Completion bell")
            .unwrap();
        let notes_y = rects
            .iter()
            .find(|(idx, _)| *idx == notes_idx)
            .map(|(_, rect)| rect.y)
            .expect("notes directory should be in visible list rects");
        let completion_y = rects
            .iter()
            .find(|(idx, _)| *idx == completion_idx)
            .map(|(_, rect)| rect.y)
            .expect("completion bell should be in visible list rects");
        assert!(
            notes_y >= list_area.y,
            "notes directory y should be inside list area"
        );
        assert!(
            completion_y > notes_y,
            "completion bell row should follow general section rows"
        );
    }

    #[test]
    fn row_at_mouse_maps_selectable_rows() {
        let config = Config::default();
        let hook_summary = hooks::integrations_summary(&config.home);
        let rows = build_rows(&config, &hook_summary);
        let list_area = Rect::new(0, 0, 50, 40);
        let rects = list_row_rects(list_area, &rows);
        let (idx, rect) = rects[2];
        assert_eq!(row_at_mouse(&rects, rect.x, rect.y), Some(idx));
        assert_eq!(
            row_at_mouse(&rects, rect.x, list_area.y + list_area.height),
            None
        );
    }

    #[test]
    fn row_backdrop_matches_sidebar_hover_semantics() {
        assert_eq!(row_backdrop(false, false), BG_PANEL);
        assert_eq!(row_backdrop(false, true), BG_HIGHLIGHT);
        assert_eq!(row_backdrop(true, false), BG_SELECTED);
        assert_eq!(row_backdrop(true, true), BG_HOVER_SELECTED);
    }

    #[test]
    fn cta_follows_list_with_hint_below() {
        let config = Config::default();
        let hook_summary = hooks::integrations_summary(&config.home);
        let rows = build_rows(&config, &hook_summary);
        let inner = Rect::new(0, 0, 80, 60);
        let layout = settings_layout(inner, &rows);
        assert_eq!(
            layout.cta.y,
            layout.list.y + layout.list.height + CTA_TOP_GAP
        );
        assert_eq!(
            layout.hint.y,
            layout.cta.y + layout.cta.height + HINT_TOP_GAP
        );
        assert!(layout.hint.y + layout.hint.height <= inner.y + inner.height);
    }

    #[test]
    fn update_rows_show_available_after_dismiss() {
        let home = tempfile::tempdir().unwrap();
        let mut cfg = SessionsConfig::default();
        cfg.update.available_version = "0.2.0".into();
        cfg.update.urgency = "recommended".into();
        cfg.update.message = "New features".into();
        cfg.update.dismissed_version = "0.2.0".into();
        cfg.save(home.path()).unwrap();

        let rows = update_rows(home.path());
        assert!(rows.iter().any(|row| row.label == "Available"));
        assert!(rows.iter().any(|row| row.label == "Install update"));
        assert!(!rows.iter().any(|row| row.label == "Status"));
    }

    #[test]
    fn row_opens_overlay_for_update_and_release_notes() {
        assert!(row_opens_overlay(&SettingsRow {
            kind: RowKind::Action,
            label: "Install update".into(),
            detail: "↵".into(),
        }));
        assert!(row_opens_overlay(&SettingsRow {
            kind: RowKind::Config,
            label: "Release notes".into(),
            detail: "Short…".into(),
        }));
        assert!(!row_opens_overlay(&SettingsRow {
            kind: RowKind::Config,
            label: "Completion bell".into(),
            detail: "On".into(),
        }));
    }

    #[test]
    fn overlay_scrolls_to_latest_output() {
        let mut overlay = SettingsOverlay::running("Upgrading sessions");
        let pane = Rect::new(0, 0, 80, 24);
        for idx in 0..20 {
            overlay.apply(OverlayMsg::Line(format!("line {idx}")));
        }
        drain_overlay(&mpsc::channel().1, &mut overlay, pane);
        assert!(overlay.scroll > 0);
    }

    #[test]
    fn panel_content_centers_when_pane_is_taller() {
        let config = Config::default();
        let hook_summary = hooks::integrations_summary(&config.home);
        let rows = build_rows(&config, &hook_summary);
        let inner = Rect::new(0, 0, 50, 80);
        let layout = settings_layout(inner, &rows);
        let desired = panel_content_height(&rows);
        let expected_top = inner.y + (inner.height - desired) / 2;
        assert_eq!(layout.header.y, expected_top);
    }

    #[test]
    fn settings_layout_fits_short_pane_without_overflow() {
        // Regression: PWD quick-launch rows made desired height exceed typical panes
        // and draw_screen panicked painting the hint past the buffer bottom.
        let config = Config::default();
        let hook_summary = hooks::integrations_summary(&config.home);
        let rows = build_rows(&config, &hook_summary);
        let inner = Rect::new(0, 0, 80, 24);
        let layout = settings_layout(inner, &rows);
        let bottom = inner.y + inner.height;
        assert!(layout.list.height > 0);
        assert!(layout.list.y + layout.list.height <= bottom);
        assert!(layout.cta.y + layout.cta.height <= bottom);
        assert!(layout.hint.y + layout.hint.height <= bottom);
        assert!(layout.list.height < list_line_count(&rows));
    }
}
