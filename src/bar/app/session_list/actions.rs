use super::super::{App, SIDEBAR_UI_SAVE_DEBOUNCE};
use crate::bar::group_order::{self};
use crate::bar::ui::{self};
use anyhow::Result;
use crossterm::event::{self, Event, MouseEvent, MouseEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::{Duration, Instant};

impl App {
    pub(crate) fn persist_sidebar_ui_now(&mut self) {
        if let Err(err) = crate::bar::sidebar_ui::save(
            &self.config,
            &self.expanded_groups,
            self.sidebar_ui_selected_sessions_session_id.as_deref(),
        ) {
            tracing::warn!("failed to persist sidebar ui: {err}");
        }
        self.sidebar_ui_save_deadline = None;
    }
    pub(crate) fn schedule_sidebar_ui_save(&mut self) {
        self.sidebar_ui_save_deadline = Some(Instant::now() + SIDEBAR_UI_SAVE_DEBOUNCE);
    }
    pub(crate) fn flush_sidebar_ui_save_if_due(&mut self) {
        let Some(deadline) = self.sidebar_ui_save_deadline else {
            return;
        };
        if Instant::now() >= deadline {
            self.persist_sidebar_ui_now();
        }
    }
    pub(crate) fn flush_sidebar_ui_save_pending(&mut self) {
        if self.sidebar_ui_save_deadline.is_some() {
            self.persist_sidebar_ui_now();
        }
    }
    pub(crate) fn sidebar_ui_save_poll_cap(&self) -> Option<Duration> {
        self.sidebar_ui_save_deadline
            .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
    }
    pub(crate) fn toggle_sessions_expanded(&mut self) {
        self.sessions_expanded = !self.sessions_expanded;
        self.scroll = 0;
        self.persist_notepad_now();
        self.force_redraw();
    }
    pub(crate) fn toggle_group_expanded(&mut self, cwd_label: String) {
        if self.expanded_groups.contains(&cwd_label) {
            self.expanded_groups.remove(&cwd_label);
        } else {
            self.expanded_groups.insert(cwd_label);
        }
        self.schedule_sidebar_ui_save();
        self.rebuild_rows();
        self.force_redraw();
    }
    pub(crate) fn toggle_group_folded(&mut self, cwd_label: String) {
        if self.folded_groups.contains(&cwd_label) {
            self.folded_groups.remove(&cwd_label);
        } else {
            self.folded_groups.insert(cwd_label);
        }
        self.persist_folded_groups();
        self.rebuild_rows();
        self.force_redraw();
    }
    pub(crate) fn persist_folded_groups(&self) {
        if let Err(err) = group_order::save_folded(&self.config, &self.folded_groups) {
            tracing::warn!("failed to persist folded groups: {err}");
        }
    }
    pub(crate) fn coalesce_mouse_moves(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        let size = terminal.size()?;
        let metrics = self.layout_metrics(size);
        let mut latest_move: Option<MouseEvent> = None;
        let mut latest_drag: Option<MouseEvent> = None;
        while event::poll(Duration::from_millis(0))? {
            match event::read()? {
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Moved => {
                    latest_move = Some(mouse);
                }
                Event::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Drag(_)) => {
                    // Keep only the latest drag sample so edge-resize / select
                    // don't apply N tmux resizes + full paints per input burst.
                    latest_drag = Some(mouse);
                }
                // Resize storms during edge-drag are echoes of our own resize-pane.
                Event::Resize(w, h) if self.edge_resize_active => {
                    self.handle_terminal_resize(w, h);
                }
                other => {
                    // Flush pending drag first so order stays sensible.
                    if let Some(mouse) = latest_drag.take() {
                        self.note_pointer_activity(&mouse, size.width);
                        self.handle_mouse(&mouse, &metrics);
                    }
                    self.handle_event(&other, terminal)?;
                    if self.refresh_close_hold_state() {
                        let size = terminal.size()?;
                        let metrics = self.layout_metrics(size);
                        self.seed_close_hover(&metrics);
                    }
                    self.redraw_if_needed(terminal)?;
                    return Ok(());
                }
            }
        }
        if let Some(mouse) = latest_drag {
            self.note_pointer_activity(&mouse, size.width);
            let size = terminal.size()?;
            let metrics = self.layout_metrics(size);
            let was_edge = self.edge_resize_active;
            self.handle_mouse(&mouse, &metrics);
            // Skip full paint storms during live edge-drag (same as handle_event).
            if !(was_edge && self.edge_resize_active) {
                self.redraw_if_needed(terminal)?;
            }
            return Ok(());
        }
        if let Some(mouse) = latest_move {
            self.note_pointer_activity(&mouse, size.width);
            // If edge-resize or list drag was left active (lost MouseUp), Moved must
            // end it — the light hover path used to keep updating hover / re-engage
            // hold-drag so PWD groups stuck to the pointer in Cursor/VS Code.
            if self.edge_resize_active
                || self.group_drag.active()
                || self.group_drag.pending()
                || self.note_drag.active()
                || self.note_drag.pending()
            {
                let size = terminal.size()?;
                let metrics = self.layout_metrics(size);
                self.handle_mouse(&mouse, &metrics);
                self.redraw_if_needed(terminal)?;
                return Ok(());
            }
            self.last_mouse = Some(mouse);
            self.maybe_clear_list_hover_suppress(mouse.row);
            if self.close_modifier_held {
                self.touch_close_hold();
                self.update_close_target_hover(&mouse, &metrics);
            } else {
                self.update_toolbar_hover(&mouse, &metrics);
                self.update_group_hover(&mouse, &metrics);
                self.update_session_hover(&mouse, &metrics);
            }
            self.redraw_if_needed(terminal)?;
        }
        Ok(())
    }
    pub(crate) fn engage_group_drag(&mut self) {
        let Some(label) = self.group_drag.pending_click_label.take() else {
            return;
        };
        self.group_hover_row = None;
        self.hover_row = None;
        // Materialize live PWDs into the order vec so preview/reorder can find them.
        self.sync_group_order_labels();
        self.group_drag.source = Some(label.clone());
        self.group_drag.hover = Some(label);
        self.rebuild_rows();
        self.force_redraw();
    }
    pub(crate) fn finish_group_drag_pending_click(&mut self) {
        let pending_label = self.group_drag.pending_click_label.take();
        self.group_drag = ui::GroupDragState::default();
        if let Some(label) = pending_label {
            self.unfocus_notepad();
            self.toggle_group_folded(label);
        }
        self.force_redraw();
    }
    /// Commit an engaged PWD group drag (reorder on drop, or fold if no motion).
    ///
    /// Also used when Cursor/VS Code xterm.js drops MouseUp: SGR `Moved` means the
    /// button is up, so we must release the sticky source highlight instead of
    /// continuing to track hover forever.
    pub(crate) fn finish_active_group_drag(&mut self, mouse: &MouseEvent) {
        if !self.group_drag.active() {
            return;
        }
        self.restore_workspace_if_panel_open();
        let Some(from) = self.group_drag.source.take() else {
            return;
        };
        let hover = self.group_drag.hover.take();
        let dragged = self.group_drag.dragged;
        match hover.as_deref() {
            Some(to) if to != from.as_str() => {
                // Labels may only exist after ensure_labels (never written
                // into a partial saved order). Sync first so reorder works.
                self.sync_group_order_labels();
                group_order::reorder(&mut self.group_order, &from, to);
                let _ = group_order::save(
                    &self.config,
                    &crate::bar::group_order::SidebarGroupOrder {
                        groups: self.group_order.clone(),
                    },
                );
                crate::telemetry::record_feature(
                    crate::telemetry::FeatureId::GroupDragReorder,
                    crate::telemetry::feature::Source::Mouse,
                );
                self.rebuild_rows();
            }
            Some(to) if to == from.as_str() && !dragged => {
                self.unfocus_notepad();
                self.toggle_group_folded(from);
            }
            _ => {
                self.rebuild_rows();
            }
        }
        if dragged {
            self.restore_preserved_selection();
        }
        self.group_drag.preserved_session_id = None;
        self.group_drag.preserved_group_toggle = None;
        if dragged {
            self.begin_list_hover_suppress_after_group_drag(mouse.row);
        } else {
            self.hover_row = None;
            self.group_hover_row = None;
        }
        self.force_redraw();
    }
    /// End stuck sidebar list drags when SGR reports motion without a button
    /// (`Moved`). xterm.js in Cursor/VS Code often drops the matching MouseUp.
    pub(crate) fn finish_sidebar_drag_if_button_released(&mut self, mouse: &MouseEvent) -> bool {
        if self.group_drag.active() {
            self.finish_active_group_drag(mouse);
            return true;
        }
        if self.note_drag.active() {
            self.finish_note_drag(mouse);
            return true;
        }
        if self.group_drag.pending() {
            // Up dropped before engage — treat as click (fold), never lift-on-hover.
            self.finish_group_drag_pending_click();
            return true;
        }
        if self.note_drag.pending() {
            self.finish_note_drag_pending_click(mouse);
            return true;
        }
        false
    }
    pub(crate) fn update_group_drag_hover(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        let source = self.group_drag.source.as_deref();
        // Hit-test against the stable (non-preview) layout with live labels, not the
        // live preview layout. Preview reorder moves sections under the cursor and
        // flips the target back to the source — the grab/flash/jump loop users see.
        let stable_order = self.group_order_with_live_labels();
        let hit_rows = ui::build_rows(
            &self.sessions,
            &self.expanded_groups,
            &self.folded_groups,
            &stable_order,
        );
        let new_hover = ui::row_from_mouse(
            mouse.row,
            metrics.list_top_y,
            metrics.list_height,
            self.scroll,
            hit_rows.len(),
        )
        .and_then(|row_idx| source.and_then(|from| ui::group_drag_target(&hit_rows, row_idx, from)))
        .or_else(|| self.group_drag.hover.clone());

        if new_hover.as_deref() != source {
            self.group_drag.dragged = true;
        }
        if self.group_drag.hover != new_hover {
            self.group_drag.hover = new_hover;
            self.rebuild_rows();
        }
    }
    pub(crate) fn cwd_for_new_session(&self) -> Option<(String, String)> {
        if let Some(session) = self.sessions.iter().find(|session| session.is_active) {
            return Some((session.cwd.clone(), session.cwd_label.clone()));
        }
        if let Some(session) = self.session_at(self.selected) {
            return Some((session.cwd.clone(), session.cwd_label.clone()));
        }
        None
    }
    pub(crate) fn create_session(&mut self) {
        self.restore_workspace_if_panel_open();
        let home = self.config.home.display().to_string();
        if let Ok(window_index) = crate::daemon::tmux::create_console_window(&self.config) {
            let cwd_label = crate::pty::format_tilde_path(&home, &self.config.home);
            self.push_optimistic_new_session(window_index, None, home, cwd_label, "console", true);
            self.client.refresh_async();
        }
        self.rows_version = self.rows_version.wrapping_add(1);
    }
    pub(crate) fn create_console_in_group(&mut self, cwd_label: &str) {
        self.restore_workspace_if_panel_open();
        let Some(cwd) = self
            .sessions
            .iter()
            .find(|session| session.cwd_label == cwd_label)
            .map(|session| session.cwd.clone())
        else {
            return;
        };
        match crate::daemon::tmux::create_window_in_cwd(&self.config, &cwd) {
            Ok(window_index) => {
                self.pending_focus_tab_index = Some(window_index);
                self.apply_refresh_snapshot();
                self.select_session_by_tab_index(window_index);
            }
            Err(_) => {
                self.pending_focus_tab_index = None;
            }
        }
        self.rows_version = self.rows_version.wrapping_add(1);
        self.force_redraw();
    }
    /// Open a coding agent (e.g. grok / opencode) in the given pwd group — same as
    /// the group-header `[G]` / `[O]` badges.
    pub(crate) fn create_agent_in_group(&mut self, cwd_label: &str, agent_id: &str) {
        self.restore_workspace_if_panel_open();
        let Some(cwd) = self
            .sessions
            .iter()
            .find(|session| session.cwd_label == cwd_label)
            .map(|session| session.cwd.clone())
        else {
            return;
        };
        match crate::daemon::tmux::create_agent_window_in_cwd(&self.config, &cwd, agent_id) {
            Ok(window_index) => {
                let title = crate::pty::format_session_title(agent_id, "?");
                self.push_optimistic_new_session(
                    window_index,
                    Some(agent_id),
                    cwd,
                    cwd_label.to_string(),
                    &title,
                    true,
                );
                self.client.refresh_async();
            }
            Err(_) => {
                self.pending_focus_tab_index = None;
            }
        }
        self.rows_version = self.rows_version.wrapping_add(1);
        self.force_redraw();
    }
    pub(crate) fn create_agent_session(&mut self, agent_id: &str) {
        self.restore_workspace_if_panel_open();
        let result = if let Some((cwd, cwd_label)) = self.cwd_for_new_session() {
            crate::daemon::tmux::create_agent_window_in_cwd(&self.config, &cwd, agent_id)
                .ok()
                .map(|index| (index, cwd, cwd_label))
        } else {
            crate::daemon::tmux::create_agent_window(&self.config, agent_id)
                .ok()
                .map(|index| (index, String::new(), String::new()))
        };
        if let Some((window_index, cwd, cwd_label)) = result {
            let cwd_label = if cwd_label.is_empty() && !cwd.is_empty() {
                crate::pty::format_tilde_path(&cwd, &self.config.home)
            } else {
                cwd_label
            };
            let title = crate::pty::format_session_title(agent_id, "?");
            self.push_optimistic_new_session(
                window_index,
                Some(agent_id),
                cwd,
                cwd_label,
                &title,
                true,
            );
            self.client.refresh_async();
        }
        self.rows_version = self.rows_version.wrapping_add(1);
    }
    pub(crate) fn update_group_hover(&mut self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) {
        if self.list_hover_updates_suppressed(mouse.row) {
            return;
        }
        let next_group =
            ui::group_row_from_mouse(mouse.row, metrics, self.scroll, self.rows.len(), &self.rows);
        if self.group_hover_row != next_group {
            self.group_hover_row = next_group;
        }
    }
    pub(crate) fn update_session_hover(&mut self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) {
        if self.close_modifier_held || self.list_hover_updates_suppressed(mouse.row) {
            return;
        }
        let next_hover = self.session_row_under_mouse(mouse, metrics);
        if self.hover_row != next_hover {
            self.hover_row = next_hover;
        }
    }
    pub(crate) fn confirm_close_session(&mut self) {
        let _ = crate::daemon::tmux::confirm_close_active_window(&self.config.tmux_session);
        self.rows_version = self.rows_version.wrapping_add(1);
    }
    pub(crate) fn detach_client(&mut self) {
        let _ = crate::daemon::tmux::detach_current_client();
        self.rows_version = self.rows_version.wrapping_add(1);
    }
}
