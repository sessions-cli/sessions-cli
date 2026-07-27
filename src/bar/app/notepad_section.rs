use super::{App, NOTEPAD_DOUBLE_CLICK_TIMEOUT, NOTEPAD_SAVE_DEBOUNCE};
use crate::bar::editor::{self};
use crate::bar::group_order::{self};
use crate::bar::keys::has_paste_modifier;
use crate::bar::notepad::{self, Note, SidebarNotepad};
use crate::bar::ui::{self};
use anyhow::Result;
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseEvent};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::{Duration, Instant};

impl App {
    pub(crate) fn display_notes(&self) -> &[Note] {
        if self.note_drag.active() && !self.notes_preview.is_empty() {
            &self.notes_preview
        } else {
            &self.notes
        }
    }
    pub(crate) fn active_note_index_in(&self, notes: &[Note]) -> Option<usize> {
        self.active_note_id
            .as_ref()
            .and_then(|id| notes.iter().position(|note| &note.id == id))
    }
    pub(crate) fn active_note_index(&self) -> Option<usize> {
        self.active_note_index_in(self.display_notes())
    }
    pub(crate) fn effective_note_hover(&self) -> Option<usize> {
        if self.note_drag.active() {
            return None;
        }
        if let Some(note_id) = self.note_drag.pending_click_note_id.as_deref() {
            return self
                .display_notes()
                .iter()
                .position(|note| note.id == note_id);
        }
        self.notepad_note_hover
    }
    pub(crate) fn active_note_text(&self) -> &str {
        self.active_note_id
            .as_ref()
            .and_then(|id| self.notes.iter().find(|note| &note.id == id))
            .map(|note| note.text.as_str())
            .unwrap_or("")
    }
    pub(crate) fn notepad_list_state(&self) -> ui::NotepadListState<'_> {
        ui::notepad_list_state(
            self.display_notes(),
            self.notepad_expanded,
            self.notes_list_expanded,
            self.active_note_index(),
        )
    }
    pub(crate) fn stable_notepad_list_state(&self) -> ui::NotepadListState<'_> {
        ui::notepad_list_state(
            &self.notes,
            self.notepad_expanded,
            self.notes_list_expanded,
            self.active_note_index_in(&self.notes),
        )
    }
    pub(crate) fn refresh_notes_preview(&mut self) {
        self.notes_preview.clear();
        let (Some(from), Some(to)) = (&self.note_drag.source, &self.note_drag.hover) else {
            return;
        };
        if from == to {
            return;
        }
        let order: Vec<String> = self.notes.iter().map(|note| note.id.clone()).collect();
        let preview = group_order::preview_order(&order, from, to);
        self.notes_preview = preview
            .iter()
            .filter_map(|id| self.notes.iter().find(|note| &note.id == id).cloned())
            .collect();
    }
    pub(crate) fn reorder_notes(&mut self, from_id: &str, to_id: &str) {
        let order: Vec<String> = self.notes.iter().map(|note| note.id.clone()).collect();
        let mut next_order = order;
        group_order::reorder(&mut next_order, from_id, to_id);
        let mut reordered = Vec::with_capacity(self.notes.len());
        for id in &next_order {
            if let Some(note) = self.notes.iter().find(|note| &note.id == id) {
                reordered.push(note.clone());
            }
        }
        if reordered.len() == self.notes.len() {
            self.notes = reordered;
            self.persist_notepad_now();
            self.rows_version = self.rows_version.wrapping_add(1);
        }
    }
    pub(crate) fn begin_note_drag(&mut self, note_index: usize, mouse_row: u16) {
        let Some(note_id) = self.notes.get(note_index).map(|note| note.id.clone()) else {
            return;
        };
        if !self.notepad_expanded {
            self.notepad_expanded = true;
        }
        self.notepad_note_hover = Some(note_index);
        self.note_drag = ui::NoteDragState {
            source: None,
            hover: None,
            dragged: false,
            preserved_active_note_id: self.active_note_id.clone(),
            pending_click_note_id: Some(note_id),
            pressed_at: Some(Instant::now()),
            pressed_row: Some(mouse_row),
        };
        self.force_redraw();
    }
    pub(crate) fn engage_note_drag(&mut self) {
        let Some(note_id) = self.note_drag.pending_click_note_id.take() else {
            return;
        };
        self.notepad_note_hover = None;
        self.note_drag.source = Some(note_id.clone());
        self.note_drag.hover = Some(note_id);
        self.refresh_notes_preview();
        self.rows_version = self.rows_version.wrapping_add(1);
        self.force_redraw();
    }
    pub(crate) fn finish_note_drag_pending_click(&mut self, mouse: &MouseEvent) {
        let pending_id = self.note_drag.pending_click_note_id.take();
        self.note_drag = ui::NoteDragState::default();
        if let Some(note_id) = pending_id {
            if let Some(note_index) = self.notes.iter().position(|note| note.id == note_id) {
                self.handle_notepad_note_title_click(mouse, note_index);
            }
        }
        self.force_redraw();
    }
    pub(crate) fn update_note_drag_hover(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        let source = self.note_drag.source.as_deref();
        let trail_base = self.sidebar_trail_base();
        let stable_state = self.stable_notepad_list_state();
        let rel = mouse.row.saturating_sub(metrics.list_top_y) as usize;
        let new_hover = if rel < metrics.list_height {
            let row_idx = self.scroll.saturating_add(rel);
            (row_idx >= trail_base)
                .then(|| row_idx.saturating_sub(trail_base))
                .and_then(|trail_idx| {
                    source.and_then(|from| ui::note_drag_target(&stable_state, trail_idx, from))
                })
        } else {
            None
        }
        .or_else(|| self.note_drag.hover.clone());

        if new_hover.as_deref() != source {
            self.note_drag.dragged = true;
        }
        if self.note_drag.hover != new_hover {
            self.note_drag.hover = new_hover;
            self.refresh_notes_preview();
            self.rows_version = self.rows_version.wrapping_add(1);
            self.force_redraw();
        }
    }
    pub(crate) fn finish_note_drag(&mut self, mouse: &MouseEvent) {
        let Some(from) = self.note_drag.source.take() else {
            return;
        };
        let hover = self.note_drag.hover.take();
        let dragged = self.note_drag.dragged;
        self.note_drag.pending_click_note_id = None;
        self.note_drag.pressed_at = None;
        self.note_drag.pressed_row = None;
        self.note_drag.preserved_active_note_id = None;
        self.notes_preview.clear();

        match hover.as_deref() {
            Some(to) if to != from.as_str() => {
                self.reorder_notes(&from, to);
            }
            Some(to) if to == from.as_str() && !dragged => {
                if let Some(note_index) = self.notes.iter().position(|note| note.id == from) {
                    self.handle_notepad_note_title_click(mouse, note_index);
                }
            }
            _ => {}
        }

        if dragged {
            self.notepad_note_hover = None;
            self.begin_list_hover_suppress_after_group_drag(mouse.row);
        } else {
            self.notepad_note_hover = None;
        }
        self.rows_version = self.rows_version.wrapping_add(1);
        self.force_redraw();
    }
    pub(crate) fn collapse_other_notes(&mut self, except_index: usize) {
        for (idx, note) in self.notes.iter_mut().enumerate() {
            if idx != except_index {
                note.expanded = false;
            }
        }
    }
    pub(crate) fn ensure_active_note(&mut self) {
        if self.notes.is_empty() {
            self.active_note_id = None;
            return;
        }
        let has_active = self
            .active_note_id
            .as_ref()
            .is_some_and(|id| self.notes.iter().any(|note| note.id == *id));
        if !has_active {
            self.active_note_id = self.notes.first().map(|note| note.id.clone());
        }
    }
    pub(crate) fn set_active_note_id(&mut self, note_id: String) {
        if self.notes.iter().any(|note| note.id == note_id) {
            self.active_note_id = Some(note_id);
        }
    }
    pub(crate) fn notepad_content_width(&self) -> usize {
        let size = self
            .render_cache
            .size
            .map(|(width, height)| ratatui::layout::Size::new(width, height))
            .unwrap_or(ratatui::layout::Size::new(ui::DEFAULT_PANE_WIDTH, 24));
        let metrics = self.layout_metrics(size);
        ui::notepad_content_width(metrics.list_line_width, self.notepad_expanded)
    }
    pub(crate) fn sync_notepad_scroll(&mut self) {
        let content_width = self.notepad_content_width();
        let cursor_line = notepad::display_line_index(
            self.active_note_text(),
            self.notepad_editor.cursor,
            content_width,
        );
        self.notepad_editor.scroll = ui::notepad_scroll_for_cursor(
            self.notepad_editor.scroll,
            cursor_line,
            ui::notepad_text_viewport_rows(self.notepad_expanded),
        );
    }
    pub(crate) fn notepad_snapshot(&self) -> SidebarNotepad {
        SidebarNotepad {
            expanded: self.notepad_expanded,
            notes: self.notes.clone(),
            active_note_id: self.active_note_id.clone(),
            sessions_expanded: self.sessions_expanded,
            notes_list_expanded: self.notes_list_expanded,
            welcome_seeded: self.notepad_welcome_seeded,
        }
    }
    pub(crate) fn notepad_save_pending(&self) -> bool {
        self.notepad_save_deadline.is_some()
    }
    pub(crate) fn notepad_save_badge_label(&self) -> Option<String> {
        ui::notepad_save_status_text(self.notepad_last_saved_at)
    }
    pub(crate) fn persist_notepad_now(&mut self) {
        match notepad::save(&self.config, &self.notepad_snapshot()) {
            Ok(()) => {
                self.notepad_last_saved_at = Some(Utc::now());
                self.rows_version = self.rows_version.wrapping_add(1);
            }
            Err(err) => tracing::warn!("failed to persist notepad: {err}"),
        }
        self.notepad_save_deadline = None;
    }
    pub(crate) fn schedule_notepad_text_save(&mut self) {
        if let Some(note_index) = self.active_note_index() {
            if let Err(err) = notepad::save_note_file(&self.config, &self.notes[note_index]) {
                tracing::warn!("failed to autosave note body: {err}");
            }
        }
        self.notepad_save_deadline = Some(Instant::now() + NOTEPAD_SAVE_DEBOUNCE);
    }
    pub(crate) fn flush_notepad_save_if_due(&mut self) {
        let Some(deadline) = self.notepad_save_deadline else {
            return;
        };
        if Instant::now() >= deadline {
            self.persist_notepad_now();
        }
    }
    pub(crate) fn flush_notepad_save_pending(&mut self) {
        if self.notepad_save_deadline.is_some() {
            self.persist_notepad_now();
        }
    }
    pub(crate) fn notepad_save_poll_cap(&self) -> Option<Duration> {
        self.notepad_save_deadline
            .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
    }
    pub(crate) fn toggle_notes_list_expanded(&mut self) {
        self.notes_list_expanded = !self.notes_list_expanded;
        self.persist_notepad_now();
        self.force_redraw();
    }
    pub(crate) fn toggle_notepad_expanded(&mut self, focus_on_expand: bool) {
        self.notepad_expanded = !self.notepad_expanded;
        if self.notepad_expanded {
            self.ensure_active_note();
            if focus_on_expand {
                if let Some(idx) = self.active_note_index() {
                    self.collapse_other_notes(idx);
                    self.notes[idx].expanded = true;
                }
                self.notepad_focused = true;
            }
            self.sync_notepad_scroll();
            self.notepad_scroll_pending = true;
        } else {
            self.notepad_focused = false;
            self.notepad_editor.scroll = 0;
            self.notepad_editor.selection = None;
            self.notepad_editor.select_anchor = None;
            self.notepad_editor.drag_selecting = false;
            self.notepad_editor.scrollbar_thumb_offset = None;
        }
        self.persist_notepad_now();
        self.force_redraw();
    }
    pub(crate) fn request_delete_note_confirm_by_id(&mut self, note_id: &str) {
        let Some(row_idx) = self.note_title_row_index(note_id) else {
            return;
        };
        self.request_delete_note_confirm_at_row(row_idx);
    }
    pub(crate) fn request_delete_note_confirm_at_row(&mut self, row_idx: usize) {
        let Some(note_index) = self.note_index_for_title_row(row_idx) else {
            return;
        };
        let Some(note) = self.notes.get(note_index) else {
            return;
        };
        let note_id = note.id.clone();
        let title = note.title.clone();
        self.unfocus_notepad();
        self.context_menu = None;
        self.disengage_close_mode();
        self.delete_note_confirm = Some(ui::DeleteNoteConfirmState {
            note_id,
            title,
            buffer: String::new(),
        });
        self.force_redraw();
    }
    pub(crate) fn note_index_for_title_row(&self, row_idx: usize) -> Option<usize> {
        let trail_idx = row_idx.checked_sub(self.sidebar_trail_base())?;
        let _line_width = self
            .render_cache
            .size
            .map(|(width, _)| width as usize)
            .unwrap_or(ui::DEFAULT_PANE_WIDTH as usize);
        match ui::notepad_trail_row_at(trail_idx, &self.notepad_list_state())? {
            ui::NotepadTrailRow::NoteTitle { note_index } => Some(note_index),
            _ => None,
        }
    }
    pub(crate) fn cancel_delete_note_confirm(&mut self) {
        if self.delete_note_confirm.take().is_some() {
            self.disengage_close_mode();
            self.force_redraw();
        }
    }
    pub(crate) fn commit_delete_note_confirm(&mut self) {
        let Some(confirm) = self.delete_note_confirm.as_ref() else {
            return;
        };
        if !ui::delete_note_confirm_ready(confirm) {
            return;
        }
        let note_id = confirm.note_id.clone();
        self.delete_note_confirm = None;
        self.disengage_close_mode();
        self.delete_note_by_id(&note_id);
    }
    pub(crate) fn handle_delete_note_confirm_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => self.cancel_delete_note_confirm(),
            KeyCode::Enter => self.commit_delete_note_confirm(),
            KeyCode::Backspace => {
                if let Some(confirm) = self.delete_note_confirm.as_mut() {
                    ui::delete_note_confirm_apply_backspace(confirm);
                    self.force_redraw();
                }
            }
            KeyCode::Char(c)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::META,
                ) =>
            {
                if let Some(confirm) = self.delete_note_confirm.as_mut() {
                    ui::delete_note_confirm_apply_char(confirm, c);
                    self.force_redraw();
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub(crate) fn delete_note_by_id(&mut self, note_id: &str) {
        let Some(note_index) = self.notes.iter().position(|note| note.id == note_id) else {
            return;
        };
        let was_active = self.active_note_id.as_deref() == Some(note_id);
        if self.rename.as_ref().is_some_and(|rename| {
            matches!(
                &rename.target,
                ui::RenameTarget::Note { note_id: id } if id == note_id
            )
        }) {
            self.rename = None;
        }
        if self
            .delete_note_confirm
            .as_ref()
            .is_some_and(|confirm| confirm.note_id == note_id)
        {
            self.delete_note_confirm = None;
        }
        self.notes.remove(note_index);
        if let Err(err) = notepad::delete_note_file(&self.config, note_id) {
            tracing::warn!("failed to remove note file: {err}");
        }
        self.ensure_active_note();
        if was_active {
            self.notepad_editor.cursor = 0;
            self.notepad_editor.scroll = 0;
            self.notepad_editor.selection = None;
            self.notepad_editor.select_anchor = None;
            self.notepad_editor.drag_selecting = false;
            self.notepad_editor.scrollbar_thumb_offset = None;
            if let Some(idx) = self.active_note_index() {
                self.collapse_other_notes(idx);
                self.notes[idx].expanded = true;
                self.sync_notepad_scroll();
            } else {
                self.notepad_focused = false;
            }
            self.notepad_scroll_pending = true;
        }
        self.persist_notepad_now();
        self.force_redraw();
    }
    pub(crate) fn add_note(&mut self) {
        if !self.notepad_expanded {
            self.notepad_expanded = true;
        }
        let title = notepad::default_note_title(&self.notes);
        let note = Note::new(title, "", true);
        let note_id = note.id.clone();
        let note_index = self.notes.len();
        self.collapse_other_notes(note_index);
        self.notes.push(note);
        self.set_active_note_id(note_id);
        self.notepad_focused = true;
        self.notepad_editor.cursor = 0;
        self.notepad_editor.scroll = 0;
        self.notepad_editor.selection = None;
        self.notepad_editor.select_anchor = None;
        self.notepad_editor.drag_selecting = false;
        self.notepad_editor.scrollbar_thumb_offset = None;
        self.notepad_scroll_pending = true;
        self.persist_notepad_now();
        self.force_redraw();
    }
    pub(crate) fn activate_note(&mut self, note_index: usize) {
        if let Some(note_id) = self.notes.get(note_index).map(|note| note.id.clone()) {
            self.set_active_note_id(note_id);
            if !self.notes[note_index].expanded {
                self.collapse_other_notes(note_index);
                self.notes[note_index].expanded = true;
            }
            self.notepad_scroll_pending = true;
            self.persist_notepad_now();
        }
    }
    pub(crate) fn toggle_note_expanded(&mut self, note_index: usize, focus_on_expand: bool) {
        let Some((note_id, expanded)) = self
            .notes
            .get(note_index)
            .map(|note| (note.id.clone(), !note.expanded))
        else {
            return;
        };
        self.notes[note_index].expanded = expanded;
        if expanded {
            self.collapse_other_notes(note_index);
        }
        self.set_active_note_id(note_id);
        if expanded {
            if focus_on_expand {
                self.notepad_focused = true;
            }
            self.notepad_editor.scroll = 0;
            self.sync_notepad_scroll();
            self.notepad_scroll_pending = true;
        } else if self.active_note_index() == Some(note_index) {
            self.notepad_focused = false;
            self.notepad_editor.selection = None;
            self.notepad_editor.select_anchor = None;
            self.notepad_editor.drag_selecting = false;
            self.notepad_editor.scrollbar_thumb_offset = None;
        }
        self.persist_notepad_now();
        self.force_redraw();
    }
    pub(crate) fn note_title_row_index(&self, note_id: &str) -> Option<usize> {
        let note_index = self.notes.iter().position(|note| note.id == note_id)?;
        ui::notepad_note_title_row_index(
            note_index,
            self.sidebar_trail_base(),
            &self.notepad_list_state(),
        )
    }
    pub(crate) fn focus_notepad(&mut self) {
        self.focus_notepad_at_cursor(None);
    }
    pub(crate) fn focus_notepad_at_cursor(&mut self, cursor: Option<usize>) {
        if !self.notepad_expanded {
            self.notepad_expanded = true;
        }
        self.notepad_focused = true;
        if let Some(cursor) = cursor {
            self.notepad_editor.checkbox_literal_edit = None;
            self.notepad_editor.suppress_terminal_cursor = false;
            self.notepad_editor.cursor = notepad::clamp_cursor(self.active_note_text(), cursor);
        }
        self.sync_notepad_scroll();
        self.notepad_scroll_pending = true;
        self.persist_notepad_now();
        self.sync_sidebar_mouse_cursor(None);
        self.force_redraw();
    }
    pub(crate) fn unfocus_notepad(&mut self) {
        if !self.notepad_focused
            && self.notepad_editor.selection.is_none()
            && !self.notepad_editor.drag_selecting
            && self.notepad_editor.scrollbar_thumb_offset.is_none()
        {
            return;
        }
        self.flush_notepad_save_pending();
        self.notepad_focused = false;
        self.notepad_editor.selection = None;
        self.notepad_editor.select_anchor = None;
        self.notepad_editor.drag_selecting = false;
        self.notepad_editor.scrollbar_thumb_offset = None;
        self.last_synced_mouse_cursor = None;
        self.sync_sidebar_mouse_cursor(None);
        self.force_redraw();
    }
    pub(crate) fn notepad_has_selection(&self) -> bool {
        self.notepad_editor
            .selection
            .is_some_and(|(start, end)| start < end)
    }
    pub(crate) fn copy_notepad_selection(&mut self) -> bool {
        let Some((start, end)) = self
            .notepad_editor
            .selection
            .filter(|(start, end)| start < end)
        else {
            return false;
        };
        let text = notepad::selected_text(self.active_note_text(), start, end);
        self.copy_text_to_clipboard(&text)
    }
    pub(crate) fn cut_notepad_selection(&mut self) -> bool {
        if !self.notepad_has_selection() {
            return false;
        }
        if !self.copy_notepad_selection() {
            return false;
        }
        self.notepad_delete_selection()
    }
    pub(crate) fn select_notepad_all(&mut self) {
        if let Some((start, end)) = notepad::select_all_range(self.active_note_text()) {
            self.notepad_editor.drag_selecting = false;
            self.notepad_editor.select_anchor = None;
            self.notepad_editor.selection = Some((start, end));
            self.notepad_editor.cursor = end;
            self.sync_notepad_scroll();
            self.force_redraw();
        }
    }
    pub(crate) fn paste_to_notepad(&mut self) {
        if let Some(resolved) = self.resolve_paste_text(None) {
            self.apply_resolved_paste(&resolved);
        } else {
            self.show_status_notice("paste failed");
            tracing::warn!("notepad paste had no OS or tmux buffer content");
        }
    }
    pub(crate) fn open_notepad_context_menu(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        self.rename = None;
        self.notepad_editor.drag_selecting = false;
        self.notepad_editor.select_anchor = None;
        let note_index = self.active_note_index().unwrap_or(0);
        if let Some(cursor) = ui::notepad_cursor_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            self.sidebar_trail_base(),
            &self.notepad_list_state(),
            note_index,
            self.active_note_text(),
            self.notepad_editor.scroll,
            self.notepad_editor.checkbox_literal_edit,
        ) {
            let in_selection = self
                .notepad_editor
                .selection
                .is_some_and(|(start, end)| start < end && start <= cursor && cursor < end);
            if in_selection {
                self.focus_notepad();
            } else {
                self.notepad_editor.selection = None;
                self.focus_notepad_at_cursor(Some(cursor));
            }
        } else {
            self.focus_notepad();
        }
        self.open_context_menu(
            ui::ContextMenuTarget::Notepad {
                has_selection: self.notepad_has_selection(),
            },
            mouse,
            metrics,
        );
    }
    pub(crate) fn handle_notepad_context_menu_action(&mut self, action: ui::ContextMenuAction) {
        match action {
            ui::ContextMenuAction::Cut => {
                self.cut_notepad_selection();
            }
            ui::ContextMenuAction::Copy => {
                self.copy_notepad_selection();
            }
            ui::ContextMenuAction::Paste => {
                self.paste_to_notepad();
            }
            ui::ContextMenuAction::SelectAll => {
                self.select_notepad_all();
            }
            _ => {}
        }
        self.force_redraw();
    }
    pub(crate) fn apply_paste_to_notepad(&mut self, raw: &str) {
        let text = crate::clipboard::sanitize_paste_text(raw, true);
        self.insert_notepad_str(&text);
    }
    pub(crate) fn insert_notepad_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.notepad_delete_selection();
        let Some(note_index) = self.active_note_index() else {
            return;
        };
        let remaining = 2000usize.saturating_sub(self.notes[note_index].text.chars().count());
        if remaining == 0 {
            return;
        }
        let insert: String = text.chars().take(remaining).collect();
        let note_text = &self.notes[note_index].text;
        let cursor = notepad::clamp_cursor(note_text, self.notepad_editor.cursor);
        let byte_idx = note_text
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(note_text.len());
        self.notes[note_index].text.insert_str(byte_idx, &insert);
        self.notepad_editor.cursor = cursor + insert.chars().count();
        self.sync_notepad_scroll();
        self.schedule_notepad_text_save();
        self.force_redraw();
    }
    pub(crate) fn start_rename_for_note(&mut self, note_id: &str, row_idx: usize) {
        let Some(title) = self
            .notes
            .iter()
            .find(|note| note.id == note_id)
            .map(|note| note.title.clone())
        else {
            return;
        };
        self.unfocus_notepad();
        self.rename = Some(ui::RenameState {
            target: ui::RenameTarget::Note {
                note_id: note_id.to_string(),
            },
            row_idx,
            buffer: title,
            select_all: true,
        });
        self.context_menu = None;
        self.last_synced_mouse_cursor = None;
        self.sync_sidebar_mouse_cursor(None);
        self.force_redraw();
    }
    pub(crate) fn notepad_register_click(&mut self, column: u16, row: u16) -> u8 {
        let count = if let Some((instant, col, y, clicks)) = self.notepad_last_click {
            if instant.elapsed() <= NOTEPAD_DOUBLE_CLICK_TIMEOUT && col == column && y == row {
                clicks.saturating_add(1)
            } else {
                1
            }
        } else {
            1
        };
        let count = if count > 3 { 1 } else { count };
        self.notepad_last_click = Some((Instant::now(), column, row, count));
        count
    }
    pub(crate) fn notepad_register_title_click(&mut self, row: u16) -> u8 {
        let count = if let Some((instant, _, y, clicks)) = self.notepad_last_click {
            if instant.elapsed() <= NOTEPAD_DOUBLE_CLICK_TIMEOUT && y == row {
                clicks.saturating_add(1)
            } else {
                1
            }
        } else {
            1
        };
        let count = if count > 3 { 1 } else { count };
        self.notepad_last_click = Some((Instant::now(), 0, row, count));
        count
    }
    pub(crate) fn handle_notepad_note_title_click(
        &mut self,
        mouse: &MouseEvent,
        note_index: usize,
    ) {
        let click_count = self.notepad_register_title_click(mouse.row);
        if click_count == 2 {
            if let Some(note_id) = self.notes.get(note_index).map(|note| note.id.clone()) {
                if let Some(row_idx) = self.note_title_row_index(&note_id) {
                    self.start_rename_for_note(&note_id, row_idx);
                }
            }
            return;
        }
        self.toggle_note_expanded(note_index, true);
    }
    pub(crate) fn notepad_scrollbar_context(
        &self,
        metrics: &ui::LayoutMetrics,
        visible_sessions: usize,
        note_index: usize,
    ) -> Option<(ui::NotepadScrollbar, usize)> {
        let terminal_area = ui::terminal_list_area(metrics);
        let line_width = metrics.list_line_width;
        let (_, _, max_scroll) =
            ui::notepad_scroll_metrics(self.active_note_text(), line_width, true);
        let scrollbar = ui::notepad_scrollbar_geometry(
            terminal_area,
            self.scroll,
            metrics.list_height,
            visible_sessions,
            &self.notepad_list_state(),
            note_index,
            self.active_note_text(),
            self.notepad_editor.scroll,
            line_width,
        )?;
        Some((scrollbar, max_scroll))
    }
    pub(crate) fn handle_notepad_scrollbar_click(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
        visible_sessions: usize,
        note_index: usize,
    ) {
        self.activate_note(note_index);
        let Some((scrollbar, max_scroll)) =
            self.notepad_scrollbar_context(metrics, visible_sessions, note_index)
        else {
            return;
        };
        if editor::thumb_hit(&scrollbar, mouse.row) {
            self.notepad_editor.scrollbar_thumb_offset =
                Some(editor::thumb_grab_offset(mouse.row, scrollbar.thumb.y));
            self.focus_notepad();
            self.force_redraw();
            return;
        }
        let Some(target) = editor::scroll_from_track_click(mouse.row, &scrollbar, max_scroll)
        else {
            return;
        };
        if target != self.notepad_editor.scroll {
            self.notepad_editor.scroll = target;
            self.focus_notepad();
            self.force_redraw();
        }
    }
    pub(crate) fn update_notepad_scrollbar_drag(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
        visible_sessions: usize,
    ) {
        let Some(grab_offset) = self.notepad_editor.scrollbar_thumb_offset else {
            return;
        };
        let Some(note_index) = self.active_note_index() else {
            return;
        };
        let Some((scrollbar, max_scroll)) =
            self.notepad_scrollbar_context(metrics, visible_sessions, note_index)
        else {
            return;
        };
        let next = editor::scroll_from_thumb_drag(mouse.row, &scrollbar, max_scroll, grab_offset);
        if next != self.notepad_editor.scroll {
            self.notepad_editor.scroll = next;
            self.force_redraw();
        }
    }
    pub(crate) fn finish_notepad_scrollbar_drag(&mut self) {
        self.notepad_editor.scrollbar_thumb_offset = None;
    }
    pub(crate) fn handle_notepad_body_click(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
        visible_sessions: usize,
        note_index: usize,
    ) {
        let click_count = self.notepad_register_click(mouse.column, mouse.row);
        let cursor = ui::notepad_cursor_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            visible_sessions,
            &self.notepad_list_state(),
            note_index,
            self.active_note_text(),
            self.notepad_editor.scroll,
            self.notepad_editor.checkbox_literal_edit,
        );
        if click_count >= 3 {
            self.notepad_editor.drag_selecting = false;
            self.notepad_editor.select_anchor = None;
            if let Some(cursor) = cursor {
                let (line, _) = notepad::cursor_line_col(self.active_note_text(), cursor);
                let (start, end) = notepad::line_range_at(self.active_note_text(), line);
                self.notepad_editor.selection = Some((start, end));
                self.focus_notepad_at_cursor(Some(end));
                self.copy_notepad_selection();
            }
            return;
        }
        if click_count == 2 {
            self.notepad_editor.drag_selecting = false;
            self.notepad_editor.select_anchor = None;
            if let Some(cursor) = cursor {
                if let Some((start, end)) = notepad::word_range_at(self.active_note_text(), cursor)
                {
                    self.notepad_editor.selection = Some((start, end));
                    self.focus_notepad_at_cursor(Some(end));
                    self.copy_notepad_selection();
                }
            }
            return;
        }

        if let Some(bracket_start) = ui::notepad_checkbox_bracket_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            visible_sessions,
            &self.notepad_list_state(),
            note_index,
            self.active_note_text(),
            self.notepad_editor.scroll,
            self.notepad_editor.selection,
            self.notepad_editor.checkbox_literal_edit,
        ) {
            self.notepad_editor.checkbox_literal_edit = None;
            self.toggle_notepad_checkbox(bracket_start);
            self.notepad_editor.suppress_terminal_cursor = true;
            self.notepad_editor.drag_selecting = false;
            self.notepad_editor.select_anchor = None;
            self.notepad_editor.selection = None;
            self.force_redraw();
            return;
        }

        if let Some(c) = cursor {
            if let Some(url) = notepad::url_at(self.active_note_text(), c) {
                self.open_url(&url);
                self.focus_notepad_at_cursor(Some(c));
                self.notepad_editor.drag_selecting = false;
                return;
            }
        }

        self.notepad_editor.drag_selecting = true;
        self.notepad_editor.select_anchor = cursor;
        self.notepad_editor.selection = None;
        self.focus_notepad_at_cursor(cursor);
    }
    pub(crate) fn update_notepad_drag_selection(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        let Some(note_index) = self.active_note_index() else {
            return;
        };
        let Some(head) = ui::notepad_selection_cursor_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            self.sidebar_trail_base(),
            &self.notepad_list_state(),
            note_index,
            self.active_note_text(),
            self.notepad_editor.scroll,
            self.notepad_editor.checkbox_literal_edit,
        ) else {
            return;
        };
        let anchor = self.notepad_editor.select_anchor.unwrap_or(head);
        let (start, end) = notepad::selection_range(anchor, head);
        let changed = self.notepad_editor.selection != Some((start, end))
            || self.notepad_editor.cursor != head;
        if !changed {
            return;
        }
        self.notepad_editor.selection = Some((start, end));
        self.notepad_editor.cursor = head;
        self.sync_notepad_scroll();
        self.force_redraw();
    }
    pub(crate) fn finish_notepad_drag_selection(&mut self) {
        self.notepad_editor.drag_selecting = false;
        self.notepad_editor.select_anchor = None;
        if self
            .notepad_editor
            .selection
            .is_some_and(|(start, end)| start == end)
        {
            self.notepad_editor.selection = None;
        } else {
            self.copy_notepad_selection();
        }
        self.force_redraw();
    }
    pub(crate) fn clear_notepad_selection(&mut self) {
        if self.notepad_editor.selection.take().is_some() {
            self.force_redraw();
        }
    }
    pub(crate) fn notepad_delete_selection(&mut self) -> bool {
        let Some((start, end)) = self
            .notepad_editor
            .selection
            .filter(|(start, end)| start < end)
        else {
            return false;
        };
        let Some(note_index) = self.active_note_index() else {
            return false;
        };
        notepad::delete_char_range(&mut self.notes[note_index].text, start, end);
        self.notepad_editor.cursor = start;
        self.notepad_editor.selection = None;
        self.sync_notepad_scroll();
        self.schedule_notepad_text_save();
        self.force_redraw();
        true
    }
    pub(crate) fn handle_notepad_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => self.unfocus_notepad(),
            KeyCode::Enter => {
                self.notepad_enter();
            }
            KeyCode::Backspace => {
                if !self.notepad_delete_selection() {
                    self.notepad_backspace();
                }
            }
            KeyCode::Left => {
                self.clear_notepad_selection();
                self.move_notepad_cursor(-1);
            }
            KeyCode::Right => {
                self.clear_notepad_selection();
                self.move_notepad_cursor(1);
            }
            KeyCode::Up => {
                self.clear_notepad_selection();
                self.scroll_notepad_lines(-1);
            }
            KeyCode::Down => {
                self.clear_notepad_selection();
                self.scroll_notepad_lines(1);
            }
            KeyCode::Char('a') | KeyCode::Char('A') if has_paste_modifier(key.modifiers) => {
                self.select_notepad_all();
            }
            KeyCode::Char('x') | KeyCode::Char('X') if has_paste_modifier(key.modifiers) => {
                self.cut_notepad_selection();
            }
            KeyCode::Char('c') | KeyCode::Char('C') if has_paste_modifier(key.modifiers) => {
                self.copy_notepad_selection();
            }
            KeyCode::Char(c)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::META,
                ) =>
            {
                self.insert_notepad_char(c);
            }
            _ => {}
        }
        self.redraw_if_needed(terminal)
    }
    pub(crate) fn notepad_checkbox_hover_from_mouse(
        &self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
        note_index: usize,
    ) -> bool {
        ui::notepad_checkbox_hover_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            self.sidebar_trail_base(),
            &self.notepad_list_state(),
            note_index,
            self.active_note_text(),
            self.notepad_editor.scroll,
            self.notepad_editor.selection,
            self.notepad_editor.checkbox_literal_edit,
        )
    }
    pub(crate) fn toggle_notepad_checkbox(&mut self, bracket_start: usize) {
        let Some(note_index) = self.active_note_index() else {
            return;
        };
        let text = &mut self.notes[note_index].text;
        if notepad::toggle_md_checkbox_at(text, bracket_start) {
            self.schedule_notepad_text_save();
            self.force_redraw();
        }
    }
    pub(crate) fn notepad_enter(&mut self) {
        let Some(note_index) = self.active_note_index() else {
            return;
        };
        if self.notes[note_index].text.chars().count() >= 2000 {
            return;
        }
        self.notepad_delete_selection();
        let text = &mut self.notes[note_index].text;
        let action = notepad::md_task_enter_action(text, self.notepad_editor.cursor);
        match action {
            notepad::MdTaskEnterAction::PlainNewline => {
                editor::insert_char(text, &mut self.notepad_editor.cursor, '\n');
            }
            notepad::MdTaskEnterAction::ContinueList { prefix } => {
                let insert = format!("\n{prefix}");
                editor::insert_str(text, &mut self.notepad_editor.cursor, &insert);
            }
            notepad::MdTaskEnterAction::ExitEmptyList { line_start } => {
                notepad::delete_char_range(text, line_start, self.notepad_editor.cursor);
                self.notepad_editor.cursor = line_start;
                editor::insert_char(text, &mut self.notepad_editor.cursor, '\n');
            }
        }
        self.sync_notepad_scroll();
        self.schedule_notepad_text_save();
        self.force_redraw();
    }
    pub(crate) fn insert_notepad_char(&mut self, ch: char) {
        let Some(note_index) = self.active_note_index() else {
            return;
        };
        if self.notes[note_index].text.chars().count() >= 2000 {
            return;
        }
        self.notepad_delete_selection();
        self.notepad_editor.checkbox_literal_edit = None;
        self.notepad_editor.suppress_terminal_cursor = false;
        let text = &mut self.notes[note_index].text;
        editor::insert_char(text, &mut self.notepad_editor.cursor, ch);
        self.sync_notepad_scroll();
        self.schedule_notepad_text_save();
        self.force_redraw();
    }
    pub(crate) fn notepad_backspace(&mut self) {
        let Some(note_index) = self.active_note_index() else {
            return;
        };
        let text = &mut self.notes[note_index].text;
        if let Some(bracket) = notepad::md_checkbox_demote_backspace(
            text,
            self.notepad_editor.cursor,
            self.notepad_editor.checkbox_literal_edit,
        ) {
            self.notepad_editor.checkbox_literal_edit = Some(bracket);
            self.notepad_editor.cursor = bracket + 1;
            self.sync_notepad_scroll();
            self.force_redraw();
            return;
        }
        self.notepad_editor.checkbox_literal_edit = None;
        if editor::backspace(text, &mut self.notepad_editor.cursor) {
            self.sync_notepad_scroll();
            self.schedule_notepad_text_save();
            self.force_redraw();
        }
    }
    pub(crate) fn move_notepad_cursor(&mut self, delta: i32) {
        let Some(note_index) = self.active_note_index() else {
            return;
        };
        self.notepad_editor.checkbox_literal_edit = None;
        self.notepad_editor.suppress_terminal_cursor = false;
        let text = &self.notes[note_index].text;
        if editor::move_cursor(text, &mut self.notepad_editor.cursor, delta) {
            self.sync_notepad_scroll();
            self.force_redraw();
        }
    }
    pub(crate) fn scroll_notepad_lines(&mut self, delta: i32) {
        let content_width = self.notepad_content_width();
        let line_count =
            notepad::wrapped_display_lines(self.active_note_text(), content_width).len();
        let viewport_rows = ui::notepad_text_viewport_rows(self.notepad_expanded) as usize;
        let max_scroll = line_count.saturating_sub(viewport_rows);
        if editor::scroll_lines(&mut self.notepad_editor.scroll, delta, max_scroll) {
            self.force_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bar::app::{App, NOTEPAD_SAVE_DEBOUNCE};
    use crate::bar::mouse_cursor::MouseCursorShape;
    use crate::bar::notepad::{self, Note};
    use crate::bar::ui;
    use crate::config::Config;
    use chrono::Utc;
    use crossterm::event::{
        KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::backend::CrosstermBackend;
    use ratatui::Terminal;
    use std::io;
    use std::time::{Duration, Instant};

    fn isolated_config() -> (tempfile::TempDir, Config) {
        notepad::isolated_test_config()
    }

    #[test]
    fn notepad_text_save_waits_for_debounce() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        app.schedule_notepad_text_save();
        assert!(app.notepad_save_deadline.is_some());
        app.flush_notepad_save_if_due();
        assert!(app.notepad_save_deadline.is_some());

        app.notepad_save_deadline = Some(Instant::now() - Duration::from_millis(1));
        app.flush_notepad_save_if_due();
        assert!(app.notepad_save_deadline.is_none());
    }

    #[test]
    fn notepad_text_save_does_not_flush_while_typing() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        app.schedule_notepad_text_save();
        app.notepad_save_deadline = Some(Instant::now() + NOTEPAD_SAVE_DEBOUNCE);
        app.flush_notepad_save_if_due();
        assert!(app.notepad_save_deadline.is_some());
    }

    #[test]
    fn persist_notepad_now_clears_pending_text_save() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        app.schedule_notepad_text_save();
        app.persist_notepad_now();
        assert!(app.notepad_save_deadline.is_none());
    }

    #[test]
    fn debounced_text_save_updates_saved_timestamp() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        let before = Utc::now() - chrono::Duration::minutes(5);
        app.notepad_last_saved_at = Some(before);
        let rows_before = app.rows_version;
        app.schedule_notepad_text_save();
        app.persist_notepad_now();
        assert!(app.notepad_last_saved_at.is_some_and(|at| at > before));
        assert_ne!(app.rows_version, rows_before);
    }

    #[test]
    fn notepad_period_inserts_text_instead_of_collapsing() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        let note = Note::new("Note 1", "", true);
        let note_id = note.id.clone();
        app.notes = vec![note];
        app.set_active_note_id(note_id);
        app.notepad_focused = true;
        app.notepad_expanded = true;
        app.notes[0].text = "hello".into();
        app.notepad_editor.cursor = 5;
        app.insert_notepad_char('.');
        assert!(app.notepad_expanded);
        assert_eq!(app.active_note_text(), "hello.");
        assert_eq!(app.notepad_editor.cursor, 6);
    }

    #[test]
    fn delete_note_confirm_requires_typing_yes() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        let note = Note::new("Gone", "bye", false);
        let note_id = note.id.clone();
        app.notes = vec![note];
        app.active_note_id = Some(note_id);
        app.notepad_expanded = true;
        app.notes_list_expanded = true;
        let row = ui::notepad_note_title_row_index(
            0,
            app.sidebar_trail_base(),
            &app.notepad_list_state(),
        )
        .unwrap();
        app.request_delete_note_confirm_at_row(row);
        assert!(app.delete_note_confirm.is_some());
        assert_eq!(app.notes.len(), 1);

        let key = crossterm::event::KeyEvent::new_with_kind(
            KeyCode::Char('y'),
            KeyModifiers::empty(),
            KeyEventKind::Press,
        );
        app.handle_delete_note_confirm_key(key).unwrap();
        app.commit_delete_note_confirm();
        assert_eq!(app.notes.len(), 1);

        let key = crossterm::event::KeyEvent::new_with_kind(
            KeyCode::Char('e'),
            KeyModifiers::empty(),
            KeyEventKind::Press,
        );
        app.handle_delete_note_confirm_key(key).unwrap();
        let key = crossterm::event::KeyEvent::new_with_kind(
            KeyCode::Char('s'),
            KeyModifiers::empty(),
            KeyEventKind::Press,
        );
        app.handle_delete_note_confirm_key(key).unwrap();
        app.commit_delete_note_confirm();
        assert!(app.notes.is_empty());
        assert!(app.delete_note_confirm.is_none());
    }

    #[test]
    fn delete_note_confirm_esc_cancels_without_deleting() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        let note = Note::new("Keep", "stay", false);
        let note_id = note.id.clone();
        app.notes = vec![note];
        app.active_note_id = Some(note_id);
        app.notepad_expanded = true;
        app.notes_list_expanded = true;
        let row = ui::notepad_note_title_row_index(
            0,
            app.sidebar_trail_base(),
            &app.notepad_list_state(),
        )
        .unwrap();
        app.request_delete_note_confirm_at_row(row);
        let key = crossterm::event::KeyEvent::new_with_kind(
            KeyCode::Esc,
            KeyModifiers::empty(),
            KeyEventKind::Press,
        );
        app.handle_delete_note_confirm_key(key).unwrap();
        assert_eq!(app.notes.len(), 1);
        assert!(app.delete_note_confirm.is_none());
    }

    #[test]
    fn delete_note_by_id_removes_note_and_promotes_next_active() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        let first = Note::new("Note 1", "one", false);
        let second = Note::new("Note 2", "two", false);
        let first_id = first.id.clone();
        let second_id = second.id.clone();
        app.notes = vec![first, second];
        app.set_active_note_id(second_id.clone());
        app.delete_note_by_id(&second_id);
        assert_eq!(app.notes.len(), 1);
        assert_eq!(app.active_note_id.as_deref(), Some(first_id.as_str()));
        app.delete_note_by_id(&first_id);
        assert!(app.notes.is_empty());
        assert!(app.active_note_id.is_none());
        assert!(!app.notepad_focused);
    }

    #[test]
    fn add_note_writes_note_file_and_prefs_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut app = App::new(&config).unwrap();
        app.add_note();
        assert_eq!(app.notes.len(), 1);
        let note_id = app.notes[0].id.clone();
        assert!(config.sidebar_note_path(&note_id).exists());
        assert!(config.sidebar_notepad_prefs_path().exists());
        let loaded = notepad::load(&config);
        assert_eq!(loaded.notes.len(), 1);
        assert_eq!(loaded.notes[0].title, "Note 1");
    }

    #[test]
    fn typing_autosaves_note_body_before_debounced_full_save() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut app = App::new(&config).unwrap();
        app.add_note();
        let note_id = app.notes[0].id.clone();
        app.notes[0].text = "draft text".into();
        app.schedule_notepad_text_save();
        let note_data = std::fs::read_to_string(config.sidebar_note_path(&note_id)).unwrap();
        assert!(note_data.contains("draft text"));
        assert!(app.notepad_save_deadline.is_some());
    }

    #[test]
    fn note_title_double_click_starts_rename_with_select_all() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        let note = Note::new("My Note", "body", true);
        let note_id = note.id.clone();
        app.notes = vec![note];
        app.active_note_id = Some(note_id);
        app.notepad_expanded = true;
        app.notes_list_expanded = true;

        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 8,
            row: 12,
            modifiers: KeyModifiers::empty(),
        };
        app.handle_notepad_note_title_click(&mouse, 0);
        assert!(app.rename.is_none());

        app.handle_notepad_note_title_click(&mouse, 0);
        let rename = app.rename.as_ref().expect("rename mode");
        assert!(rename.select_all);
        assert_eq!(rename.buffer, "My Note");
        assert!(matches!(
            &rename.target,
            ui::RenameTarget::Note { note_id: id } if id == &app.notes[0].id
        ));
    }

    #[test]
    fn note_title_hover_uses_text_cursor() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        // OSC 22 shapes only apply on full terminals (not Cursor/VS Code).
        app.host = crate::bar::host_terminal::detect_from_env(Some("ghostty"), false);
        app.sessions_expanded = false;
        app.notepad_expanded = true;
        app.notes_list_expanded = true;
        app.notes = vec![Note::new("Note 1", "", true)];
        let metrics = ui::layout_metrics_with_notepad(
            ratatui::layout::Size::new(40, 30),
            &app.rows,
            false,
            &app.notes,
            true,
            true,
        );
        let title_row =
            ui::notepad_note_title_row_index(0, app.rows.len(), &app.notepad_list_state())
                .expect("title row");
        let row_y = metrics
            .list_top_y
            .saturating_add(title_row.saturating_sub(app.scroll) as u16);
        app.last_mouse = Some(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 8,
            row: row_y,
            modifiers: KeyModifiers::empty(),
        });
        app.last_mouse_activity = Instant::now();
        assert_eq!(
            app.resolve_sidebar_mouse_cursor(Some(&metrics)),
            Some(MouseCursorShape::Text)
        );
    }

    #[test]
    fn notepad_focused_over_toolbar_uses_pointer_not_text() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        app.host = crate::bar::host_terminal::detect_from_env(Some("ghostty"), false);
        app.notepad_focused = true;
        app.notepad_expanded = true;
        let metrics = ui::layout_metrics_with_notepad(
            ratatui::layout::Size::new(40, 30),
            &app.rows,
            true,
            &app.notes,
            true,
            false,
        );
        app.last_mouse = Some(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 2,
            row: metrics.toolbar_top_y,
            modifiers: KeyModifiers::empty(),
        });
        app.last_mouse_activity = Instant::now();
        assert_eq!(
            app.resolve_sidebar_mouse_cursor(Some(&metrics)),
            Some(MouseCursorShape::Pointer)
        );
    }

    fn note_title_mouse_row(app: &App, metrics: &ui::LayoutMetrics, note_index: usize) -> u16 {
        let row = ui::notepad_note_title_row_index(
            note_index,
            app.sidebar_trail_base(),
            &app.notepad_list_state(),
        )
        .unwrap();
        metrics.list_top_y + row.saturating_sub(app.scroll) as u16
    }

    #[test]
    fn note_title_click_without_drag_toggles_expand() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        app.notes = vec![Note::new("Note 1", "body", false)];
        app.notepad_expanded = true;
        app.notes_list_expanded = true;
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 40), &app.rows);
        let row_y = note_title_mouse_row(&app, &metrics, 0);
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
        assert!(app.note_drag.pending());
        assert!(!app.note_drag.active());
        app.handle_mouse(&up, &metrics);
        assert!(!app.note_drag.pending());
        assert!(!app.note_drag.active());
        assert!(app.notes[0].expanded);
    }

    #[test]
    fn note_drag_drop_reorders_notes() {
        let (_dir, config) = isolated_config();
        let mut app = App::new(&config).unwrap();
        app.notes = vec![
            Note::new("Note 1", "one", false),
            Note::new("Note 2", "two", false),
            Note::new("Note 3", "three", false),
        ];
        app.notepad_expanded = true;
        app.notes_list_expanded = true;
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 40), &app.rows);
        let source_y = note_title_mouse_row(&app, &metrics, 0);
        let target_y = note_title_mouse_row(&app, &metrics, 2);
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
        assert_eq!(app.notes[0].title, "Note 2");
        assert_eq!(app.notes[1].title, "Note 3");
        assert_eq!(app.notes[2].title, "Note 1");
    }
}
