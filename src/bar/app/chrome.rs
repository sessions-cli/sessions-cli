use super::{
    load_update_banner, App, CLIPBOARD_NOTICE_DURATION, POINTER_EXIT_HOVER_CLEAR,
    SIDEBAR_ENGAGE_THROTTLE, SIDEBAR_FOCUS_PROBE_INTERVAL, SIDEBAR_POINTER_CURSOR_HOLD,
    TELEMETRY_FLUSH_INTERVAL, WORKSPACE_PANEL_OPEN_GRACE, WORKSPACE_PANEL_PROBE_INTERVAL,
};
use crate::bar::client::ClientEvent;
use crate::bar::mouse_cursor::{self, MouseCursorShape};
use crate::bar::ui::{self, GroupDragState, NotepadHit, RowKind, ToolbarAction};
use anyhow::Result;
use crossterm::event::{
    EnableMouseCapture, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::Instant;

impl App {
    pub(crate) fn reload_update_banner(&mut self) {
        self.update_banner = load_update_banner();
    }
    pub(crate) fn maybe_flush_telemetry(&mut self) {
        if self.last_telemetry_flush.elapsed() < TELEMETRY_FLUSH_INTERVAL {
            return;
        }
        self.last_telemetry_flush = Instant::now();
        let wrote =
            crate::telemetry::counters::save_pending_to_file(&self.config.home).unwrap_or(false);
        if wrote {
            // Merge counters into daemon accumulator — no Supabase HTTP from the bar.
            self.client.telemetry_flush_async();
        }
    }
    pub(crate) fn show_status_notice(&mut self, message: impl Into<String>) {
        self.clipboard_notice_text = Some(message.into());
        self.clipboard_notice_until = Some(Instant::now() + CLIPBOARD_NOTICE_DURATION);
        self.force_redraw();
    }
    pub(crate) fn show_clipboard_notice(&mut self) {
        self.show_status_notice("copied");
    }
    pub(crate) fn expire_clipboard_notice_if_due(&mut self) {
        let Some(until) = self.clipboard_notice_until else {
            return;
        };
        if Instant::now() >= until {
            self.clipboard_notice_until = None;
            self.clipboard_notice_text = None;
            self.force_redraw();
        }
    }
    pub(crate) fn copy_text_to_clipboard(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        match crate::clipboard::copy(text) {
            Ok(()) => {
                self.show_clipboard_notice();
                true
            }
            Err(error) => {
                tracing::warn!("clipboard copy failed: {error:#}");
                false
            }
        }
    }
    pub(crate) fn apply_paste_to_rename(&mut self, raw: &str) {
        let text = crate::clipboard::sanitize_paste_text(raw, false);
        if text.is_empty() {
            return;
        }
        if let Some(rename) = self.rename.as_mut() {
            ui::rename_apply_paste(rename, &text);
            self.force_redraw();
        }
    }
    pub(crate) fn resolve_paste_text(&self, primary: Option<&str>) -> Option<String> {
        if let Some(text) = primary.filter(|text| !text.is_empty()) {
            return Some(text.to_string());
        }
        crate::clipboard::paste()
            .ok()
            .filter(|text| !text.is_empty())
    }
    pub(crate) fn apply_resolved_paste(&mut self, text: &str) -> bool {
        if self.rename.is_some() {
            self.apply_paste_to_rename(text);
            return true;
        }
        if !self.notepad_expanded {
            self.toggle_notepad_expanded(true);
        }
        self.focus_notepad_at_cursor(Some(self.notepad_editor.cursor));
        self.apply_paste_to_notepad(text);
        true
    }
    pub(crate) fn handle_paste_event(
        &mut self,
        text: &str,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        if let Some(resolved) = self.resolve_paste_text(Some(text)) {
            self.apply_resolved_paste(&resolved);
        } else {
            self.show_status_notice("paste failed");
            tracing::warn!("paste event had no OS or tmux buffer content");
        }
        self.redraw_if_needed(terminal)
    }
    pub(crate) fn handle_paste_key(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        if let Some(resolved) = self.resolve_paste_text(None) {
            self.apply_resolved_paste(&resolved);
        } else {
            self.show_status_notice("paste failed");
            tracing::warn!("paste key had no OS or tmux buffer content");
        }
        self.redraw_if_needed(terminal)
    }
    pub(crate) fn clear_list_text_selection(&mut self) {
        self.list_select_anchor = None;
        self.list_select_head = None;
        self.list_text_selecting = false;
    }
    pub(crate) fn begin_list_text_selection(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        // Cursor/VS Code: skip drag-select — tiny move events make selection
        // stick to the pointer and steal click-to-activate.
        if !self.host.allows_list_text_drag_select() {
            self.clear_list_text_selection();
            return;
        }
        let total_rows = ui::total_list_rows(
            self.rows.len(),
            self.sessions_expanded,
            &self.notepad_list_state(),
        );
        self.list_select_anchor = ui::list_text_point_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            total_rows,
            &self.rows,
            metrics.list_line_width,
        );
        self.list_select_head = self.list_select_anchor;
        self.list_text_selecting = false;
    }
    pub(crate) fn update_list_text_selection(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        if !self.host.allows_list_text_drag_select() {
            self.clear_list_text_selection();
            return;
        }
        let Some(anchor) = self.list_select_anchor else {
            return;
        };
        let total_rows = ui::total_list_rows(
            self.rows.len(),
            self.sessions_expanded,
            &self.notepad_list_state(),
        );
        let Some(head) = ui::list_text_point_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            total_rows,
            &self.rows,
            metrics.list_line_width,
        ) else {
            return;
        };
        self.list_select_head = Some(head);
        // Require real movement so click jitter does not start a selection.
        let min = crate::bar::host_terminal::LIST_TEXT_SELECT_MIN_DISTANCE;
        let row_delta = head.row_idx.abs_diff(anchor.row_idx);
        let col_delta = head.char_idx.abs_diff(anchor.char_idx);
        if row_delta > 0 || col_delta >= min {
            self.list_text_selecting = true;
        }
    }
    pub(crate) fn finish_list_text_selection(&mut self, metrics: &ui::LayoutMetrics) -> bool {
        let was_selecting = self.list_text_selecting;
        if was_selecting {
            if let (Some(anchor), Some(head)) = (self.list_select_anchor, self.list_select_head) {
                let text =
                    ui::list_selected_plain_text(&self.rows, metrics.list_line_width, anchor, head);
                self.copy_text_to_clipboard(&text);
            }
        }
        self.clear_list_text_selection();
        was_selecting
    }
    /// PWD/note drag engages only when the pointer leaves the source section —
    /// never on hold alone. Hold-to-lift flashed the ⠿ grip on fold clicks and,
    /// combined with xterm.js Drag noise, marked `dragged` so release neither
    /// folded nor reordered.
    pub(crate) fn maybe_engage_sidebar_drag(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        if self.note_drag.pending() && self.note_pending_should_engage(mouse, metrics) {
            self.engage_note_drag();
            self.update_note_drag_hover(mouse, metrics);
        } else if self.group_drag.pending() && self.group_pending_should_engage(mouse, metrics) {
            self.engage_group_drag();
            self.update_group_drag_hover(mouse, metrics);
        }
    }

    /// True when a pending PWD press has moved onto a different directory group.
    fn group_pending_should_engage(&self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) -> bool {
        let Some(label) = self.group_drag.pending_click_label.as_deref() else {
            return false;
        };
        let stable_order = self.group_order_with_live_labels();
        let hit_rows = ui::build_rows(
            &self.sessions,
            &self.expanded_groups,
            &self.folded_groups,
            &stable_order,
        );
        let Some(row_idx) = ui::row_from_mouse(
            mouse.row,
            metrics.list_top_y,
            metrics.list_height,
            self.scroll,
            hit_rows.len(),
        ) else {
            return false;
        };
        ui::group_drag_target(&hit_rows, row_idx, label)
            .as_deref()
            .is_some_and(|target| target != label)
    }

    /// True when a pending note-title press has moved onto a different note.
    fn note_pending_should_engage(&self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) -> bool {
        let Some(note_id) = self.note_drag.pending_click_note_id.as_deref() else {
            return false;
        };
        let trail_base = self.sidebar_trail_base();
        let stable_state = self.stable_notepad_list_state();
        let rel = mouse.row.saturating_sub(metrics.list_top_y) as usize;
        if rel >= metrics.list_height {
            return false;
        }
        let row_idx = self.scroll.saturating_add(rel);
        if row_idx < trail_base {
            return false;
        }
        let trail_idx = row_idx.saturating_sub(trail_base);
        ui::note_drag_target(&stable_state, trail_idx, note_id)
            .as_deref()
            .is_some_and(|target| target != note_id)
    }
    /// True when the pointer is over a session, group header, or group-toggle row.
    pub(crate) fn mouse_over_list_content_row(
        &self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) -> bool {
        ui::row_from_mouse(
            mouse.row,
            metrics.list_top_y,
            metrics.list_height,
            self.scroll,
            self.rows.len(),
        )
        .is_some_and(|idx| {
            matches!(
                self.rows.get(idx),
                Some(RowKind::Session { .. } | RowKind::Group { .. } | RowKind::GroupToggle { .. })
            )
        })
    }

    pub(crate) fn handle_mouse(&mut self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) {
        self.last_mouse = Some(*mouse);
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right)
        ) {
            self.suppress_list_hover_after_group_drag = false;
            self.suppress_list_hover_y = None;
        }
        // Collapsed rail: any click expands; ignore chrome hits while rail-only.
        if self.is_sidebar_rail_collapsed() {
            if matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right)
            ) {
                self.expand_sidebar_from_rail();
            }
            return;
        }
        if self.edge_resize_active {
            match mouse.kind {
                // SGR: button-held motion is Drag only. Moved means button is up
                // (or Up was dropped by xterm.js) — never resize on bare hover.
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.update_edge_resize(mouse);
                    return;
                }
                MouseEventKind::Moved => {
                    // Lost MouseUp is common in Cursor/VS Code; end the drag so
                    // subsequent hover cannot keep sliding the pane.
                    self.finish_edge_resize();
                    return;
                }
                MouseEventKind::Up(MouseButton::Left) | MouseEventKind::Up(MouseButton::Right) => {
                    self.finish_edge_resize();
                    return;
                }
                MouseEventKind::Down(_) => {
                    self.finish_edge_resize();
                }
                _ => {}
            }
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                self.clear_close_mode();
                if let Some(row_idx) = self.close_session_under_mouse(mouse, metrics) {
                    if let Some(session_id) =
                        self.session_at(row_idx).map(|session| session.id.clone())
                    {
                        self.rename = None;
                        self.open_context_menu(
                            ui::ContextMenuTarget::Session { session_id },
                            mouse,
                            metrics,
                        );
                    }
                } else if let Some(row_idx) = ui::group_row_from_mouse(
                    mouse.row,
                    metrics,
                    self.scroll,
                    self.rows.len(),
                    &self.rows,
                ) {
                    if let Some(cwd_label) = ui::group_label_at(&self.rows, row_idx) {
                        self.rename = None;
                        self.open_context_menu(
                            ui::ContextMenuTarget::Group {
                                cwd_label: cwd_label.to_string(),
                            },
                            mouse,
                            metrics,
                        );
                    }
                } else if let Some(note_index) = ui::notepad_note_title_row_from_mouse(
                    mouse.row,
                    metrics,
                    self.scroll,
                    self.sidebar_trail_base(),
                    &self.notepad_list_state(),
                ) {
                    if let Some(note_id) = self.notes.get(note_index).map(|note| note.id.clone()) {
                        self.rename = None;
                        self.open_context_menu(
                            ui::ContextMenuTarget::Note { note_id },
                            mouse,
                            metrics,
                        );
                    }
                } else if self.notepad_expanded
                    && ui::notepad_scrollable_hit(
                        mouse.column,
                        mouse.row,
                        metrics,
                        self.scroll,
                        self.sidebar_trail_base(),
                        &self.notepad_list_state(),
                    )
                {
                    self.open_notepad_context_menu(mouse, metrics);
                } else {
                    self.dismiss_context_menu();
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // IDE right-edge grip must not steal session/group row clicks.
                // Only start in-bar resize from chrome / empty list space.
                if self.is_edge_resize_hit(mouse.column, metrics)
                    && !self.mouse_over_list_content_row(mouse, metrics)
                {
                    self.begin_edge_resize(mouse.column);
                    return;
                }
                if let Some(menu) = self.context_menu.clone() {
                    let area = ratatui::layout::Rect {
                        x: 0,
                        y: 0,
                        width: metrics.frame_width,
                        height: metrics.frame_height,
                    };
                    if let Some(action) =
                        ui::context_menu_action_at(&menu, mouse.column, mouse.row, area)
                    {
                        match (&menu.target, action) {
                            (
                                ui::ContextMenuTarget::Session { session_id },
                                ui::ContextMenuAction::Rename,
                            ) => {
                                if let Some(row_idx) = self.session_row_index(session_id) {
                                    self.start_rename_for_session(session_id, row_idx);
                                }
                            }
                            (
                                ui::ContextMenuTarget::Session { session_id },
                                ui::ContextMenuAction::Delete,
                            ) => {
                                self.close_session_by_id(session_id);
                            }
                            (
                                ui::ContextMenuTarget::Group { cwd_label },
                                ui::ContextMenuAction::Delete,
                            ) => {
                                self.close_group(cwd_label);
                            }
                            (
                                ui::ContextMenuTarget::Note { note_id },
                                ui::ContextMenuAction::Rename,
                            ) => {
                                if let Some(row_idx) = self.note_title_row_index(note_id) {
                                    self.start_rename_for_note(note_id, row_idx);
                                }
                            }
                            (
                                ui::ContextMenuTarget::Note { note_id },
                                ui::ContextMenuAction::Delete,
                            ) => {
                                self.request_delete_note_confirm_by_id(note_id);
                            }
                            (ui::ContextMenuTarget::Notepad { .. }, action) => {
                                self.handle_notepad_context_menu_action(action);
                            }
                            _ => {}
                        }
                    }
                    self.context_menu = None;
                    self.force_redraw();
                    return;
                }
                if self.rename.is_some() || self.delete_note_confirm.is_some() {
                    return;
                }
                if let Some((cwd_label, agent_id)) = ui::group_launch_click(
                    mouse.column,
                    mouse.row,
                    metrics,
                    self.scroll,
                    self.rows.len(),
                    &self.rows,
                    &self.group_launch,
                )
                .map(|(label, agent)| (label.to_string(), agent))
                {
                    if agent_id == "console" {
                        self.create_console_in_group(&cwd_label);
                    } else {
                        self.create_agent_in_group(&cwd_label, &agent_id);
                    }
                    return;
                }
                if ui::notepad_section_add_click(
                    mouse.column,
                    mouse.row,
                    metrics,
                    self.scroll,
                    self.sidebar_trail_base(),
                    &self.notepad_list_state(),
                ) {
                    self.add_note();
                    return;
                }
                if ui::collapse_control_hit(mouse.column, mouse.row, metrics) {
                    self.clear_close_mode();
                    self.unfocus_notepad();
                    self.toggle_sidebar_rail();
                    return;
                }
                if ui::sessions_title_add_click(mouse.column, mouse.row, metrics) {
                    self.clear_close_mode();
                    self.unfocus_notepad();
                    self.create_session();
                    return;
                }
                if ui::sessions_title_hit(mouse.row, metrics) {
                    self.clear_close_mode();
                    self.unfocus_notepad();
                    self.toggle_sessions_expanded();
                    return;
                }
                let trail_base = self.sidebar_trail_base();
                match ui::notepad_hit_from_mouse(
                    mouse.column,
                    mouse.row,
                    metrics,
                    self.scroll,
                    trail_base,
                    &self.notepad_list_state(),
                ) {
                    Some(NotepadHit::SectionHeader) => {
                        self.clear_close_mode();
                        self.toggle_notepad_expanded(true);
                        return;
                    }
                    Some(NotepadHit::SectionAdd) => {
                        self.clear_close_mode();
                        self.add_note();
                        return;
                    }
                    Some(NotepadHit::NoteTitle { note_index }) => {
                        self.clear_close_mode();
                        self.begin_note_drag(note_index, mouse.row);
                        return;
                    }
                    Some(NotepadHit::NotesToggle) => {
                        self.clear_close_mode();
                        self.toggle_notes_list_expanded();
                        return;
                    }
                    Some(NotepadHit::NoteBodyScrollbar { note_index }) => {
                        self.clear_close_mode();
                        self.handle_notepad_scrollbar_click(mouse, metrics, trail_base, note_index);
                        return;
                    }
                    Some(NotepadHit::NoteBody { note_index }) => {
                        self.clear_close_mode();
                        self.activate_note(note_index);
                        self.handle_notepad_body_click(mouse, metrics, trail_base, note_index);
                        return;
                    }
                    None => self.unfocus_notepad(),
                }
                if let Some(action) = ui::toolbar_action_from_mouse(mouse.row, metrics) {
                    self.clear_close_mode();
                    self.run_toolbar_action(action);
                    return;
                }
                if let Some(action) = ui::update_banner_action_from_mouse(mouse.row, metrics) {
                    self.clear_close_mode();
                    self.run_update_banner_action(action);
                    return;
                }
                if ui::settings_action_from_mouse(mouse.row, metrics) {
                    self.clear_close_mode();
                    self.run_toolbar_action(ToolbarAction::Settings);
                    return;
                }
                if ui::leave_action_from_mouse(mouse.row, metrics) {
                    self.clear_close_mode();
                    self.run_toolbar_action(ToolbarAction::Leave);
                    return;
                }
                if self.close_modifier_held {
                    self.touch_close_hold();
                    if let Some(row_idx) = self.close_note_under_mouse(mouse, metrics) {
                        self.request_delete_note_confirm_at_row(row_idx);
                        return;
                    }
                    if let Some(row_idx) = ui::row_from_mouse(
                        mouse.row,
                        metrics.list_top_y,
                        metrics.list_height,
                        self.scroll,
                        self.rows.len(),
                    ) {
                        if self.session_at(row_idx).is_some() {
                            self.close_row(row_idx);
                        }
                    }
                    return;
                }
                let Some(row_idx) = ui::row_from_mouse(
                    mouse.row,
                    metrics.list_top_y,
                    metrics.list_height,
                    self.scroll,
                    self.rows.len(),
                ) else {
                    return;
                };
                if let Some(label) = ui::group_label_at(&self.rows, row_idx).map(str::to_string) {
                    if ui::is_group_trailing_click_for(
                        mouse.column,
                        metrics,
                        self.group_launch.len(),
                    ) {
                        return;
                    }
                    self.clear_list_text_selection();
                    self.group_hover_row = None;
                    self.hover_row = None;
                    self.group_drag = GroupDragState {
                        source: None,
                        hover: None,
                        dragged: false,
                        pending_click_label: Some(label),
                        pressed_at: Some(Instant::now()),
                        pressed_row: Some(mouse.row),
                        preserved_session_id: self
                            .session_at(self.selected)
                            .map(|session| session.id.clone()),
                        preserved_group_toggle: ui::group_toggle_at(&self.rows, self.selected)
                            .map(str::to_string),
                    };
                    self.force_redraw();
                } else if matches!(self.rows.get(row_idx), Some(RowKind::Session { .. })) {
                    // Session rows: Ghostty uses Down→text-select + Up→activate.
                    // Cursor/VS Code (xterm.js) often drops/mangles MouseUp and
                    // emits drag noise that used to steal activate-on-up — so on
                    // IDE hosts switch immediately on Down (create badges already
                    // fire on Down; sessions must match).
                    self.clear_list_text_selection();
                    if !self.host.allows_list_text_drag_select() {
                        self.set_selected(row_idx);
                        self.force_redraw();
                        self.clear_close_mode();
                        self.activate_selected();
                    } else if ui::pointer_in_list_body(mouse.column, metrics) {
                        self.begin_list_text_selection(mouse, metrics);
                    }
                } else if matches!(self.rows.get(row_idx), Some(RowKind::GroupToggle { .. }))
                    && !self.host.allows_list_text_drag_select()
                {
                    self.clear_list_text_selection();
                    self.set_selected(row_idx);
                    self.force_redraw();
                    self.clear_close_mode();
                    self.activate_selected();
                } else if ui::pointer_in_list_body(mouse.column, metrics) {
                    self.begin_list_text_selection(mouse, metrics);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.edge_resize_active {
                    self.update_edge_resize(mouse);
                    return;
                }
                if self.context_menu.is_some() {
                    self.update_context_menu_hover(mouse, metrics);
                    return;
                }
                self.maybe_clear_list_hover_suppress(mouse.row);
                self.maybe_engage_sidebar_drag(mouse, metrics);
                if self.notepad_editor.scrollbar_thumb_offset.is_some() {
                    self.update_notepad_scrollbar_drag(mouse, metrics, self.sidebar_trail_base());
                } else if self.notepad_editor.drag_selecting {
                    self.update_notepad_drag_selection(mouse, metrics);
                } else if self.note_drag.active() {
                    // `dragged` is set only when hover leaves the source note.
                    self.update_note_drag_hover(mouse, metrics);
                } else if self.list_select_anchor.is_some() && !self.group_drag.active() {
                    self.update_list_text_selection(mouse, metrics);
                } else if self.group_drag.active() {
                    // `dragged` is set only when hover leaves the source PWD —
                    // never from same-row Drag noise (xterm.js click jitter).
                    self.update_group_drag_hover(mouse, metrics);
                } else if self.close_modifier_held {
                    self.touch_close_hold();
                    self.update_close_target_hover(mouse, metrics);
                } else {
                    self.update_toolbar_hover(mouse, metrics);
                    self.update_group_hover(mouse, metrics);
                    self.update_session_hover(mouse, metrics);
                }
            }
            MouseEventKind::Moved => {
                if self.context_menu.is_some() {
                    self.update_context_menu_hover(mouse, metrics);
                    return;
                }
                // SGR: button-held motion is Drag only. Moved means the button is
                // up (or Up was dropped by xterm.js in Cursor/VS Code). Never keep
                // a PWD/note drag glued to bare hover — same class of bug as
                // edge-resize following the pointer after a lost MouseUp.
                if self.finish_sidebar_drag_if_button_released(mouse) {
                    return;
                }
                self.maybe_clear_list_hover_suppress(mouse.row);
                if self.close_modifier_held {
                    self.touch_close_hold();
                    self.update_close_target_hover(mouse, metrics);
                } else {
                    self.update_toolbar_hover(mouse, metrics);
                    self.update_group_hover(mouse, metrics);
                    self.update_session_hover(mouse, metrics);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.edge_resize_active {
                    self.finish_edge_resize();
                    return;
                }
                // IDE: never treat residual list-select state as a completed drag.
                if !self.host.allows_list_text_drag_select() {
                    self.clear_list_text_selection();
                }
                if self.context_menu.is_some()
                    || self.rename.is_some()
                    || self.delete_note_confirm.is_some()
                {
                    return;
                }
                if self.notepad_editor.scrollbar_thumb_offset.is_some() {
                    self.finish_notepad_scrollbar_drag();
                    return;
                }
                if self.notepad_editor.drag_selecting {
                    self.finish_notepad_drag_selection();
                    return;
                }
                if self.list_select_anchor.is_some() && self.finish_list_text_selection(metrics) {
                    return;
                }
                if self.close_modifier_held {
                    return;
                }
                if self.note_drag.pending() {
                    self.finish_note_drag_pending_click(mouse);
                    return;
                }
                if self.note_drag.active() {
                    self.finish_note_drag(mouse);
                    return;
                }
                if self.group_drag.pending() {
                    self.finish_group_drag_pending_click();
                    return;
                }
                if self.group_drag.active() {
                    self.finish_active_group_drag(mouse);
                    return;
                }

                let Some(row_idx) = ui::row_from_mouse(
                    mouse.row,
                    metrics.list_top_y,
                    metrics.list_height,
                    self.scroll,
                    self.rows.len(),
                ) else {
                    return;
                };
                if ui::selectable_indices(&self.rows).contains(&row_idx) {
                    self.set_selected(row_idx);
                    self.force_redraw();
                    self.clear_close_mode();
                    self.activate_selected();
                }
            }
            MouseEventKind::ScrollUp => {
                self.clear_close_mode();
                let trail_base = self.sidebar_trail_base();
                if ui::notepad_scrollable_hit(
                    mouse.column,
                    mouse.row,
                    metrics,
                    self.scroll,
                    trail_base,
                    &self.notepad_list_state(),
                ) {
                    self.scroll_notepad_lines(-1);
                } else if ui::pointer_in_list_viewport_y(mouse.row, metrics) {
                    self.scroll_list_viewport(-1, metrics);
                }
            }
            MouseEventKind::ScrollDown => {
                self.clear_close_mode();
                let trail_base = self.sidebar_trail_base();
                if ui::notepad_scrollable_hit(
                    mouse.column,
                    mouse.row,
                    metrics,
                    self.scroll,
                    trail_base,
                    &self.notepad_list_state(),
                ) {
                    self.scroll_notepad_lines(1);
                } else if ui::pointer_in_list_viewport_y(mouse.row, metrics) {
                    self.scroll_list_viewport(1, metrics);
                }
            }
            _ => {}
        }
    }
    pub(crate) fn pointer_over_sidebar_target(
        &self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) -> bool {
        ui::toolbar_action_from_mouse(mouse.row, metrics).is_some()
            || ui::settings_action_from_mouse(mouse.row, metrics)
            || ui::leave_action_from_mouse(mouse.row, metrics)
            || ui::notepad_hit_from_mouse(
                mouse.column,
                mouse.row,
                metrics,
                self.scroll,
                self.sidebar_trail_base(),
                &self.notepad_list_state(),
            )
            .is_some()
            || ui::sessions_title_hit(mouse.row, metrics)
            || ui::group_row_from_mouse(
                mouse.row,
                metrics,
                self.scroll,
                self.rows.len(),
                &self.rows,
            )
            .is_some()
            || self.session_row_under_mouse(mouse, metrics).is_some()
    }
    pub(crate) fn refresh_pointer_hover_from_mouse(&mut self, metrics: &ui::LayoutMetrics) {
        let Some(mouse) = self.last_mouse else {
            return;
        };
        if self.close_modifier_held {
            self.update_close_target_hover(&mouse, metrics);
        } else {
            self.update_toolbar_hover(&mouse, metrics);
            self.update_group_hover(&mouse, metrics);
            self.update_session_hover(&mouse, metrics);
        }
    }
    pub(crate) fn workspace_panel_open(&self) -> bool {
        self.workspace_settings_open
            || self.workspace_new_session_open
            || self.workspace_automations_open
            || self.workspace_mcps_open
            || self.workspace_skills_open
    }

    fn clear_workspace_panel_flags(&mut self) {
        self.workspace_settings_open = false;
        self.workspace_new_session_open = false;
        self.workspace_automations_open = false;
        self.workspace_mcps_open = false;
        self.workspace_skills_open = false;
        self.workspace_panel_open_grace_until = None;
    }

    fn mark_workspace_panel_opening(&mut self) {
        // Async spawn (tmux run-shell -b) lags behind the click; keep optimistic
        // selection on the toolbar until the pane is confirmed open/closed.
        self.workspace_panel_open_grace_until = Some(Instant::now() + WORKSPACE_PANEL_OPEN_GRACE);
        // Defer the next probe so we don't immediately wipe optimistic flags.
        self.last_workspace_panel_probe = Instant::now();
    }

    pub(crate) fn restore_workspace_if_panel_open(&mut self) {
        let panels = crate::daemon::tmux::workspace_pane_panel_state(&self.config.tmux_ui_session)
            .unwrap_or_default();
        if !panels.any() {
            // Optimistic flags may still be set while the pane is spawning.
            if self.workspace_panel_open() {
                self.clear_workspace_panel_flags();
                self.force_redraw();
            }
            return;
        }
        if crate::daemon::tmux::restore_workspace_attach(
            &self.config.tmux_ui_session,
            &self.config.tmux_session,
        )
        .is_ok()
        {
            self.clear_workspace_panel_flags();
            self.force_redraw();
        }
    }
    pub(crate) fn run_toolbar_action(&mut self, action: ToolbarAction) {
        self.unfocus_notepad();
        if action != ToolbarAction::Settings
            && action != ToolbarAction::NewSession
            && action != ToolbarAction::Automations
            && action != ToolbarAction::Mcps
            && action != ToolbarAction::Skills
            && action != ToolbarAction::Leave
        {
            self.restore_workspace_if_panel_open();
        }
        match action {
            ToolbarAction::NewSession => self.open_new_session_panel(),
            ToolbarAction::Automations => self.open_automations_panel(),
            ToolbarAction::Mcps => self.open_mcps_panel(),
            ToolbarAction::Skills => self.open_skills_panel(),
            ToolbarAction::Search => {
                self.pulse_coming_soon(action);
            }
            ToolbarAction::Settings => self.open_sessions_settings(),
            ToolbarAction::Leave => self.detach_client(),
        }
        self.force_redraw();
    }
    pub(crate) fn run_update_banner_action(&mut self, action: ui::UpdateBannerAction) {
        self.unfocus_notepad();
        match action {
            ui::UpdateBannerAction::Upgrade => {
                crate::telemetry::record_feature(
                    crate::telemetry::FeatureId::UpdateBannerUpgrade,
                    crate::telemetry::feature::Source::Mouse,
                );
                let binary = crate::paths::resolve_binary(&self.config.home);
                std::thread::spawn(move || {
                    let _ = std::process::Command::new(binary).arg("upgrade").status();
                });
                self.show_status_notice("Upgrading sessions…");
            }
            ui::UpdateBannerAction::Dismiss => {
                crate::telemetry::record_feature(
                    crate::telemetry::FeatureId::UpdateBannerDismiss,
                    crate::telemetry::feature::Source::Mouse,
                );
                if let Some(version) = self
                    .update_banner
                    .as_ref()
                    .map(|banner| banner.version.clone())
                {
                    if let Ok(mut cfg) =
                        crate::telemetry::config::SessionsConfig::load(&self.config.home)
                    {
                        let _ = cfg.dismiss_update(&version);
                    }
                }
                self.update_banner = None;
                self.update_upgrade_hover = false;
                self.update_dismiss_hover = false;
            }
        }
        self.force_redraw();
    }
    pub(crate) fn pulse_coming_soon(&mut self, action: ToolbarAction) {
        self.coming_soon_anims.insert(action, Instant::now());
    }
    pub(crate) fn note_pointer_activity(&mut self, mouse: &MouseEvent, pane_width: u16) {
        self.last_mouse_activity = Instant::now();
        self.pointer_near_exit = mouse.column >= pane_width;
        // Engage tmux pane focus on click only — not on hover/move.
        if mouse.column < pane_width
            && matches!(mouse.kind, MouseEventKind::Down(_) | MouseEventKind::Up(_))
        {
            self.ensure_sidebar_pane_engaged();
        } else if mouse.column >= pane_width && matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.ensure_workspace_pane_engaged();
        }
    }
    pub(crate) fn refresh_sidebar_mouse_capture(&mut self) {
        // Workspace agents replace terminal mouse mode; restore SGR tracking whenever
        // the sidebar pane is engaged so Moved events reach hover handlers again.
        let _ = execute!(io::stdout(), EnableMouseCapture);
    }
    pub(crate) fn ensure_workspace_pane_engaged(&mut self) {
        if self.last_workspace_pane_focused && !self.sidebar_pane_focused {
            return;
        }
        if self.last_sidebar_engage.elapsed() < SIDEBAR_ENGAGE_THROTTLE {
            return;
        }
        if crate::daemon::tmux::select_ui_workspace_pane(&self.config.tmux_ui_session).is_ok() {
            self.last_workspace_pane_focused = true;
            self.sidebar_pane_focused = false;
            self.last_sidebar_engage = Instant::now();
            self.last_sidebar_focus_probe = Instant::now();
            self.clear_pointer_hover_states();
            self.collapse_sidebar_rail_if_narrow();
        }
    }
    pub(crate) fn ensure_sidebar_pane_engaged(&mut self) {
        if self.is_sidebar_rail_collapsed() {
            self.expand_sidebar_from_rail();
        }
        if self.sidebar_pane_focused && !self.last_workspace_pane_focused {
            self.refresh_sidebar_mouse_capture();
            self.pointer_hover_refresh_pending = true;
            return;
        }
        if self.last_sidebar_engage.elapsed() < SIDEBAR_ENGAGE_THROTTLE {
            return;
        }
        if crate::daemon::tmux::select_own_pane().is_ok() {
            self.sidebar_pane_focused = true;
            self.last_workspace_pane_focused = false;
            self.last_sidebar_engage = Instant::now();
            self.last_sidebar_focus_probe = Instant::now();
            self.pointer_hover_refresh_pending = true;
            self.refresh_sidebar_mouse_capture();
        }
    }
    pub(crate) fn sync_sidebar_mouse_cursor(&mut self, metrics: Option<&ui::LayoutMetrics>) {
        let shape = self.resolve_sidebar_mouse_cursor(metrics);
        match shape {
            Some(cursor) => {
                let _ = mouse_cursor::set_mouse_cursor(cursor);
                self.last_synced_mouse_cursor = Some(cursor);
            }
            None if self.last_synced_mouse_cursor.is_some() => {
                let _ = mouse_cursor::reset_mouse_cursor();
                self.last_synced_mouse_cursor = None;
            }
            None => {}
        }
    }
    pub(crate) fn resolve_sidebar_mouse_cursor(
        &self,
        metrics: Option<&ui::LayoutMetrics>,
    ) -> Option<MouseCursorShape> {
        // xterm.js ignores OSC 22 — skip the whole path in IDE hosts.
        if !self.host.supports_osc22() {
            return None;
        }
        if !self.pointer_in_sidebar_pane(metrics) {
            return None;
        }
        if self.edge_resize_active {
            return Some(MouseCursorShape::Pointer);
        }
        if let (Some(metrics), Some(mouse)) = (metrics, self.last_mouse.as_ref()) {
            if self.is_edge_resize_hit(mouse.column, metrics) {
                return Some(MouseCursorShape::Pointer);
            }
        }
        if self.close_modifier_held {
            if let (Some(metrics), Some(mouse)) = (metrics, self.last_mouse.as_ref()) {
                if self.close_session_under_mouse(mouse, metrics).is_some()
                    || self.close_note_under_mouse(mouse, metrics).is_some()
                {
                    return Some(MouseCursorShape::Pointer);
                }
            }
            return Some(MouseCursorShape::Default);
        }
        if self.pointer_over_text_edit(metrics) {
            return Some(MouseCursorShape::Text);
        }
        if self.sidebar_pointer_hover_active(metrics) {
            Some(MouseCursorShape::Pointer)
        } else {
            Some(MouseCursorShape::Default)
        }
    }
    pub(crate) fn pointer_in_sidebar_pane(&self, metrics: Option<&ui::LayoutMetrics>) -> bool {
        if let Some(mouse) = &self.last_mouse {
            let frame_width = metrics.map(|m| m.frame_width).unwrap_or(u16::MAX);
            if mouse.column < frame_width {
                return true;
            }
        }
        self.last_mouse_activity.elapsed() < SIDEBAR_POINTER_CURSOR_HOLD
    }
    pub(crate) fn sidebar_pointer_hover_active(&self, metrics: Option<&ui::LayoutMetrics>) -> bool {
        if self.close_modifier_held || self.has_pointer_hover() {
            return true;
        }
        if let (Some(metrics), Some(mouse)) = (metrics, self.last_mouse.as_ref()) {
            return self.pointer_over_sidebar_target(mouse, metrics);
        }
        false
    }
    pub(crate) fn pointer_over_text_edit(&self, metrics: Option<&ui::LayoutMetrics>) -> bool {
        if self.rename.is_some() {
            return true;
        }
        let (Some(metrics), Some(mouse)) = (metrics, self.last_mouse.as_ref()) else {
            return self.notepad_focused && self.notepad_expanded;
        };
        match ui::notepad_hit_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            self.sidebar_trail_base(),
            &self.notepad_list_state(),
        ) {
            Some(ui::NotepadHit::NoteTitle { .. }) => true,
            Some(ui::NotepadHit::NoteBody { note_index }) => {
                if self.notepad_checkbox_hover_from_mouse(mouse, metrics, note_index) {
                    return false;
                }
                self.notepad_focused && self.notepad_expanded
            }
            Some(ui::NotepadHit::NoteBodyScrollbar { .. }) => {
                self.notepad_focused && self.notepad_expanded
            }
            _ => false,
        }
    }
    pub(crate) fn has_pointer_hover(&self) -> bool {
        self.hover_row.is_some()
            || self.group_hover_row.is_some()
            || self.toolbar_hover.is_some()
            || self.settings_hover
            || self.leave_hover
            || self.notepad_section_header_hover
            || self.notepad_section_add_hover
            || self.notepad_note_hover.is_some()
            || self.sessions_title_hover
            || self.sessions_title_add_hover
            || self.collapse_control_hover
    }
    pub(crate) fn clear_stale_pointer_hover_after_exit(&mut self, metrics: &ui::LayoutMetrics) {
        if !self.pointer_near_exit || !self.has_pointer_hover() || self.close_modifier_held {
            return;
        }
        if self.last_mouse_activity.elapsed() < POINTER_EXIT_HOVER_CLEAR {
            return;
        }
        if self
            .last_mouse
            .is_some_and(|mouse| self.pointer_over_sidebar_target(&mouse, metrics))
        {
            return;
        }
        self.clear_pointer_hover_states();
    }
    pub(crate) fn sync_sidebar_pane_focus(&mut self) {
        if self.last_sidebar_focus_probe.elapsed()
            < self.effective_probe_interval(SIDEBAR_FOCUS_PROBE_INTERVAL)
        {
            return;
        }
        self.last_sidebar_focus_probe = Instant::now();
        // Host is cached for 60s inside detect_for_ui_session — no per-tick
        // show-options storm. Still re-check so attach/host flips settle.
        let next_host =
            crate::bar::host_terminal::detect_for_ui_session(Some(&self.config.tmux_ui_session));
        if next_host != self.host {
            self.host = next_host;
            self.force_redraw();
        }

        let prev = self.sidebar_pane_focused;
        let Some((workspace_focused, sidebar_active)) =
            crate::daemon::tmux::ui_window_focus_snapshot(&self.config.tmux_ui_session)
        else {
            return;
        };
        if self.last_workspace_pane_focused && !workspace_focused {
            self.rows_version = self.rows_version.wrapping_add(1);
        }
        let was_workspace = self.last_workspace_pane_focused;
        self.last_workspace_pane_focused = workspace_focused;
        if workspace_focused {
            self.sidebar_pane_focused = false;
            // No focus-stealing hover probe: clear highlights once the workspace
            // owns focus so stale hover does not stick without live MouseMove.
            if !was_workspace {
                self.clear_pointer_hover_states();
                self.collapse_sidebar_rail_if_narrow();
            }
            if self.notepad_focused {
                self.unfocus_notepad();
            }
            return;
        }

        if !prev && sidebar_active {
            self.pointer_hover_refresh_pending = true;
            self.refresh_sidebar_mouse_capture();
        }
        self.sidebar_pane_focused = sidebar_active;
        if prev && !sidebar_active && self.notepad_focused {
            self.unfocus_notepad();
        }
    }
    pub(crate) fn clear_pointer_hover_states(&mut self) {
        let mut changed = false;
        if self.close_modifier_held {
            if self.group_hover_row.is_some() {
                self.group_hover_row = None;
                changed = true;
            }
            if self.toolbar_hover.is_some() {
                self.toolbar_hover = None;
                changed = true;
            }
            if self.settings_hover {
                self.settings_hover = false;
                changed = true;
            }
            if self.leave_hover {
                self.leave_hover = false;
                changed = true;
            }
            if self.update_upgrade_hover {
                self.update_upgrade_hover = false;
                changed = true;
            }
            if self.update_dismiss_hover {
                self.update_dismiss_hover = false;
                changed = true;
            }
            if self.notepad_section_header_hover {
                self.notepad_section_header_hover = false;
                changed = true;
            }
            if self.notepad_section_add_hover {
                self.notepad_section_add_hover = false;
                changed = true;
            }
            if self.notepad_note_hover.is_some() {
                self.notepad_note_hover = None;
                changed = true;
            }
            if self.sessions_title_hover {
                self.sessions_title_hover = false;
                changed = true;
            }
            if self.sessions_title_add_hover {
                self.sessions_title_add_hover = false;
                changed = true;
            }
            if self.collapse_control_hover {
                self.collapse_control_hover = false;
                changed = true;
            }
            if self.hover_row.is_some() {
                self.hover_row = None;
                changed = true;
            }
        } else {
            if self.hover_row.is_some() {
                self.hover_row = None;
                changed = true;
            }
            if self.group_hover_row.is_some() {
                self.group_hover_row = None;
                changed = true;
            }
            if self.toolbar_hover.is_some() {
                self.toolbar_hover = None;
                changed = true;
            }
            if self.settings_hover {
                self.settings_hover = false;
                changed = true;
            }
            if self.leave_hover {
                self.leave_hover = false;
                changed = true;
            }
            if self.update_upgrade_hover {
                self.update_upgrade_hover = false;
                changed = true;
            }
            if self.update_dismiss_hover {
                self.update_dismiss_hover = false;
                changed = true;
            }
            if self.notepad_section_header_hover {
                self.notepad_section_header_hover = false;
                changed = true;
            }
            if self.notepad_section_add_hover {
                self.notepad_section_add_hover = false;
                changed = true;
            }
            if self.notepad_note_hover.is_some() {
                self.notepad_note_hover = None;
                changed = true;
            }
            if self.sessions_title_hover {
                self.sessions_title_hover = false;
                changed = true;
            }
            if self.sessions_title_add_hover {
                self.sessions_title_add_hover = false;
                changed = true;
            }
            if self.collapse_control_hover {
                self.collapse_control_hover = false;
                changed = true;
            }
        }
        if changed {
            self.pointer_near_exit = false;
            self.last_synced_mouse_cursor = None;
            self.sync_sidebar_mouse_cursor(None);
            self.force_redraw();
        }
    }
    pub(crate) fn sync_workspace_panel_state(&mut self, force: bool) {
        // Always poll on interval so we can discover open panels after async spawn
        // and clear selection when the user Escapes from inside a panel.
        if !force
            && self.last_workspace_panel_probe.elapsed()
                < self.effective_probe_interval(WORKSPACE_PANEL_PROBE_INTERVAL)
        {
            return;
        }
        self.last_workspace_panel_probe = Instant::now();
        // Timed-out / failed tmux probes must be cheap no-ops — do not treat as
        // "all panels closed" (that forced redraw storms and cleared optimistic open).
        let Ok(panels) =
            crate::daemon::tmux::workspace_pane_panel_state(&self.config.tmux_ui_session)
        else {
            return;
        };

        // During spawn grace, ignore all-false probes (pane not respawned yet).
        if !panels.any() {
            if self
                .workspace_panel_open_grace_until
                .is_some_and(|until| Instant::now() < until)
            {
                return;
            }
            self.workspace_panel_open_grace_until = None;
        } else {
            // Pane confirmed open — drop grace so Esc/close probes apply immediately.
            self.workspace_panel_open_grace_until = None;
        }

        if panels.settings != self.workspace_settings_open
            || panels.new_session != self.workspace_new_session_open
            || panels.automations != self.workspace_automations_open
            || panels.mcps != self.workspace_mcps_open
            || panels.skills != self.workspace_skills_open
        {
            self.workspace_settings_open = panels.settings;
            self.workspace_new_session_open = panels.new_session;
            self.workspace_automations_open = panels.automations;
            self.workspace_mcps_open = panels.mcps;
            self.workspace_skills_open = panels.skills;
            self.force_redraw();
        }
    }
    pub(crate) fn open_new_session_panel(&mut self) {
        if crate::daemon::tmux::workspace_pane_running_new_session(&self.config.tmux_ui_session)
            .unwrap_or(false)
            || self.workspace_new_session_open
        {
            // Already open — keep panel; selection already on New Session.
            self.workspace_new_session_open = true;
            self.force_redraw();
            return;
        }
        // Exclusive optimistic open: only one right-pane panel is selected.
        self.clear_workspace_panel_flags();
        self.workspace_new_session_open = true;
        self.mark_workspace_panel_opening();
        self.spawn_workspace_panel(crate::daemon::tmux::spawn_open_workspace_new_session);
    }
    pub(crate) fn open_automations_panel(&mut self) {
        if crate::daemon::tmux::workspace_pane_running_automations(&self.config.tmux_ui_session)
            .unwrap_or(false)
            || self.workspace_automations_open
        {
            self.restore_workspace_if_panel_open();
            return;
        }
        self.clear_workspace_panel_flags();
        self.workspace_automations_open = true;
        self.mark_workspace_panel_opening();
        self.spawn_workspace_panel(crate::daemon::tmux::spawn_open_workspace_automations);
    }
    pub(crate) fn open_mcps_panel(&mut self) {
        // Toggle: second click / ⌘M while open restores the agent attach.
        if crate::daemon::tmux::workspace_pane_running_mcps(&self.config.tmux_ui_session)
            .unwrap_or(false)
            || self.workspace_mcps_open
        {
            self.restore_workspace_if_panel_open();
            return;
        }
        self.clear_workspace_panel_flags();
        self.workspace_mcps_open = true;
        self.mark_workspace_panel_opening();
        self.spawn_workspace_panel(crate::daemon::tmux::spawn_open_workspace_mcps);
    }
    pub(crate) fn open_skills_panel(&mut self) {
        if crate::daemon::tmux::workspace_pane_running_skills(&self.config.tmux_ui_session)
            .unwrap_or(false)
            || self.workspace_skills_open
        {
            self.restore_workspace_if_panel_open();
            return;
        }
        self.clear_workspace_panel_flags();
        self.workspace_skills_open = true;
        self.mark_workspace_panel_opening();
        self.spawn_workspace_panel(crate::daemon::tmux::spawn_open_workspace_skills);
    }
    pub(crate) fn open_sessions_settings(&mut self) {
        if crate::daemon::tmux::workspace_pane_running_settings(&self.config.tmux_ui_session)
            .unwrap_or(false)
            || self.workspace_settings_open
        {
            self.restore_workspace_if_panel_open();
            return;
        }
        self.clear_workspace_panel_flags();
        self.workspace_settings_open = true;
        self.mark_workspace_panel_opening();
        self.spawn_workspace_panel(crate::daemon::tmux::spawn_toggle_workspace_settings);
    }
    pub(crate) fn spawn_workspace_panel(&mut self, spawn: fn() -> anyhow::Result<()>) {
        match spawn() {
            Ok(()) => {
                // Do not force-sync here: run-shell -b has not opened the pane yet,
                // and an empty probe would wipe the optimistic toolbar selection.
                self.force_redraw();
            }
            Err(error) => {
                self.clear_workspace_panel_flags();
                let _ = crate::daemon::tmux::run_tmux(&[
                    "display-message",
                    &format!("sessions panel failed: {error}"),
                ]);
                self.force_redraw();
            }
        }
    }
    pub(crate) fn toggle_workspace_panel(
        &mut self,
        toggle: fn(&str, &str) -> anyhow::Result<bool>,
    ) {
        match toggle(&self.config.tmux_ui_session, &self.config.tmux_session) {
            Ok(_) => {
                // After a sync toggle, re-probe soon (pane state should be settled).
                self.last_workspace_panel_probe = Instant::now()
                    .checked_sub(WORKSPACE_PANEL_PROBE_INTERVAL)
                    .unwrap_or_else(Instant::now);
                self.sync_workspace_panel_state(true);
            }
            Err(error) => {
                let _ = crate::daemon::tmux::run_tmux(&[
                    "display-message",
                    &format!("sessions panel failed: {error}"),
                ]);
            }
        }
    }
    pub(crate) fn update_toolbar_hover(&mut self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) {
        let next_toolbar = ui::toolbar_action_from_mouse(mouse.row, metrics);
        let next_settings = ui::settings_action_from_mouse(mouse.row, metrics);
        let next_leave = ui::leave_action_from_mouse(mouse.row, metrics);
        let (next_update_upgrade, next_update_dismiss) =
            ui::update_banner_hover_from_mouse(mouse.row, metrics);
        let trail_base = self.sidebar_trail_base();
        let next_notepad_header = ui::notepad_section_header_hover_from_mouse(
            mouse.row,
            metrics,
            self.scroll,
            trail_base,
            &self.notepad_list_state(),
        );
        let next_notepad_add = ui::notepad_section_add_hover_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            trail_base,
            &self.notepad_list_state(),
        );
        let next_note_hover =
            if self.note_drag.active() || self.list_hover_updates_suppressed(mouse.row) {
                None
            } else if self.note_drag.pending() {
                self.effective_note_hover()
            } else {
                ui::notepad_note_hover_from_mouse(
                    mouse.row,
                    metrics,
                    self.scroll,
                    trail_base,
                    &self.notepad_list_state(),
                )
            };
        let next_sessions_title = ui::sessions_title_hit(mouse.row, metrics);
        let next_sessions_title_add =
            ui::sessions_title_add_hover_from_mouse(mouse.column, mouse.row, metrics);
        let next_collapse_control =
            ui::collapse_control_hover_from_mouse(mouse.column, mouse.row, metrics);
        if self.toolbar_hover != next_toolbar
            || self.settings_hover != next_settings
            || self.leave_hover != next_leave
            || self.update_upgrade_hover != next_update_upgrade
            || self.update_dismiss_hover != next_update_dismiss
            || self.notepad_section_header_hover != next_notepad_header
            || self.notepad_section_add_hover != next_notepad_add
            || self.notepad_note_hover != next_note_hover
            || self.sessions_title_hover != next_sessions_title
            || self.sessions_title_add_hover != next_sessions_title_add
            || self.collapse_control_hover != next_collapse_control
        {
            self.toolbar_hover = next_toolbar;
            self.settings_hover = next_settings;
            self.leave_hover = next_leave;
            self.update_upgrade_hover = next_update_upgrade;
            self.update_dismiss_hover = next_update_dismiss;
            self.notepad_section_header_hover = next_notepad_header;
            self.notepad_section_add_hover = next_notepad_add;
            self.notepad_note_hover = next_note_hover;
            self.sessions_title_hover = next_sessions_title;
            self.sessions_title_add_hover = next_sessions_title_add;
            self.collapse_control_hover = next_collapse_control;
        }
    }
    pub(crate) fn dismiss_context_menu(&mut self) {
        if self.context_menu.is_some() {
            self.context_menu = None;
            self.force_redraw();
        }
    }

    pub(crate) fn open_context_menu(
        &mut self,
        target: ui::ContextMenuTarget,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        let mut menu = ui::ContextMenu {
            target,
            x: mouse.column,
            y: mouse.row,
            hover: None,
        };
        let area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: metrics.frame_width,
            height: metrics.frame_height,
        };
        menu.hover = ui::context_menu_action_at(&menu, mouse.column, mouse.row, area);
        self.context_menu = Some(menu);
        self.force_redraw();
    }

    pub(crate) fn update_context_menu_hover(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        let Some(menu) = self.context_menu.as_ref() else {
            return;
        };
        let area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: metrics.frame_width,
            height: metrics.frame_height,
        };
        let next = ui::context_menu_action_at(menu, mouse.column, mouse.row, area);
        if menu.hover == next {
            return;
        }
        if let Some(menu) = self.context_menu.as_mut() {
            menu.hover = next;
        }
        self.force_redraw();
    }
    pub(crate) fn try_start_rename_for_hover(&mut self) -> bool {
        let Some(row_idx) = self.hover_row else {
            return false;
        };
        let Some(session) = self.session_at(row_idx) else {
            return false;
        };
        let session_id = session.id.clone();
        self.start_rename_for_session(&session_id, row_idx);
        true
    }
    pub(crate) fn start_rename_for_session(&mut self, session_id: &str, row_idx: usize) {
        let Some(label) = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(ui::session_display_label)
        else {
            return;
        };
        self.set_selected(row_idx);
        self.rename = Some(ui::RenameState {
            target: ui::RenameTarget::Session {
                session_id: session_id.to_string(),
            },
            row_idx,
            buffer: label,
            select_all: true,
        });
        self.context_menu = None;
        self.rows_version = self.rows_version.wrapping_add(1);
        self.last_synced_mouse_cursor = None;
        self.sync_sidebar_mouse_cursor(None);
        self.force_redraw();
    }
    pub(crate) fn open_url(&self, url: &str) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(url).status();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(url).status();
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = url;
        }
    }
    pub(crate) fn handle_rename_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        _terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                self.rename = None;
                self.last_synced_mouse_cursor = None;
                self.sync_sidebar_mouse_cursor(None);
                self.force_redraw();
            }
            KeyCode::Enter | KeyCode::Tab => {
                self.commit_rename_with_advance(key.code == KeyCode::Tab);
            }
            KeyCode::Backspace => {
                if let Some(rename) = self.rename.as_mut() {
                    ui::rename_apply_backspace(rename);
                    self.force_redraw();
                }
            }
            KeyCode::Delete => {
                if let Some(rename) = self.rename.as_mut() {
                    if rename.select_all {
                        rename.buffer.clear();
                        rename.select_all = false;
                        self.force_redraw();
                    }
                }
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End => {
                if let Some(rename) = self.rename.as_mut() {
                    if rename.select_all {
                        ui::rename_deselect(rename);
                        self.force_redraw();
                    }
                }
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(rename) = self.rename.as_mut() {
                    rename.select_all = true;
                    self.force_redraw();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(rename) = self.rename.as_mut() {
                    ui::rename_apply_char(rename, c);
                    self.force_redraw();
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn commit_rename(&mut self) {
        self.commit_rename_with_advance(true);
    }

    pub(crate) fn commit_rename_with_advance(&mut self, _advance: bool) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        self.last_synced_mouse_cursor = None;
        self.sync_sidebar_mouse_cursor(None);
        match rename.target {
            ui::RenameTarget::Session { session_id } => {
                let buffer = rename.buffer.trim().to_string();
                if buffer.is_empty() {
                    self.force_redraw();
                    return;
                }
                if let Ok(Some(patch)) = self.client.rename(&session_id, buffer) {
                    self.apply_event(ClientEvent::Patch(patch));
                }
            }
            ui::RenameTarget::Note { note_id } => {
                let buffer = rename.buffer.trim().to_string();
                if buffer.is_empty() {
                    self.force_redraw();
                    return;
                }
                if let Some(note) = self.notes.iter_mut().find(|note| note.id == note_id) {
                    note.title = buffer;
                    self.persist_notepad_now();
                }
                self.force_redraw();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bar::app::test_fixtures::{sample_session, sample_session_in_group};
    use crate::bar::app::{
        App, POINTER_EXIT_HOVER_CLEAR, SIDEBAR_FOCUS_PROBE_INTERVAL, SIDEBAR_POINTER_CURSOR_HOLD,
        WORKSPACE_PANEL_OPEN_GRACE, WORKSPACE_PANEL_PROBE_INTERVAL,
    };
    use crate::bar::group_order;
    use crate::bar::ui::{self, RowKind, ToolbarAction};
    use crate::config::Config;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn isolated_config(dir: &TempDir) -> Config {
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        config
    }

    #[test]
    fn pointer_hover_updates_on_non_group_drag() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", false),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();
        let first = app
            .rows
            .iter()
            .position(|row| matches!(row, RowKind::Session { .. }))
            .unwrap();
        app.hover_row = Some(first);
        app.group_hover_row = Some(1);

        let size = ratatui::layout::Size::new(40, 20);
        let metrics = ui::layout_metrics(size, &app.rows);
        let second = app
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, RowKind::Session { .. }))
            .nth(1)
            .map(|(idx, _)| idx)
            .unwrap();
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 10,
            row: metrics.list_top_y + (second - app.scroll) as u16,
            modifiers: KeyModifiers::empty(),
        };
        app.handle_mouse(&drag, &metrics);

        assert_eq!(app.hover_row, Some(second));
        assert_eq!(app.group_hover_row, None);
    }

    #[test]
    fn rename_starts_for_hovered_session() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![sample_session("tmux:win:1", 1, "one", false)];
        app.rebuild_rows();
        let row_idx = app
            .rows
            .iter()
            .position(|row| matches!(row, RowKind::Session { .. }))
            .unwrap();
        app.hover_row = Some(row_idx);

        assert!(app.try_start_rename_for_hover());
        assert_eq!(
            app.rename.as_ref().map(|rename| match &rename.target {
                ui::RenameTarget::Session { session_id } => session_id.as_str(),
                ui::RenameTarget::Note { .. } => "",
            }),
            Some("tmux:win:1")
        );
        assert!(app.rename.as_ref().is_some_and(|rename| rename.select_all));
    }

    #[test]
    fn pointer_hover_clears_when_sidebar_loses_focus() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.hover_row = Some(3);
        app.group_hover_row = Some(1);
        app.toolbar_hover = Some(ToolbarAction::NewSession);
        app.settings_hover = true;

        app.clear_pointer_hover_states();

        assert_eq!(app.hover_row, None);
        assert_eq!(app.group_hover_row, None);
        assert_eq!(app.toolbar_hover, None);
        assert!(!app.settings_hover);
    }

    #[test]
    fn right_edge_pointer_does_not_clear_hover_over_row() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![sample_session("tmux:win:1", 1, "one", false)];
        app.rebuild_rows();
        let session_row = app
            .rows
            .iter()
            .position(|row| matches!(row, RowKind::Session { .. }))
            .unwrap();
        app.hover_row = Some(session_row);
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let mouse = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 39,
            row: metrics.list_top_y,
            modifiers: KeyModifiers::empty(),
        };
        app.last_mouse = Some(mouse);
        app.last_mouse_activity =
            Instant::now() - POINTER_EXIT_HOVER_CLEAR - Duration::from_millis(1);
        app.pointer_near_exit = false;
        app.clear_stale_pointer_hover_after_exit(&metrics);
        assert_eq!(app.hover_row, Some(session_row));
    }

    #[test]
    fn ensure_sidebar_pane_engaged_uses_cached_workspace_focus() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.sidebar_pane_focused = true;
        app.last_workspace_pane_focused = false;
        app.last_sidebar_engage = Instant::now()
            .checked_sub(super::super::SIDEBAR_ENGAGE_THROTTLE)
            .unwrap_or_else(Instant::now);
        app.ensure_sidebar_pane_engaged();
        assert!(app.sidebar_pane_focused);
    }

    #[test]
    fn needs_sidebar_hover_poll_when_sidebar_pane_focused() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.sidebar_pane_focused = true;
        app.last_workspace_pane_focused = false;
        app.user_pane_width = Some(40);
        app.last_mouse = Some(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::empty(),
        });
        app.last_mouse_activity = Instant::now();
        assert!(app.needs_sidebar_hover_poll());
        assert!(app.needs_pointer_hover_poll());
    }

    #[test]
    fn no_hover_poll_while_workspace_pane_focused() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.last_workspace_pane_focused = true;
        app.sidebar_pane_focused = false;
        app.user_pane_width = Some(40);
        app.last_mouse = Some(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::empty(),
        });
        app.last_mouse_activity = Instant::now();
        // Workspace-focused hover used to focus-flash the sidebar (~30Hz) and
        // corrupt the nested workspace redraw. Hover only while sidebar focused.
        assert!(!app.needs_sidebar_hover_poll());
        assert!(!app.needs_pointer_hover_poll());
    }

    #[test]
    fn clear_pointer_hover_when_leaving_sidebar_focus() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.hover_row = Some(3);
        app.group_hover_row = Some(1);
        app.toolbar_hover = Some(ToolbarAction::NewSession);
        app.clear_pointer_hover_states();
        assert_eq!(app.hover_row, None);
        assert_eq!(app.group_hover_row, None);
        assert_eq!(app.toolbar_hover, None);
    }

    #[test]
    fn restore_workspace_clears_stale_cached_flags_without_tmux_dismiss() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.workspace_new_session_open = true;
        app.workspace_panel_open_grace_until = Some(Instant::now() + WORKSPACE_PANEL_OPEN_GRACE);
        app.restore_workspace_if_panel_open();
        assert!(!app.workspace_new_session_open);
        assert!(app.workspace_panel_open_grace_until.is_none());
    }

    #[test]
    fn optimistic_panel_open_survives_empty_probe_during_grace() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.workspace_skills_open = true;
        app.mark_workspace_panel_opening();
        assert!(app.workspace_panel_open());
        // Force a probe immediately (empty without a real panel).
        app.last_workspace_panel_probe = Instant::now()
            .checked_sub(WORKSPACE_PANEL_PROBE_INTERVAL)
            .unwrap_or_else(Instant::now);
        app.sync_workspace_panel_state(true);
        // Grace keeps optimistic selection so the toolbar stays highlighted.
        assert!(
            app.workspace_skills_open,
            "empty probe must not clear skills open during spawn grace"
        );
        assert!(app.workspace_panel_open());
    }

    #[test]
    fn empty_probe_clears_optimistic_flags_after_grace() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.workspace_skills_open = true;
        app.workspace_panel_open_grace_until = Some(
            Instant::now()
                .checked_sub(Duration::from_millis(1))
                .unwrap_or_else(Instant::now),
        );
        app.last_workspace_panel_probe = Instant::now()
            .checked_sub(WORKSPACE_PANEL_PROBE_INTERVAL)
            .unwrap_or_else(Instant::now);
        app.sync_workspace_panel_state(true);
        assert!(
            !app.workspace_skills_open,
            "expired grace should allow empty probe to clear flags"
        );
    }

    #[test]
    fn pointer_in_sidebar_pane_stays_true_while_stationary() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.last_mouse = Some(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::empty(),
        });
        app.last_mouse_activity =
            Instant::now() - SIDEBAR_POINTER_CURSOR_HOLD - Duration::from_millis(1);
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        assert!(app.pointer_in_sidebar_pane(Some(&metrics)));
    }

    fn group_header_mouse_row(app: &App, metrics: &ui::LayoutMetrics, label: &str) -> u16 {
        let group_row = app
            .rows
            .iter()
            .position(|row| matches!(row, RowKind::Group { label: cwd, .. } if cwd == label))
            .unwrap();
        metrics.list_top_y + group_row.saturating_sub(app.scroll) as u16
    }

    #[test]
    fn group_drag_start_clears_stale_row_hover_states() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session_in_group("tmux:win:1", 1, "one", "~/a", false),
            sample_session_in_group("tmux:win:2", 2, "two", "~/b", false),
        ];
        app.group_order = vec!["~/a".into(), "~/b".into()];
        app.folded_groups.clear();
        app.rebuild_rows();
        app.group_hover_row = Some(0);
        app.hover_row = Some(2);
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let row_y = group_header_mouse_row(&app, &metrics, "~/a");
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: row_y,
            modifiers: KeyModifiers::empty(),
        };
        app.handle_mouse(&down, &metrics);
        assert!(app.group_drag.pending());
        assert!(!app.group_drag.active());
        assert_eq!(app.group_hover_row, None);
        assert_eq!(app.hover_row, None);
    }

    #[test]
    fn group_header_click_folds_without_drag() {
        let dir = TempDir::new().unwrap();
        let config = isolated_config(&dir);
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session_in_group("tmux:win:1", 1, "one", "~/a", false),
            sample_session_in_group("tmux:win:2", 2, "two", "~/b", false),
        ];
        app.group_order = vec!["~/a".into(), "~/b".into()];
        app.folded_groups.clear();
        app.rebuild_rows();
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let row_y = group_header_mouse_row(&app, &metrics, "~/a");
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: row_y,
            modifiers: KeyModifiers::empty(),
        };
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 10,
            row: row_y,
            modifiers: KeyModifiers::empty(),
        };
        app.handle_mouse(&down, &metrics);
        app.handle_mouse(&up, &metrics);
        assert!(app.folded_groups.contains("~/a"));
    }

    #[test]
    fn group_header_drag_release_does_not_fold() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session_in_group("tmux:win:1", 1, "one", "~/a", false),
            sample_session_in_group("tmux:win:2", 2, "two", "~/b", false),
        ];
        app.group_order = vec!["~/a".into(), "~/b".into()];
        app.folded_groups.clear();
        app.rebuild_rows();
        let first = app.session_row_index("tmux:win:1").unwrap();
        app.set_selected(first);
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let source_y = group_header_mouse_row(&app, &metrics, "~/a");
        // Must leave the source PWD section — motion within the same group stays a click.
        let target_y = group_header_mouse_row(&app, &metrics, "~/b");
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: source_y,
            modifiers: KeyModifiers::empty(),
        };
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 10,
            row: target_y,
            modifiers: KeyModifiers::empty(),
        };
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 10,
            row: target_y,
            modifiers: KeyModifiers::empty(),
        };
        app.handle_mouse(&down, &metrics);
        app.handle_mouse(&drag, &metrics);
        app.handle_mouse(&up, &metrics);
        assert!(!app.folded_groups.contains("~/a"));
        assert_eq!(
            app.session_at(app.selected)
                .map(|session| session.id.as_str()),
            Some("tmux:win:1")
        );
    }

    #[test]
    fn group_header_same_section_drag_noise_still_folds() {
        // xterm.js often emits same-row or within-section Drag samples on a
        // simple click. That must not lift the ⠿ grip or swallow fold.
        let dir = TempDir::new().unwrap();
        let config = isolated_config(&dir);
        let mut app = App::new(&config).unwrap();
        app.host = crate::bar::host_terminal::detect_from_env(Some("vscode"), false);
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session_in_group("tmux:win:1", 1, "one", "~/a", false),
            sample_session_in_group("tmux:win:2", 2, "two", "~/b", false),
        ];
        app.group_order = vec!["~/a".into(), "~/b".into()];
        app.folded_groups.clear();
        app.rebuild_rows();
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let row_y = group_header_mouse_row(&app, &metrics, "~/a");

        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: row_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        // Same-row Drag noise (IDE click jitter).
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 10,
                row: row_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        // Motion into the session row of the *same* PWD section.
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 10,
                row: row_y.saturating_add(1),
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(
            app.group_drag.pending() && !app.group_drag.active(),
            "within-section motion must not engage reorder"
        );
        assert!(!app.group_drag.dragged);
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 10,
                row: row_y.saturating_add(1),
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(
            app.folded_groups.contains("~/a"),
            "fold click must succeed despite IDE Drag noise"
        );
        assert!(!app.group_drag.active() && !app.group_drag.pending());
    }

    #[test]
    fn group_drag_grip_only_after_leaving_source() {
        let dir = TempDir::new().unwrap();
        let config = isolated_config(&dir);
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session_in_group("tmux:win:1", 1, "one", "~/a", false),
            sample_session_in_group("tmux:win:2", 2, "two", "~/b", false),
        ];
        app.group_order = vec!["~/a".into(), "~/b".into()];
        app.folded_groups.clear();
        app.rebuild_rows();
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let source_y = group_header_mouse_row(&app, &metrics, "~/a");
        let target_y = group_header_mouse_row(&app, &metrics, "~/b");
        let rows = app.rows.clone();
        let sections = ui::group_sections(&rows);

        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: source_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        // Pending only — no Source grip while still on the press row.
        assert!(app.group_drag.pending());
        assert_eq!(
            ui::group_section_highlight(&sections, &rows, 0, &app.group_drag),
            None,
            "pending press must not show ⠿"
        );

        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 10,
                row: target_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(app.group_drag.active() && app.group_drag.dragged);
        let preview_rows = app.rows.clone();
        let preview_sections = ui::group_sections(&preview_rows);
        // Source section is the dragged PWD wherever preview placed it.
        let source_idx = preview_sections
            .iter()
            .position(|s| s.label == "~/a")
            .expect("source section");
        assert_eq!(
            ui::group_section_highlight(
                &preview_sections,
                &preview_rows,
                preview_sections[source_idx].start,
                &app.group_drag
            ),
            Some(ui::GroupHighlight::Source),
            "⠿ only after leaving the source section"
        );
    }

    #[test]
    fn group_drag_drop_suppresses_hover_on_release_row() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session_in_group("tmux:win:1", 1, "one", "~/a", false),
            sample_session_in_group("tmux:win:2", 2, "two", "~/b", false),
        ];
        app.group_order = vec!["~/a".into(), "~/b".into()];
        app.folded_groups.clear();
        app.rebuild_rows();
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let source_y = group_header_mouse_row(&app, &metrics, "~/a");
        let target_y = group_header_mouse_row(&app, &metrics, "~/b");
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: source_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 10,
                row: target_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 10,
                row: target_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(app.suppress_list_hover_after_group_drag);
        assert_eq!(app.hover_row, None);
        assert_eq!(app.group_hover_row, None);
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Moved,
                column: 10,
                row: target_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(app.suppress_list_hover_after_group_drag);
        assert_eq!(app.hover_row, None);
        assert_eq!(app.group_hover_row, None);
    }

    #[test]
    fn group_drag_moved_finishes_after_lost_mouseup() {
        // Regression (Cursor/VS Code xterm.js): MouseUp is often dropped. Moved
        // used to keep tracking the engaged PWD source so the group stuck to the
        // pointer and would not "let go" when repositioning.
        let dir = TempDir::new().unwrap();
        let config = isolated_config(&dir);
        let mut app = App::new(&config).unwrap();
        app.host = crate::bar::host_terminal::detect_from_env(Some("vscode"), false);
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session_in_group("tmux:win:1", 1, "one", "~/a", false),
            sample_session_in_group("tmux:win:2", 2, "two", "~/b", false),
        ];
        app.group_order = vec!["~/a".into(), "~/b".into()];
        app.folded_groups.clear();
        app.rebuild_rows();
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let source_y = group_header_mouse_row(&app, &metrics, "~/a");
        let target_y = group_header_mouse_row(&app, &metrics, "~/b");

        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: source_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 10,
                row: target_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(app.group_drag.active(), "drag over target should engage");
        assert_eq!(app.group_drag.source.as_deref(), Some("~/a"));

        // Lost MouseUp → bare Moved (button not held).
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Moved,
                column: 10,
                row: target_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(
            !app.group_drag.active(),
            "Moved must release the PWD drag (button is not held)"
        );
        assert!(
            !app.group_drag.pending(),
            "Moved must not leave a pending press after drop"
        );
        assert_eq!(
            app.group_order,
            vec!["~/b".to_string(), "~/a".to_string()],
            "drop on target via Moved should still commit the reorder"
        );
    }

    #[test]
    fn group_drag_pending_moved_finishes_as_click_not_sticky_lift() {
        // Lost MouseUp after Down (no Drag): Moved must fold/click, not engage
        // hold-drag and stick the source highlight to subsequent hover.
        let dir = TempDir::new().unwrap();
        let config = isolated_config(&dir);
        let mut app = App::new(&config).unwrap();
        app.host = crate::bar::host_terminal::detect_from_env(Some("cursor"), false);
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session_in_group("tmux:win:1", 1, "one", "~/a", false),
            sample_session_in_group("tmux:win:2", 2, "two", "~/b", false),
        ];
        app.group_order = vec!["~/a".into(), "~/b".into()];
        app.folded_groups.clear();
        app.rebuild_rows();
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let row_y = group_header_mouse_row(&app, &metrics, "~/a");

        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: row_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(app.group_drag.pending());

        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Moved,
                column: 10,
                row: row_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(
            !app.group_drag.pending() && !app.group_drag.active(),
            "Moved after pending press must clear drag state, not lift-on-hover"
        );
        assert!(
            app.folded_groups.contains("~/a"),
            "lost-Up click should still fold the group"
        );
    }

    #[test]
    fn group_drag_reorders_when_saved_order_is_partial() {
        // Real bug: sidebar-group-order.json often only has a few (or stale test)
        // labels; live PWDs are appended only for display via ensure_labels and
        // never written into group_order. reorder() then no-ops on drop.
        let dir = TempDir::new().unwrap();
        let config = isolated_config(&dir);
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session_in_group("tmux:win:1", 1, "one", "~/projects/alpha", false),
            sample_session_in_group("tmux:win:2", 2, "two", "~/projects/beta", false),
            sample_session_in_group("tmux:win:3", 3, "three", "~/projects/gamma", false),
        ];
        // Partial / polluted saved order — none of the live labels.
        app.group_order = vec!["~/b".into(), "~/a".into()];
        app.folded_groups.clear();
        app.rebuild_rows();

        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 30), &app.rows);
        let source_y = group_header_mouse_row(&app, &metrics, "~/projects/alpha");
        let target_y = group_header_mouse_row(&app, &metrics, "~/projects/gamma");

        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: source_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 10,
                row: target_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        // Preview must already place alpha after gamma while dragging.
        let preview = app.effective_group_order();
        let alpha = preview.iter().position(|l| l == "~/projects/alpha");
        let gamma = preview.iter().position(|l| l == "~/projects/gamma");
        assert!(
            alpha.is_some() && gamma.is_some() && alpha > gamma,
            "drag preview should reorder live labels even when saved order is partial; got {preview:?}"
        );

        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 10,
                row: target_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );

        let alpha = app.group_order.iter().position(|l| l == "~/projects/alpha");
        let gamma = app.group_order.iter().position(|l| l == "~/projects/gamma");
        assert!(
            alpha.is_some() && gamma.is_some() && alpha > gamma,
            "drop should persist reorder; got {:?}",
            app.group_order
        );
        let saved = group_order::load(&config);
        assert_eq!(saved.groups, app.group_order);
    }

    fn session_mouse_row(app: &App, metrics: &ui::LayoutMetrics, session_id: &str) -> u16 {
        let row = app
            .rows
            .iter()
            .position(|r| matches!(r, RowKind::Session { session } if session.id == session_id))
            .expect("session row");
        metrics.list_top_y + row.saturating_sub(app.scroll) as u16
    }

    #[test]
    fn ide_host_activates_session_on_mouse_down() {
        // Cursor/VS Code: MouseUp is unreliable; session switch must fire on Down.
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.host = crate::bar::host_terminal::detect_from_env(Some("vscode"), false);
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let row_y = session_mouse_row(&app, &metrics, "tmux:win:2");
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: row_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert_eq!(
            app.session_at(app.selected).map(|s| s.id.as_str()),
            Some("tmux:win:2"),
            "IDE host must select the clicked session on MouseDown"
        );
    }

    #[test]
    fn full_host_does_not_activate_session_on_mouse_down_alone() {
        // Ghostty keeps Down for text-select; activate remains on Up.
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.host = crate::bar::host_terminal::detect_from_env(Some("ghostty"), false);
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();
        let first = app.session_row_index("tmux:win:1").unwrap();
        app.set_selected(first);
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let row_y = session_mouse_row(&app, &metrics, "tmux:win:2");
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: row_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert_eq!(
            app.session_at(app.selected).map(|s| s.id.as_str()),
            Some("tmux:win:1"),
            "full host should not switch selection on Down alone"
        );
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 10,
                row: row_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert_eq!(
            app.session_at(app.selected).map(|s| s.id.as_str()),
            Some("tmux:win:2"),
            "full host still activates on MouseUp"
        );
    }

    #[test]
    fn ide_edge_resize_does_not_start_on_session_row() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.host = crate::bar::host_terminal::detect_from_env(Some("vscode"), false);
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(53, 20), &app.rows);
        let row_y = session_mouse_row(&app, &metrics, "tmux:win:2");
        // Rightmost columns are the edge grip, but over a session row must activate.
        let grip_col = metrics.frame_width.saturating_sub(1);
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: grip_col,
                row: row_y,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(
            !app.edge_resize_active,
            "edge resize must not start on a session row"
        );
        assert_eq!(
            app.session_at(app.selected).map(|s| s.id.as_str()),
            Some("tmux:win:2")
        );
    }

    #[test]
    fn edge_resize_moved_finishes_does_not_resize() {
        // Regression: Moved used to call update_edge_resize, so after a lost
        // MouseUp (xterm.js) plain hover kept sliding the sidebar.
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.host = crate::bar::host_terminal::detect_from_env(Some("vscode"), false);
        app.user_pane_width = Some(40);
        app.last_applied_sidebar_width = Some(40);
        app.preferred_pane_width = 40;
        app.begin_edge_resize(38);
        assert!(app.edge_resize_active);
        assert_eq!(app.edge_resize_grab_offset, 2);
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Moved,
                column: 20,
                row: 5,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(
            !app.edge_resize_active,
            "Moved must end edge-resize (button is not held)"
        );
        assert_eq!(app.edge_resize_grab_offset, 0);
        // Hover must not have treated column 20 as a live drag target (20+2=22).
        assert_ne!(
            app.preferred_pane_width, 22,
            "Moved must not apply edge-resize width from a hover sample"
        );
    }

    #[test]
    fn edge_resize_drag_tracks_grab_offset() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.host = crate::bar::host_terminal::detect_from_env(Some("vscode"), false);
        app.user_pane_width = Some(40);
        app.last_applied_sidebar_width = Some(40);
        app.preferred_pane_width = 40;
        app.last_client_width = Some(120);
        app.begin_edge_resize(38); // grab offset 2
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        app.handle_mouse(
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 30,
                row: 5,
                modifiers: KeyModifiers::empty(),
            },
            &metrics,
        );
        assert!(app.edge_resize_active);
        // desired = 30 + 2 = 32 (may only update preferred if throttle/tmux skip)
        assert_eq!(app.preferred_pane_width, 32);
        assert_eq!(app.user_pane_width, Some(32));
    }
}
