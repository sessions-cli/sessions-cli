use super::super::{parse_digit_ordinal, App, DIGIT_JUMP_TIMEOUT};
use crate::bar::group_order::{self, SidebarGroupOrder};
use crate::bar::ui::{self, RowKind};
use crate::model::Session;
use std::time::Instant;

impl App {

    pub(crate) fn sync_selection_to_active(&mut self, force: bool) {
        if self.pending_focus_tab_index.is_some() {
            return;
        }
        // Keep an explicit non-active selection while the user is browsing.
        if !force {
            if let Some(session) = self.session_at(self.selected) {
                if !session.is_active {
                    return;
                }
            }
        }
        if let Some(&idx) = self.selectable.iter().find(|&&i| {
            matches!(
                &self.rows[i],
                RowKind::Session {
                    session,
                    ..
                } if session.is_active
            )
        }) {
            self.set_selected(idx);
            self.force_redraw();
        }
    }
    pub(crate) fn force_redraw(&mut self) {
        self.rows_version = self.rows_version.wrapping_add(1);
    }
    pub(crate) fn force_layout_redraw(&mut self) {
        self.rows_version = self.rows_version.wrapping_add(1);
        self.render_cache.size = None;
        self.render_cache.scroll = None;
    }
    pub(crate) fn handle_terminal_resize(&mut self, width: u16, height: u16) {
        let height_changed = self
            .render_cache.size
            .is_none_or(|(_, last_height)| last_height != height);
        if height_changed {
            self.force_layout_redraw();
        } else {
            self.rows_version = self.rows_version.wrapping_add(1);
            self.render_cache.size = Some((width, height));
        }
        if self.last_mouse.is_some() {
            self.pointer_hover_refresh_pending = true;
        }
    }
    pub(crate) fn ensure_active_session_visible(&mut self) {
        let Some(active) = self.sessions.iter().find(|session| session.is_active) else {
            return;
        };
        let cwd = active.cwd_label.clone();
        if self.expanded_groups.contains(&cwd) {
            return;
        }
        let group: Vec<Session> = self
            .sessions
            .iter()
            .filter(|session| session.cwd_label == cwd)
            .cloned()
            .collect();
        let visible = group_order::visible_sessions_in_group(&group, false);
        if visible.iter().any(|session| session.id == active.id) {
            return;
        }
        if self.expanded_groups.insert(cwd) {
            self.schedule_sidebar_ui_save();
        }
    }
    pub(crate) fn effective_group_order(&self) -> Vec<String> {
        match (&self.group_drag.source, &self.group_drag.hover) {
            (Some(from), Some(to)) if from != to => {
                group_order::preview_order(&self.group_order, from, to)
            }
            _ => self.group_order.clone(),
        }
    }
    /// Refresh session clones embedded in row items without rebuilding group structure.
    pub(crate) fn sync_row_sessions(&mut self) {
        for row in &mut self.rows {
            let ui::RowKind::Session { session } = row else {
                continue;
            };
            if let Some(fresh) = self.sessions.iter().find(|s| s.id == session.id) {
                *session = fresh.clone();
            }
        }
        self.rows_version = self.rows_version.wrapping_add(1);
    }

    pub(crate) fn rebuild_rows(&mut self) {
        let group_order = self.effective_group_order();
        let previously_selected_session = self
            .group_drag
            .preserved_session_id
            .clone()
            .or_else(|| {
                self.sidebar_ui_selected_sessions_session_id.as_deref().and_then(|ssn| {
                    self.sessions.iter().find(|session| {
                        session.sessions_session_id.as_deref() == Some(ssn)
                    })
                    .map(|session| session.id.clone())
                })
            })
            .or_else(|| {
                self.session_at(self.selected)
                    .map(|session| session.id.clone())
            });
        let previously_selected_group = self
            .group_drag
            .preserved_group_toggle
            .clone()
            .or_else(|| ui::group_toggle_at(&self.rows, self.selected).map(str::to_string));
        let mut display_group_order = group_order;
        group_order::ensure_labels(
            &mut display_group_order,
            &group_order::unique_labels(&self.sessions),
        );
        self.ensure_active_session_visible();
        self.rows = ui::build_rows(
            &self.sessions,
            &self.expanded_groups,
            &self.folded_groups,
            &display_group_order,
        );
        self.selectable = ui::selectable_indices(&self.rows);
        if !self.selection_initialized {
            let restored_from_ssn = self
                .sidebar_ui_selected_sessions_session_id
                .as_deref()
                .and_then(|ssn| {
                    self.selectable.iter().copied().find(|&row_idx| {
                        self.session_at(row_idx).is_some_and(|session| {
                            session.sessions_session_id.as_deref() == Some(ssn)
                        })
                    })
                });
            if let Some(idx) = restored_from_ssn {
                self.set_selected(idx);
            } else if let Some(idx) = self
                .selectable
                .iter()
                .find(|&&i| {
                    if let RowKind::Session { session, .. } = &self.rows[i] {
                        session.is_active
                    } else {
                        false
                    }
                })
                .copied()
            {
                self.set_selected(idx);
            } else if let Some(&first) = self.selectable.first() {
                self.set_selected(first);
            }
            self.selection_initialized = true;
        } else if let Some(pending_tab_index) = self.pending_focus_tab_index {
            if let Some(idx) = self.selectable.iter().copied().find(|&row_idx| {
                self.session_at(row_idx)
                    .is_some_and(|session| session.tab_index == pending_tab_index)
            }) {
                self.set_selected(idx);
            } else if !self.selectable.contains(&self.selected) {
                if let Some(&first) = self.selectable.first() {
                    self.set_selected(first);
                }
            }
        } else if self.group_drag.active() {
            let restored = previously_selected_session
                .as_deref()
                .and_then(|selected_id| self.session_row_index(selected_id))
                .or_else(|| {
                    previously_selected_group.as_deref().and_then(|cwd_label| {
                        self.rows.iter().enumerate().find_map(|(idx, row)| match row {
                            RowKind::GroupToggle { cwd_label: label, .. } if label == cwd_label => {
                                Some(idx)
                            }
                            _ => None,
                        })
                    })
                });
            if let Some(idx) = restored {
                self.set_selected(idx);
            }
        } else if let Some(selected_id) = previously_selected_session.as_deref() {
            if let Some(idx) = self.selectable.iter().copied().find(|&row_idx| {
                self.session_at(row_idx)
                    .is_some_and(|session| session.id == selected_id)
            }) {
                self.set_selected(idx);
            } else if !self.selectable.contains(&self.selected) {
                if let Some(&first) = self.selectable.first() {
                    self.set_selected(first);
                }
            }
        } else if let Some(cwd_label) = previously_selected_group.as_deref() {
            if let Some(idx) = self.selectable.iter().copied().find(|&row_idx| {
                ui::group_toggle_at(&self.rows, row_idx) == Some(cwd_label)
            }) {
                self.set_selected(idx);
            } else if !self.selectable.contains(&self.selected) {
                if let Some(&first) = self.selectable.first() {
                    self.set_selected(first);
                }
            }
        } else if !self.selectable.contains(&self.selected) {
            if let Some(&first) = self.selectable.first() {
                self.set_selected(first);
            }
        }
        self.reconcile_close_hover(None);
        self.force_redraw();
    }

    /// Sidebar focus persists `last_active_sessions_session_id` immediately (see
    /// [`crate::session::update_last_active`] — not debounced, acceptable for focus).
    fn persist_last_active_for_row(&self, row_idx: usize) {
        if let Some(ssn) = self
            .session_at(row_idx)
            .and_then(|session| session.sessions_session_id.clone())
        {
            let _ = crate::session::update_last_active(&self.config, &ssn);
        }
    }

    pub(crate) fn focus_row(&mut self, row_idx: usize) -> bool {
        if !matches!(self.rows.get(row_idx), Some(RowKind::Session { .. })) {
            return false;
        }
        self.persist_last_active_for_row(row_idx);
        let Some(session) = self.session_at(row_idx) else {
            return false;
        };
        let tab_index = session.tab_index;
        let sessions_session_id = session.sessions_session_id.clone();
        let live_tab_index = sessions_session_id.as_deref().and_then(|ssn| {
            crate::daemon::tmux::list_live_sessions_session_ids(&self.config.tmux_session)
                .ok()
                .and_then(|live| live.get(ssn).copied())
        });
        if self.session_at(row_idx).is_some_and(|session| session.is_active)
            && live_tab_index.is_some_and(|live| live == tab_index)
        {
            self.pending_focus_tab_index = None;
            if let Some(session_id) = self.session_at(row_idx).map(|session| session.id.clone()) {
                if self.acknowledge_session_completion(&session_id) {
                    self.rebuild_rows();
                    self.force_redraw();
                }
            }
            return true;
        }
        self.set_selected(row_idx);
        let focused_tab_index = live_tab_index.unwrap_or(tab_index);
        self.pending_focus_tab_index = Some(focused_tab_index);
        let focus_ok = if let Some(ref ssn) = sessions_session_id {
            crate::daemon::tmux::select_window_by_sessions_session_id(
                &self.config.tmux_session,
                ssn,
            )
            .is_ok()
                || crate::daemon::tmux::select_window(
                    &self.config.tmux_session,
                    focused_tab_index,
                )
                .is_ok()
        } else {
            crate::daemon::tmux::select_window(&self.config.tmux_session, focused_tab_index)
                .is_ok()
        };
        if focus_ok {
            self.apply_exclusive_focus(focused_tab_index);
            if let Some((cwd, cwd_label, session_id)) = self
                .sessions
                .iter()
                .find(|session| session.tab_index == focused_tab_index)
                .map(|session| {
                    (
                        session.cwd.clone(),
                        session.cwd_label.clone(),
                        session.id.clone(),
                    )
                })
            {
                let _ = crate::session::workspace_usage::WorkspaceUsageStore::record_focus_at(
                    &self.config.home,
                    &cwd,
                    &cwd_label,
                );
                let _ = self.acknowledge_session_completion(&session_id);
            }
            self.rebuild_rows();
            self.force_redraw();
            true
        } else {
            self.pending_focus_tab_index = None;
            false
        }
    }

    pub(crate) fn ordinal_for_row(&self, row_idx: usize) -> Option<u32> {
        self.rows
            .iter()
            .take(row_idx + 1)
            .filter(|row| matches!(row, RowKind::Session { .. }))
            .count()
            .try_into()
            .ok()
            .filter(|ordinal| *ordinal > 0)
    }
    pub(crate) fn set_selected(&mut self, idx: usize) {
        if self.selected != idx {
            self.selected = idx;
            self.selection_scroll_sync = true;
        }
        let selected_ssn = self
            .session_at(idx)
            .and_then(|session| session.sessions_session_id.clone());
        if selected_ssn != self.sidebar_ui_selected_sessions_session_id {
            self.sidebar_ui_selected_sessions_session_id = selected_ssn;
            self.schedule_sidebar_ui_save();
        }
    }
    pub(crate) fn scroll_list_viewport(&mut self, delta: i32, metrics: &ui::LayoutMetrics) {
        let total_rows = ui::total_list_rows(
            self.rows.len(),
            self.sessions_expanded,
            &self.notepad_list_state(),
        );
        let next = ui::scroll_list_by(self.scroll, delta, total_rows, metrics.list_height);
        if next != self.scroll {
            self.scroll = next;
            self.force_redraw();
        }
    }
    pub(crate) fn move_selection(&mut self, delta: i32) {
        self.unfocus_notepad();
        if self.selectable.is_empty() {
            return;
        }
        let pos = self
            .selectable
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0) as i32;
        let new_pos = (pos + delta).clamp(0, self.selectable.len() as i32 - 1);
        self.set_selected(self.selectable[new_pos as usize]);
        self.hover_row = None;
        self.force_redraw();
    }
    pub(crate) fn activate_selected(&mut self) {
        self.unfocus_notepad();
        if let Some(cwd_label) = ui::group_toggle_at(&self.rows, self.selected) {
            self.toggle_group_expanded(cwd_label.to_string());
            return;
        }

        let selected = self.selected;
        if matches!(self.rows.get(selected), Some(RowKind::Session { .. })) {
            self.restore_workspace_if_panel_open();
        }
        let _ = self.focus_row(selected);
    }
    pub(crate) fn selected_ordinal(&self) -> Option<u32> {
        self.ordinal_for_row(self.selected)
    }
    /// Row index for rendering — preview reorder shifts indices; pin by id during drag.
    pub(crate) fn effective_selected_row(&self) -> usize {
        if !self.group_drag.active() {
            return self.selected;
        }
        if let Some(session_id) = &self.group_drag.preserved_session_id {
            if let Some(idx) = self.session_row_index(session_id) {
                return idx;
            }
        }
        if let Some(cwd_label) = &self.group_drag.preserved_group_toggle {
            if let Some(idx) = self.rows.iter().enumerate().find_map(|(idx, row)| match row {
                RowKind::GroupToggle { cwd_label: label, .. } if label == cwd_label => Some(idx),
                _ => None,
            }) {
                return idx;
            }
        }
        self.selected
    }
    pub(crate) fn push_digit(&mut self, digit: char) {
        self.digit_buffer.push(digit);
        let max = self.session_row_count();
        if max == 0 {
            self.clear_digit_buffer();
            return;
        }

        let parsed = parse_digit_ordinal(&self.digit_buffer);
        let Some(ordinal) = parsed else {
            self.clear_digit_buffer();
            self.force_redraw();
            return;
        };

        if ordinal as usize > max {
            if self.digit_buffer.len() >= 2 {
                self.clear_digit_buffer();
                self.force_redraw();
            } else {
                self.digit_deadline = Some(Instant::now() + DIGIT_JUMP_TIMEOUT);
            }
            return;
        }

        let can_jump_now = self.digit_buffer.len() >= 2
            || ordinal == 10
            || (ordinal > 0 && (ordinal as usize * 10) > max);
        if can_jump_now {
            self.jump_to_ordinal(ordinal);
            self.clear_digit_buffer();
        } else {
            self.digit_deadline = Some(Instant::now() + DIGIT_JUMP_TIMEOUT);
            self.force_redraw();
        }
    }
    pub(crate) fn flush_digit_buffer_if_due(&mut self) {
        let Some(deadline) = self.digit_deadline else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }
        let Some(ordinal) = parse_digit_ordinal(&self.digit_buffer) else {
            self.clear_digit_buffer();
            return;
        };
        if ordinal > 0 && (ordinal as usize) <= self.session_row_count() {
            self.jump_to_ordinal(ordinal);
        }
        self.clear_digit_buffer();
    }
    pub(crate) fn session_row_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row, RowKind::Session { .. }))
            .count()
    }
    pub(crate) fn clear_digit_buffer(&mut self) {
        self.digit_buffer.clear();
        self.digit_deadline = None;
    }
    pub(crate) fn jump_to_ordinal(&mut self, ordinal: u32) {
        let session_rows: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| matches!(row, RowKind::Session { .. }).then_some(idx))
            .collect();
        if let Some(&idx) = session_rows.get(ordinal.saturating_sub(1) as usize) {
            self.set_selected(idx);
            self.force_redraw();
            self.activate_selected();
        }
    }
    pub(crate) fn select_session_by_tab_index(&mut self, tab_index: u32) -> bool {
        let Some(row_idx) = self.selectable.iter().copied().find(|&row_idx| {
            self.session_at(row_idx)
                .is_some_and(|session| session.tab_index == tab_index)
        }) else {
            return false;
        };
        self.set_selected(row_idx);
        self.force_redraw();
        true
    }
    pub(crate) fn session_row_index(&self, session_id: &str) -> Option<usize> {
        self.rows
            .iter()
            .enumerate()
            .find_map(|(idx, row)| match row {
                RowKind::Session { session, .. } if session.id == session_id => Some(idx),
                _ => None,
            })
    }
    pub(crate) fn session_at(&self, row: usize) -> Option<&Session> {
        match self.rows.get(row)? {
            RowKind::Session { session, .. } => Some(session),
            _ => None,
        }
    }
    pub(crate) fn restore_preserved_selection(&mut self) {
        if let Some(session_id) = self.group_drag.preserved_session_id.as_deref() {
            if let Some(idx) = self.selectable.iter().copied().find(|&row_idx| {
                self.session_at(row_idx)
                    .is_some_and(|session| session.id == session_id)
            }) {
                self.set_selected(idx);
            }
            return;
        }
        if let Some(cwd_label) = self.group_drag.preserved_group_toggle.as_deref() {
            if let Some(idx) = self.selectable.iter().copied().find(|&row_idx| {
                ui::group_toggle_at(&self.rows, row_idx) == Some(cwd_label)
            }) {
                self.set_selected(idx);
            }
        }
    }
    pub(crate) fn maybe_clear_list_hover_suppress(&mut self, mouse_row: u16) {
        if self.suppress_list_hover_after_group_drag
            && self.suppress_list_hover_y != Some(mouse_row)
        {
            self.suppress_list_hover_after_group_drag = false;
            self.suppress_list_hover_y = None;
        }
    }
    pub(crate) fn list_hover_updates_suppressed(&self, mouse_row: u16) -> bool {
        self.suppress_list_hover_after_group_drag
            && self.suppress_list_hover_y == Some(mouse_row)
    }
    pub(crate) fn begin_list_hover_suppress_after_group_drag(&mut self, mouse_row: u16) {
        self.hover_row = None;
        self.group_hover_row = None;
        self.notepad_note_hover = None;
        self.suppress_list_hover_after_group_drag = true;
        self.suppress_list_hover_y = Some(mouse_row);
    }
}

#[cfg(test)]
mod tests {
    use crate::bar::app::{parse_digit_ordinal, App};
    use crate::bar::app::test_fixtures::{sample_session, sample_session_in_group};
    use crate::bar::client::ClientEvent;
    use crate::bar::ui::{GroupDragState, RowKind};
    use crate::config::Config;
    use chrono::Utc;
    use std::collections::HashSet;

    #[test]
    fn parse_digit_ordinal_maps_zero_to_ten() {
        assert_eq!(parse_digit_ordinal("0"), Some(10));
        assert_eq!(parse_digit_ordinal("1"), Some(1));
        assert_eq!(parse_digit_ordinal("12"), Some(12));
        assert_eq!(parse_digit_ordinal(""), None);
        assert_eq!(parse_digit_ordinal("00"), None);
    }

    #[test]
    fn selected_ordinal_skips_group_toggles() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = (1..=8)
            .map(|i| sample_session(&format!("tmux:win:{i}"), i, &format!("thread-{i}"), false))
            .collect();
        let mut expanded_groups = HashSet::new();
        expanded_groups.insert("~/tmp".to_string());
        app.expanded_groups = expanded_groups;
        app.rebuild_rows();

        let target = app
            .rows
            .iter()
            .position(
                |row| matches!(row, RowKind::Session { session } if session.id == "tmux:win:8"),
            )
            .unwrap();
        app.selected = target;

        assert_eq!(app.selected_ordinal(), Some(1));
    }

    #[test]
    fn rebuild_rows_keeps_selected_session_after_focus_state_changes() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", false),
            sample_session("tmux:win:2", 2, "two", true),
        ];
        app.rebuild_rows();

        app.selected = app
            .rows
            .iter()
            .position(
                |row| matches!(row, RowKind::Session { session } if session.id == "tmux:win:1"),
            )
            .unwrap();

        for session in &mut app.sessions {
            session.is_active = session.id == "tmux:win:1";
        }
        app.rebuild_rows();

        assert_eq!(
            app.session_at(app.selected)
                .map(|session| session.id.as_str()),
            Some("tmux:win:1")
        );
    }

    #[test]
    fn rebuild_rows_prefers_persisted_sessions_session_id_over_active() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        let mut inactive = sample_session("tmux:win:1", 1, "one", false);
        inactive.sessions_session_id = Some("ssn_persisted".into());
        let mut active = sample_session("tmux:win:2", 2, "two", true);
        active.sessions_session_id = Some("ssn_active".into());
        app.sessions = vec![inactive, active];
        app.sidebar_ui_selected_sessions_session_id = Some("ssn_persisted".into());
        app.rebuild_rows();

        assert_eq!(
            app.session_at(app.selected)
                .and_then(|session| session.sessions_session_id.as_deref()),
            Some("ssn_persisted")
        );
    }

    #[test]
    fn rebuild_rows_selects_pending_focus_tab_index() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();
        let first = app
            .rows
            .iter()
            .position(
                |row| matches!(row, RowKind::Session { session } if session.id == "tmux:win:1"),
            )
            .unwrap();
        app.selected = first;
        app.pending_focus_tab_index = Some(2);
        app.rebuild_rows();
        let second = app
            .rows
            .iter()
            .position(
                |row| matches!(row, RowKind::Session { session } if session.id == "tmux:win:2"),
            )
            .unwrap();
        assert_eq!(app.selected, second);
    }

    #[test]
    fn sidebar_ordinal_differs_from_tab_index_with_multiple_groups() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = (1..=6)
            .map(|i| {
                let mut session = sample_session_in_group(
                    &format!("tmux:win:{i}"),
                    i,
                    &format!("thread-{i}"),
                    "~/tmp/a",
                    false,
                );
                let at = Utc::now() - chrono::Duration::minutes(i as i64);
                session.messaged_at = Some(at);
                session.last_event_at = at;
                session
            })
            .chain((15..=16).map(|i| {
                let mut session = sample_session_in_group(
                    &format!("tmux:win:{i}"),
                    i,
                    &format!("other-{i}"),
                    "~/tmp/b",
                    false,
                );
                let at = Utc::now() - chrono::Duration::minutes(20 + i as i64);
                session.messaged_at = Some(at);
                session.last_event_at = at;
                session
            }))
            .collect();
        app.rebuild_rows();

        let target = app
            .rows
            .iter()
            .position(
                |row| matches!(row, RowKind::Session { session } if session.id == "tmux:win:16"),
            )
            .unwrap();
        app.selected = target;

        assert_eq!(app.selected_ordinal(), Some(8));
        assert_eq!(
            app.session_at(app.selected)
                .map(|session| session.tab_index),
            Some(16)
        );
    }

    #[test]
    fn sync_selection_to_active_follows_external_focus_change() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();

        for session in &mut app.sessions {
            session.is_active = session.id == "tmux:win:2";
        }
        app.rebuild_rows();
        app.sync_selection_to_active(true);

        assert_eq!(
            app.session_at(app.selected)
                .map(|session| session.id.as_str()),
            Some("tmux:win:2")
        );
    }

    #[test]
    fn sync_selection_to_active_preserves_non_active_selection() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();

        app.selected = app
            .rows
            .iter()
            .position(
                |row| matches!(row, RowKind::Session { session } if session.id == "tmux:win:2"),
            )
            .unwrap();
        let selected_before = app.selected;

        app.sync_selection_to_active(false);

        assert_eq!(app.selected, selected_before);
        assert_eq!(
            app.session_at(app.selected)
                .map(|session| session.id.as_str()),
            Some("tmux:win:2")
        );
    }

    #[test]
    fn width_only_terminal_resize_avoids_layout_reset() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.render_cache.size = Some((55, 24));
        app.render_cache.scroll = Some(3);
        app.handle_terminal_resize(70, 24);
        assert_eq!(app.render_cache.size, Some((70, 24)));
        assert_eq!(app.render_cache.scroll, Some(3));
    }

    #[test]
    fn height_terminal_resize_clears_layout_cache() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.render_cache.size = Some((55, 24));
        app.render_cache.scroll = Some(3);
        app.handle_terminal_resize(55, 20);
        assert_eq!(app.render_cache.size, None);
        assert_eq!(app.render_cache.scroll, None);
    }

    #[test]
    fn group_drag_preview_keeps_preserved_session_selection() {
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
        assert_eq!(app.selected_ordinal(), Some(1));

        app.group_drag = GroupDragState {
            source: Some("~/a".into()),
            hover: Some("~/b".into()),
            dragged: true,
            preserved_session_id: Some("tmux:win:1".into()),
            ..Default::default()
        };
        app.rebuild_rows();

        assert_eq!(
            app.session_at(app.selected)
                .map(|session| session.id.as_str()),
            Some("tmux:win:1")
        );
        assert_eq!(app.selected_ordinal(), Some(2));
    }

    #[test]
    fn rebuild_rows_repins_selection_after_group_reorder() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        let mut first = sample_session_in_group("tmux:win:1", 1, "one", "~/a", false);
        first.sessions_session_id = Some("ssn-one".into());
        app.sessions = vec![
            first,
            sample_session_in_group("tmux:win:2", 2, "two", "~/b", false),
        ];
        app.group_order = vec!["~/a".into(), "~/b".into()];
        app.folded_groups.clear();
        app.rebuild_rows();

        let selected_session = app.session_row_index("tmux:win:1").unwrap();
        app.set_selected(selected_session);
        app.group_order = vec!["~/b".into(), "~/a".into()];
        let stale_index = app.session_row_index("tmux:win:2").unwrap();
        app.selected = stale_index;

        app.rebuild_rows();

        assert_eq!(
            app.session_at(app.selected)
                .map(|session| session.id.as_str()),
            Some("tmux:win:1")
        );
    }

    #[test]
    fn effective_selected_row_follows_preserved_session_during_drag() {
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
        app.group_drag = GroupDragState {
            source: Some("~/a".into()),
            hover: Some("~/b".into()),
            dragged: true,
            preserved_session_id: Some("tmux:win:1".into()),
            ..Default::default()
        };
        app.rebuild_rows();
        app.selected = 0;

        assert_eq!(
            app.session_at(app.effective_selected_row())
                .map(|session| session.id.as_str()),
            Some("tmux:win:1")
        );
    }

}
