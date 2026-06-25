use super::*;
use super::{
    AcknowledgedCompletion, App, CLIPBOARD_NOTICE_DURATION, CLOSE_HOLD_MIN_SETTLE,
    CLOSE_HOLD_MISSED_REPEAT_TOLERANCE, CLOSE_HOLD_RELEASE_SLACK, CLOSE_HOLD_REPEAT_LEARN_MIN,
    CLOSE_HOLD_SILENCE_MIN, DIGIT_JUMP_TIMEOUT, DRAG_HOLD_MIN, NOTEPAD_DOUBLE_CLICK_TIMEOUT,
    NOTEPAD_SAVE_DEBOUNCE, POINTER_EXIT_HOVER_CLEAR, SIDEBAR_ENGAGE_THROTTLE,
    SIDEBAR_FOCUS_PROBE_INTERVAL, SIDEBAR_POINTER_CURSOR_HOLD, TELEMETRY_FLUSH_INTERVAL,
    WORKSPACE_PANEL_PROBE_INTERVAL, AGENTS_WINDOW_PROBE_INTERVAL,
    is_fresh_unacknowledged_completion, load_update_banner, parse_digit_ordinal,
};
use crate::bar::client::ClientEvent;
use crate::bar::editor::{self, TextEditor};
use crate::bar::group_order::{self, SidebarGroupOrder};
use crate::bar::keys::{has_command_modifier, has_paste_modifier};
use crate::bar::mouse_cursor::{self, MouseCursorShape};
use crate::bar::notepad::{self, Note, SidebarNotepad};
use crate::bar::ui::{self, GroupDragState, NotepadHit, RowKind, ToolbarAction};
use crate::config::Config;
use crate::model::{AgentState, ServerEvent, Session};
use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::{HashMap, HashSet};
use std::io;
use std::time::{Duration, Instant};

impl App {
    pub(crate) fn reload_update_banner(&mut self) {
        self.update_banner = load_update_banner();
    }
    pub(crate) fn maybe_flush_telemetry(&mut self) {
        if self.last_telemetry_flush.elapsed() < TELEMETRY_FLUSH_INTERVAL {
            return;
        }
        self.last_telemetry_flush = Instant::now();
        let wrote = crate::telemetry::counters::save_pending_to_file(&self.config.home)
            .unwrap_or(false);
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
    pub(crate) fn begin_list_text_selection(&mut self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) {
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
    pub(crate) fn update_list_text_selection(&mut self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) {
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
        if head != anchor {
            self.list_text_selecting = true;
        }
        self.list_select_head = Some(head);
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
    pub(crate) fn drag_hold_poll_cap(&self) -> Option<Duration> {
        let note = self
            .note_drag
            .pending()
            .then(|| {
                self.note_drag
                    .pressed_at
                    .and_then(|pressed| DRAG_HOLD_MIN.checked_sub(pressed.elapsed()))
            })
            .flatten()
            .filter(|remaining| !remaining.is_zero());
        let group = self
            .group_drag
            .pending()
            .then(|| {
                self.group_drag
                    .pressed_at
                    .and_then(|pressed| DRAG_HOLD_MIN.checked_sub(pressed.elapsed()))
            })
            .flatten()
            .filter(|remaining| !remaining.is_zero());
        match (note, group) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
    fn sidebar_drag_should_engage(&self, mouse: &MouseEvent, pressed_at: Option<Instant>, pressed_row: Option<u16>) -> bool {
        pressed_at.is_some_and(|pressed| pressed.elapsed() >= DRAG_HOLD_MIN)
            || pressed_row != Some(mouse.row)
    }
    pub(crate) fn maybe_engage_sidebar_drag(&mut self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) {
        if self.note_drag.pending()
            && self.sidebar_drag_should_engage(
                mouse,
                self.note_drag.pressed_at,
                self.note_drag.pressed_row,
            )
        {
            self.engage_note_drag();
            self.update_note_drag_hover(mouse, metrics);
        } else if self.group_drag.pending()
            && self.sidebar_drag_should_engage(
                mouse,
                self.group_drag.pressed_at,
                self.group_drag.pressed_row,
            )
        {
            self.engage_group_drag();
            self.update_group_drag_hover(mouse, metrics);
        }
    }
    pub(crate) fn maybe_engage_pending_drag_from_hold(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        let note_hold = self.note_drag.pending()
            && self
                .note_drag
                .pressed_at
                .is_some_and(|pressed| pressed.elapsed() >= DRAG_HOLD_MIN);
        let group_hold = self.group_drag.pending()
            && self
                .group_drag
                .pressed_at
                .is_some_and(|pressed| pressed.elapsed() >= DRAG_HOLD_MIN);
        if !note_hold && !group_hold {
            return Ok(());
        }
        let Some(mouse) = self.last_mouse else {
            return Ok(());
        };
        let size = terminal.size()?;
        let metrics = self.layout_metrics(size);
        if note_hold {
            self.engage_note_drag();
            self.update_note_drag_hover(&mouse, &metrics);
        }
        if group_hold {
            self.engage_group_drag();
            self.update_group_drag_hover(&mouse, &metrics);
        }
        self.redraw_if_needed(terminal)
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
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                self.clear_close_mode();
                if let Some(row_idx) = self.close_session_under_mouse(mouse, metrics) {
                    if let Some(session_id) =
                        self.session_at(row_idx).map(|session| session.id.clone())
                    {
                        self.rename = None;
                        self.context_menu = Some(ui::ContextMenu {
                            target: ui::ContextMenuTarget::Session { session_id },
                            x: mouse.column,
                            y: mouse.row,
                        });
                        self.force_redraw();
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
                        self.context_menu = Some(ui::ContextMenu {
                            target: ui::ContextMenuTarget::Group {
                                cwd_label: cwd_label.to_string(),
                            },
                            x: mouse.column,
                            y: mouse.row,
                        });
                        self.force_redraw();
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
                        self.context_menu = Some(ui::ContextMenu {
                            target: ui::ContextMenuTarget::Note { note_id },
                            x: mouse.column,
                            y: mouse.row,
                        });
                        self.force_redraw();
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
                            (ui::ContextMenuTarget::Note { note_id }, ui::ContextMenuAction::Rename) => {
                                if let Some(row_idx) = self.note_title_row_index(&note_id) {
                                    self.start_rename_for_note(&note_id, row_idx);
                                }
                            }
                            (ui::ContextMenuTarget::Note { note_id }, ui::ContextMenuAction::Delete) => {
                                self.request_delete_note_confirm_by_id(&note_id);
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
                if let Some(cwd_label) = ui::group_add_click(
                    mouse.column,
                    mouse.row,
                    metrics,
                    self.scroll,
                    self.rows.len(),
                    &self.rows,
                )
                .map(str::to_string)
                {
                    self.create_console_in_group(&cwd_label);
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
                let line_width = metrics.list_line_width;
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
                        self.handle_notepad_scrollbar_click(
                            mouse,
                            metrics,
                            trail_base,
                            note_index,
                        );
                        return;
                    }
                    Some(NotepadHit::NoteBody { note_index }) => {
                        self.clear_close_mode();
                        self.activate_note(note_index);
                        self.handle_notepad_body_click(
                            mouse,
                            metrics,
                            trail_base,
                            note_index,
                        );
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
                    if ui::is_group_trailing_click(mouse.column, metrics) {
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
                } else if ui::pointer_in_list_body(mouse.column, metrics) {
                    self.begin_list_text_selection(mouse, metrics);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.maybe_clear_list_hover_suppress(mouse.row);
                self.maybe_engage_sidebar_drag(mouse, metrics);
                if self.notepad_editor.scrollbar_thumb_offset.is_some() {
                    self.update_notepad_scrollbar_drag(mouse, metrics, self.sidebar_trail_base());
                } else if self.notepad_editor.drag_selecting {
                    self.update_notepad_drag_selection(mouse, metrics);
                } else if self.note_drag.active() {
                    self.note_drag.dragged = true;
                    self.update_note_drag_hover(mouse, metrics);
                } else if self.list_select_anchor.is_some() && !self.group_drag.active() {
                    self.update_list_text_selection(mouse, metrics);
                } else if self.group_drag.active() {
                    self.group_drag.dragged = true;
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
                self.maybe_clear_list_hover_suppress(mouse.row);
                self.maybe_engage_sidebar_drag(mouse, metrics);
                if self.note_drag.active() {
                    self.update_note_drag_hover(mouse, metrics);
                } else if self.group_drag.active() {
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
            MouseEventKind::Up(MouseButton::Left) => {
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
                    self.restore_workspace_if_panel_open();
                }
                if let Some(from) = self.group_drag.source.take() {
                    let hover = self.group_drag.hover.take();
                    let dragged = self.group_drag.dragged;
                    match hover.as_deref() {
                        Some(to) if to != from.as_str() => {
                            group_order::reorder(&mut self.group_order, &from, to);
                            let _ = group_order::save(
                                &self.config,
                                &SidebarGroupOrder {
                                    groups: self.group_order.clone(),
                                },
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
    pub(crate) fn pointer_over_sidebar_target(&self, mouse: &MouseEvent, metrics: &ui::LayoutMetrics) -> bool {
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
        self.workspace_settings_open || self.workspace_new_session_open
    }
    pub(crate) fn restore_workspace_if_panel_open(&mut self) {
        let (settings_open, new_session_open) = crate::daemon::tmux::workspace_pane_panel_state(
            &self.config.tmux_ui_session,
        )
        .unwrap_or((false, false));
        if !settings_open && !new_session_open {
            self.workspace_settings_open = false;
            self.workspace_new_session_open = false;
            return;
        }
        if crate::daemon::tmux::restore_workspace_attach(
            &self.config.tmux_ui_session,
            &self.config.tmux_session,
        )
        .is_ok()
        {
            self.workspace_settings_open = false;
            self.workspace_new_session_open = false;
            self.force_redraw();
        }
    }
    pub(crate) fn run_toolbar_action(&mut self, action: ToolbarAction) {
        self.unfocus_notepad();
        if action != ToolbarAction::Settings
            && action != ToolbarAction::NewSession
            && action != ToolbarAction::Leave
        {
            self.restore_workspace_if_panel_open();
        }
        match action {
            ToolbarAction::NewSession => self.open_new_session_panel(),
            ToolbarAction::Search
            | ToolbarAction::Automations
            | ToolbarAction::Mcps
            | ToolbarAction::Skills => {
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
        if mouse.column < pane_width {
            self.ensure_sidebar_pane_engaged();
        }
    }
    pub(crate) fn ensure_sidebar_pane_engaged(&mut self) {
        if self.sidebar_pane_focused && !self.workspace_pane_has_focus() {
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
        if !self.pointer_in_sidebar_pane(metrics) {
            return None;
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
            Some(ui::NotepadHit::NoteBody { .. } | ui::NotepadHit::NoteBodyScrollbar { .. }) => {
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
        if self.last_sidebar_focus_probe.elapsed() < SIDEBAR_FOCUS_PROBE_INTERVAL {
            return;
        }
        self.last_sidebar_focus_probe = Instant::now();

        let prev = self.sidebar_pane_focused;
        let workspace_focused = self.workspace_pane_has_focus();
        if self.last_workspace_pane_focused && !workspace_focused {
            self.rows_version = self.rows_version.wrapping_add(1);
        }
        self.last_workspace_pane_focused = workspace_focused;
        if workspace_focused {
            self.sidebar_pane_focused = false;
            if self.notepad_focused {
                self.unfocus_notepad();
            }
            return;
        }

        let Some(active) = crate::daemon::tmux::current_pane_is_active() else {
            return;
        };
        self.sidebar_pane_focused = active;
        if prev && !active {
            if self.notepad_focused {
                self.unfocus_notepad();
            }
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
        }
        if changed {
            self.pointer_near_exit = false;
            self.last_synced_mouse_cursor = None;
            self.sync_sidebar_mouse_cursor(None);
            self.force_redraw();
        }
    }
    pub(crate) fn sync_workspace_panel_state(&mut self, force: bool) {
        let panels_maybe_open =
            self.workspace_settings_open || self.workspace_new_session_open;
        if !force && !panels_maybe_open {
            return;
        }
        if !force
            && self.last_workspace_panel_probe.elapsed() < WORKSPACE_PANEL_PROBE_INTERVAL
        {
            return;
        }
        self.last_workspace_panel_probe = Instant::now();
        let (settings_open, new_session_open) = crate::daemon::tmux::workspace_pane_panel_state(
            &self.config.tmux_ui_session,
        )
        .unwrap_or((false, false));
        if settings_open != self.workspace_settings_open
            || new_session_open != self.workspace_new_session_open
        {
            self.workspace_settings_open = settings_open;
            self.workspace_new_session_open = new_session_open;
            self.force_redraw();
        }
    }
    pub(crate) fn open_new_session_panel(&mut self) {
        self.workspace_new_session_open = true;
        self.spawn_workspace_panel(crate::daemon::tmux::spawn_open_workspace_new_session);
    }
    pub(crate) fn open_sessions_settings(&mut self) {
        if crate::daemon::tmux::workspace_pane_running_settings(&self.config.tmux_ui_session)
            .unwrap_or(false)
        {
            self.restore_workspace_if_panel_open();
            return;
        }
        self.workspace_settings_open = true;
        self.spawn_workspace_panel(crate::daemon::tmux::spawn_toggle_workspace_settings);
    }
    pub(crate) fn spawn_workspace_panel(&mut self, spawn: fn() -> anyhow::Result<()>) {
        match spawn() {
            Ok(()) => {
                self.last_workspace_panel_probe =
                    Instant::now() - WORKSPACE_PANEL_PROBE_INTERVAL;
                self.sync_workspace_panel_state(true);
            }
            Err(error) => {
                self.workspace_settings_open = false;
                self.workspace_new_session_open = false;
                let _ = crate::daemon::tmux::run_tmux(&[
                    "display-message",
                    &format!("sessions panel failed: {error}"),
                ]);
            }
        }
    }
    pub(crate) fn toggle_workspace_panel(&mut self, toggle: fn(&str, &str) -> anyhow::Result<bool>) {
        match toggle(&self.config.tmux_ui_session, &self.config.tmux_session) {
            Ok(_) => {
                self.last_workspace_panel_probe = Instant::now() - WORKSPACE_PANEL_PROBE_INTERVAL;
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
        let next_note_hover = if self.note_drag.active() || self.list_hover_updates_suppressed(mouse.row)
        {
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
        let next_sessions_title_add = ui::sessions_title_add_hover_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
        );
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
        }
    }
    pub(crate) fn dismiss_context_menu(&mut self) {
        if self.context_menu.is_some() {
            self.context_menu = None;
            self.force_redraw();
        }
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
            KeyCode::Enter => self.commit_rename(),
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
        let Some(rename) = self.rename.take() else {
            return;
        };
        self.last_synced_mouse_cursor = None;
        self.sync_sidebar_mouse_cursor(None);
        let buffer = rename.buffer.trim().to_string();
        if buffer.is_empty() {
            self.force_redraw();
            return;
        }
        match rename.target {
            ui::RenameTarget::Session { session_id } => {
                if let Ok(Some(patch)) = self.client.rename(&session_id, buffer) {
                    self.apply_event(ClientEvent::Patch(patch));
                }
            }
            ui::RenameTarget::Note { note_id } => {
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
    use crate::bar::app::{App, POINTER_EXIT_HOVER_CLEAR, SIDEBAR_FOCUS_PROBE_INTERVAL, SIDEBAR_POINTER_CURSOR_HOLD};
    use crate::bar::app::test_fixtures::{sample_session, sample_session_in_group};
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
        fn sync_sidebar_pane_focus_does_not_clear_pointer_hover() {
            let config = Config::default();
            let mut app = App::new(&config).unwrap();
            app.hover_row = Some(3);
            app.group_hover_row = Some(1);
            app.last_sidebar_focus_probe =
                Instant::now() - SIDEBAR_FOCUS_PROBE_INTERVAL - Duration::from_millis(1);
            app.sync_sidebar_pane_focus();
            assert_eq!(app.hover_row, Some(3));
            assert_eq!(app.group_hover_row, Some(1));
        }

    #[test]
        fn restore_workspace_clears_stale_cached_flags_without_tmux_dismiss() {
            let config = Config::default();
            let mut app = App::new(&config).unwrap();
            app.workspace_new_session_open = true;
            app.restore_workspace_if_panel_open();
            assert!(!app.workspace_new_session_open);
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
        let row_y = group_header_mouse_row(&app, &metrics, "~/a");
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: row_y,
            modifiers: KeyModifiers::empty(),
        };
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 10,
            row: row_y.saturating_add(1),
            modifiers: KeyModifiers::empty(),
        };
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 10,
            row: row_y.saturating_add(1),
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

}
