use super::*;
use super::{
    AcknowledgedCompletion, App, CLIPBOARD_NOTICE_DURATION, CLOSE_HOLD_MIN_SETTLE,
    CLOSE_HOLD_MISSED_REPEAT_TOLERANCE, CLOSE_HOLD_RELEASE_SLACK, CLOSE_HOLD_REPEAT_LEARN_MIN,
    CLOSE_HOLD_SILENCE_MIN, DIGIT_JUMP_TIMEOUT, NOTEPAD_DOUBLE_CLICK_TIMEOUT,
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
    pub(crate) fn sidebar_trail_base(&self) -> usize {
        ui::sidebar_trail_base_row(self.rows.len(), self.sessions_expanded)
    }
    pub(crate) fn layout_metrics(&self, size: ratatui::layout::Size) -> ui::LayoutMetrics {
        ui::layout_metrics_with_notepad(
            size,
            &self.rows,
            self.sessions_expanded,
            &self.notes,
            self.notepad_expanded,
            self.update_banner.is_some(),
        )
    }
    pub(crate) fn needs_run_spinner_animation(&self) -> bool {
        ui::needs_run_spinner_animation(&self.sessions)
    }
    pub(crate) fn needs_continuous_animation(&self) -> bool {
        self.needs_run_spinner_animation()
            || self.needs_coming_soon_animation()
            || (self.notepad_focused && self.notepad_expanded)
    }
    pub(crate) fn workspace_pane_has_focus(&self) -> bool {
        crate::daemon::tmux::ui_window_active_pane_index(&self.config.tmux_ui_session) == Some(1)
    }
    pub(crate) fn animation_interval_ms(&self) -> u64 {
        if self.needs_run_spinner_animation() {
            ui::RUN_SPINNER_INTERVAL_MS
        } else {
            ui::COMING_SOON_INTERVAL_MS
        }
    }
    pub(crate) fn advance_anim_frame(&mut self) {
        if !self.needs_continuous_animation() {
            return;
        }
        if self.last_anim_tick.elapsed() < Duration::from_millis(self.animation_interval_ms()) {
            return;
        }
        self.anim_frame = self.anim_frame.wrapping_add(1);
        self.last_anim_tick = Instant::now();
    }
    pub(crate) fn needs_coming_soon_animation(&self) -> bool {
        !self.coming_soon_anims.is_empty()
    }
    pub(crate) fn active_coming_soon_frames(&self) -> Vec<(ToolbarAction, usize)> {
        let mut frames: Vec<_> = self
            .coming_soon_anims
            .iter()
            .filter_map(|(action, started)| {
                let frame =
                    (started.elapsed().as_millis() as u64 / ui::COMING_SOON_INTERVAL_MS) as usize;
                if frame < ui::COMING_SOON_CYCLE_FRAMES {
                    Some((*action, frame))
                } else {
                    None
                }
            })
            .collect();
        frames.sort_by_key(|(action, _)| *action);
        frames
    }
    pub(crate) fn expire_coming_soon_anims_if_due(&mut self) {
        self.coming_soon_anims
            .retain(|_, started| started.elapsed().as_millis() < ui::COMING_SOON_CYCLE_MS as u128);
    }
    pub(crate) fn ensure_sidebar_width(&self) {
        let desired = self.user_pane_width.unwrap_or_else(|| {
            ui::desired_pane_width(&self.rows, self.session_row_count(), &self.digit_buffer)
        });
        let _ = crate::daemon::tmux::resize_current_pane_width(desired);
    }
    pub(crate) fn terminal_size(terminal: &Terminal<CrosstermBackend<io::Stdout>>) -> ratatui::layout::Size {
        terminal
            .size()
            .unwrap_or(ratatui::layout::Size::new(80, 24))
    }
    pub(crate) fn sidebar_snapshot<'a>(
        &'a self,
        coming_soon_frames: &'a [(ui::ToolbarAction, usize)],
        clipboard_notice: Option<&'a str>,
    ) -> ui::SidebarSnapshot<'a> {
        let drag_active = self.group_drag.active() || self.note_drag.active();
        ui::SidebarSnapshot {
            sessions: ui::SessionsView {
                rows: &self.rows,
                selected: self.effective_selected_row(),
                scroll: self.scroll,
                digit_buffer: &self.digit_buffer,
                close_modifier_held: self.close_modifier_held,
                hover_row: if drag_active { None } else { self.hover_row },
                close_target: self.close_target_row,
                group_hover_row: if drag_active { None } else { self.group_hover_row },
                sessions_expanded: self.sessions_expanded,
                folded_groups: &self.folded_groups,
                group_order: &self.group_order,
                group_drag: &self.group_drag,
                sessions_title_hover: self.sessions_title_hover,
                sessions_title_add_hover: self.sessions_title_add_hover,
                anim_frame: self.anim_frame,
            },
            notepad: ui::NotepadView {
                notes: self.display_notes(),
                expanded: self.notepad_expanded,
                notes_list_expanded: self.notes_list_expanded,
                active_note_index: self.active_note_index(),
                text: self.active_note_text(),
                cursor: self.notepad_editor.cursor,
                scroll: self.notepad_editor.scroll,
                focused: self.notepad_focused,
                section_header_hover: self.notepad_section_header_hover,
                section_add_hover: self.notepad_section_add_hover,
                note_hover: self.effective_note_hover(),
                note_drag: &self.note_drag,
                selection: self.notepad_editor.selection,
                last_saved_at: self.notepad_last_saved_at,
            },
            chrome: ui::ChromeView {
                toolbar_hover: self.toolbar_hover,
                coming_soon_frames,
                settings_hover: self.settings_hover,
                leave_hover: self.leave_hover,
                workspace_settings_open: self.workspace_settings_open,
                workspace_new_session_open: self.workspace_new_session_open,
            },
            overlay: ui::OverlayView {
                context_menu: self.context_menu.as_ref(),
                rename: self.rename.as_ref(),
                delete_note_confirm: self.delete_note_confirm.as_ref(),
                clipboard_notice,
                update_banner: self.update_banner.as_ref(),
                update_upgrade_hover: self.update_upgrade_hover,
                update_dismiss_hover: self.update_dismiss_hover,
            },
        }
    }
    pub(crate) fn redraw_if_needed(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        let size = Self::terminal_size(terminal);
        let metrics = self.layout_metrics(size);
        let display_notes = if self.note_drag.active() && !self.notes_preview.is_empty() {
            self.notes_preview.as_slice()
        } else {
            self.notes.as_slice()
        };
        let active_note_index = self
            .active_note_id
            .as_ref()
            .and_then(|id| display_notes.iter().position(|note| &note.id == id));
        let notepad_state = ui::notepad_list_state(
            display_notes,
            self.notepad_expanded,
            self.notes_list_expanded,
            active_note_index,
        );
        let total_rows = ui::total_list_rows(
            self.rows.len(),
            self.sessions_expanded,
            &notepad_state,
        );
        self.scroll = ui::clamp_list_scroll(self.scroll, total_rows, metrics.list_height);
        if self.notepad_scroll_pending {
            self.scroll = ui::clamp_list_scroll(
                ui::ensure_active_note_visible(
                    self.scroll,
                    metrics.list_height,
                    ui::sidebar_trail_base_row(self.rows.len(), self.sessions_expanded),
                    &notepad_state,
                ),
                total_rows,
                metrics.list_height,
            );
            self.notepad_scroll_pending = false;
        }
        if self.selection_scroll_sync {
            self.scroll =
                ui::ensure_selection_visible(self.selected, self.scroll, metrics.list_height);
            self.selection_scroll_sync = false;
        }

        if self.last_time_tick.elapsed() >= Duration::from_secs(30) {
            self.last_time_tick = Instant::now();
            self.rows_version = self.rows_version.wrapping_add(1);
        }

        let layout_changed = self.render_cache.size != Some((size.width, size.height))
            || self.render_cache.scroll != Some(self.scroll);
        if layout_changed
            && !self.workspace_pane_has_focus()
            && self.user_pane_width.is_none()
        {
            self.ensure_sidebar_width();
        }
        let close_visual_changed = self.close_modifier_held != self.render_cache.close_mode;
        let hover_visual_changed = self.hover_row != self.render_cache.hover_row
            || self.close_target_row != self.render_cache.close_target_row
            || self.group_hover_row != self.render_cache.group_hover_row
            || self.toolbar_hover != self.render_cache.toolbar_hover
            || self.settings_hover != self.render_cache.settings_hover
            || self.leave_hover != self.render_cache.leave_hover
            || self.workspace_settings_open != self.render_cache.workspace_settings_open
            || self.workspace_new_session_open != self.render_cache.workspace_new_session_open
            || self.notepad_section_header_hover != self.render_cache.notepad_section_header_hover
            || self.notepad_section_add_hover != self.render_cache.notepad_section_add_hover
            || self.notepad_note_hover != self.render_cache.notepad_note_hover
            || self.sessions_title_hover != self.render_cache.sessions_title_hover
            || self.sessions_title_add_hover != self.render_cache.sessions_title_add_hover
            || self.update_upgrade_hover != self.render_cache.update_upgrade_hover
            || self.update_dismiss_hover != self.render_cache.update_dismiss_hover;
        let notepad_visual_changed = self.sessions_expanded != self.render_cache.sessions_expanded
            || self.notepad_expanded != self.render_cache.notepad_expanded
            || self.notes_list_expanded != self.render_cache.notes_list_expanded
            || self.notepad_focused != self.render_cache.notepad_focused
            || self.notes != self.render_cache.notes
            || self.active_note_id != self.render_cache.active_note_id
            || self.notepad_editor.cursor != self.render_cache.notepad_cursor
            || self.notepad_editor.scroll != self.render_cache.notepad_scroll
            || self.notepad_editor.selection != self.render_cache.notepad_selection
            || self.notepad_save_badge_label() != self.render_cache.notepad_save_badge;
        let anim_visual_changed = self.needs_continuous_animation()
            && self.render_cache.anim_frame != Some(self.anim_frame);
        let coming_soon_frames = self.active_coming_soon_frames();
        let coming_soon_changed = coming_soon_frames != self.render_cache.coming_soon_frames;
        let clipboard_notice = self.active_status_notice();
        let clipboard_notice_changed = clipboard_notice != self.render_cache.clipboard_notice;
        let update_banner_label = self.update_banner.as_ref().map(|b| b.label.clone());
        let update_banner_changed = update_banner_label != self.render_cache.update_banner;
        let hover_only_redraw = hover_visual_changed
            && !close_visual_changed
            && !anim_visual_changed
            && !notepad_visual_changed
            && !self.group_drag.active()
            && !self.note_drag.active()
            && !layout_changed
            && self.render_cache.rows_version == self.rows_version;
        let data_changed = !hover_only_redraw
            && !anim_visual_changed
            && self.rows_version != self.render_cache.rows_version;
        if close_visual_changed
            || hover_visual_changed
            || notepad_visual_changed
            || anim_visual_changed
            || coming_soon_changed
            || clipboard_notice_changed
            || update_banner_changed
            || self.group_drag.active()
            || self.note_drag.active()
            || data_changed
            || layout_changed
        {
            // Always allow the first render so the sidebar isn't blank on startup.
            let snap = self.sidebar_snapshot(&coming_soon_frames, clipboard_notice.as_deref());
            terminal.draw(|f| ui::draw(f, &snap))?;
            if !hover_only_redraw {
                self.render_cache.rows_version = self.rows_version;
            }
            self.render_cache.size = Some((size.width, size.height));
            self.render_cache.scroll = Some(self.scroll);
            self.render_cache.close_mode = self.close_modifier_held;
            self.render_cache.hover_row = self.hover_row;
            self.render_cache.close_target_row = self.close_target_row;
            self.render_cache.group_hover_row = self.group_hover_row;
            self.render_cache.toolbar_hover = self.toolbar_hover;
            self.render_cache.coming_soon_frames = coming_soon_frames;
            self.render_cache.settings_hover = self.settings_hover;
            self.render_cache.leave_hover = self.leave_hover;
            self.render_cache.workspace_settings_open = self.workspace_settings_open;
            self.render_cache.workspace_new_session_open = self.workspace_new_session_open;
            self.render_cache.sessions_expanded = self.sessions_expanded;
            self.render_cache.notepad_expanded = self.notepad_expanded;
            self.render_cache.notes_list_expanded = self.notes_list_expanded;
            self.render_cache.notepad_focused = self.notepad_focused;
            self.render_cache.notes = self.notes.clone();
            self.render_cache.active_note_id = self.active_note_id.clone();
            self.render_cache.notepad_cursor = self.notepad_editor.cursor;
            self.render_cache.notepad_scroll = self.notepad_editor.scroll;
            self.render_cache.notepad_section_header_hover = self.notepad_section_header_hover;
            self.render_cache.notepad_section_add_hover = self.notepad_section_add_hover;
            self.render_cache.notepad_note_hover = self.notepad_note_hover;
            self.render_cache.notepad_selection = self.notepad_editor.selection;
            self.render_cache.notepad_save_badge = self.notepad_save_badge_label();
            self.render_cache.sessions_title_hover = self.sessions_title_hover;
            self.render_cache.sessions_title_add_hover = self.sessions_title_add_hover;
            self.render_cache.anim_frame = Some(self.anim_frame);
            self.render_cache.clipboard_notice = clipboard_notice.clone();
            self.render_cache.update_banner = update_banner_label;
            self.render_cache.update_upgrade_hover = self.update_upgrade_hover;
            self.render_cache.update_dismiss_hover = self.update_dismiss_hover;
            self.sync_sidebar_mouse_cursor(Some(&metrics));
        }
        Ok(())
    }
    pub(crate) fn active_status_notice(&self) -> Option<String> {
        self.clipboard_notice_until
            .is_some_and(|until| Instant::now() < until)
            .then(|| self.clipboard_notice_text.clone())
            .flatten()
    }
}
