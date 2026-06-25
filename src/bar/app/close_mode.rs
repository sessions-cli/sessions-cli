use super::App;
use crate::bar::ui::{self, RowKind};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEventKind, MouseEvent};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::{Duration, Instant};

/// Minimum gap between `d` repeats before we treat silence as a release.
pub(crate) const CLOSE_HOLD_SILENCE_MIN: Duration = Duration::from_millis(18);
/// After key-repeat starts, treat silence as release after this many missed repeat cycles.
pub(crate) const CLOSE_HOLD_MISSED_REPEAT_TOLERANCE: u32 = 2;
/// Slack added on top of the tolerated silence window.
pub(crate) const CLOSE_HOLD_RELEASE_SLACK: Duration = Duration::from_millis(16);
/// Ignore duplicate press/release pairs right after engage.
pub(crate) const CLOSE_HOLD_MIN_SETTLE: Duration = Duration::from_millis(120);
/// Minimum gap before treating a second `d` press as key-repeat (not a spurious duplicate).
pub(crate) const CLOSE_HOLD_REPEAT_LEARN_MIN: Duration = Duration::from_millis(120);

impl App {
    pub(crate) fn close_hold_silence_limit(&self) -> Duration {
        let gap = self
            .d_repeat_gap
            .unwrap_or(Duration::from_millis(40))
            .max(CLOSE_HOLD_SILENCE_MIN);
        gap.saturating_mul(CLOSE_HOLD_MISSED_REPEAT_TOLERANCE)
            .saturating_add(CLOSE_HOLD_RELEASE_SLACK)
    }
    pub(crate) fn close_mode_live(&self) -> bool {
        if !self.close_modifier_held {
            return false;
        }
        let Some(last) = self.d_last_active else {
            return false;
        };
        last.elapsed() <= self.close_hold_silence_limit()
    }
    pub(crate) fn clear_close_mode(&mut self) {
        self.disengage_close_mode();
    }
    pub(crate) fn note_d_key_activity(&mut self, from_repeat: bool) {
        let now = Instant::now();
        if let Some(last) = self.d_last_active {
            let gap = now.duration_since(last);
            let learn_gap = if from_repeat {
                gap >= Duration::from_millis(8)
            } else {
                gap >= CLOSE_HOLD_REPEAT_LEARN_MIN
            };
            if learn_gap && gap <= Duration::from_millis(500) {
                self.d_seen_repeat = true;
                self.d_repeat_gap = Some(match self.d_repeat_gap {
                    Some(prev) => {
                        let ms = ((prev.as_millis() as f64) * 0.4 + (gap.as_millis() as f64) * 0.6)
                            as u64;
                        Duration::from_millis(
                            ms.max(CLOSE_HOLD_REPEAT_LEARN_MIN.as_millis() as u64),
                        )
                    }
                    None => gap,
                });
            }
        } else if !self.d_key_down {
            self.d_seen_repeat = false;
            self.d_repeat_gap = None;
        }
        self.d_last_active = Some(now);
    }
    pub(crate) fn close_hold_can_disengage(&self) -> bool {
        self.close_hold_engaged_at
            .is_some_and(|engaged| engaged.elapsed() >= CLOSE_HOLD_MIN_SETTLE)
    }
    pub(crate) fn handle_close_mode_d_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<bool> {
        if key.code != KeyCode::Char('d') {
            return Ok(false);
        }
        match key.kind {
            KeyEventKind::Press => {
                self.d_key_down = true;
                self.d_release_pending = false;
                self.d_seen_repeat = true;
                self.note_d_key_activity(false);
                let entering = self.engage_close_mode_from_hold(terminal)?;
                Ok(entering || self.close_modifier_held)
            }
            KeyEventKind::Repeat => {
                self.d_key_down = true;
                self.d_release_pending = false;
                self.d_seen_repeat = true;
                self.note_d_key_activity(true);
                if self.close_modifier_held {
                    self.touch_close_hold();
                } else {
                    let _ = self.engage_close_mode_from_hold(terminal)?;
                }
                Ok(true)
            }
            KeyEventKind::Release => {
                self.d_key_down = false;
                if self.close_modifier_held {
                    if self.close_hold_can_disengage() {
                        self.disengage_close_mode();
                    } else {
                        self.d_release_pending = true;
                    }
                } else {
                    self.d_last_active = None;
                    self.d_seen_repeat = false;
                    self.d_repeat_gap = None;
                    self.d_release_pending = false;
                }
                Ok(true)
            }
        }
    }
    pub(crate) fn handle_notepad_d_for_close_mode(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<bool> {
        if key.code != KeyCode::Char('d') {
            return Ok(true);
        }
        match key.kind {
            KeyEventKind::Press if self.close_modifier_held => {
                self.d_key_down = true;
                self.d_release_pending = false;
                self.note_d_key_activity(false);
                self.touch_close_hold();
                Ok(false)
            }
            KeyEventKind::Repeat => {
                self.unfocus_notepad();
                self.handle_close_mode_d_key(key, terminal).map(|_| false)
            }
            KeyEventKind::Release => self.handle_close_mode_d_key(key, terminal).map(|_| false),
            _ => Ok(true),
        }
    }
    pub(crate) fn engage_close_mode_from_hold(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<bool> {
        if self.close_modifier_held {
            return Ok(false);
        }
        self.close_modifier_held = true;
        self.close_mode_latched = false;
        self.close_hold_engaged_at = Some(Instant::now());
        self.d_release_pending = false;
        let size = Self::terminal_size(terminal);
        let metrics = self.layout_metrics(size);
        self.seed_close_hover(&metrics);
        #[cfg(not(test))]
        self.redraw_if_needed(terminal)?;
        Ok(true)
    }
    pub(crate) fn disengage_close_mode(&mut self) {
        if !self.close_modifier_held {
            self.d_key_down = false;
            return;
        }
        self.close_modifier_held = false;
        self.close_mode_latched = false;
        self.d_key_down = false;
        self.d_last_active = None;
        self.d_seen_repeat = false;
        self.d_repeat_gap = None;
        self.d_release_pending = false;
        self.close_hold_engaged_at = None;
        self.close_target_row = None;
    }
    pub(crate) fn refresh_close_hold_state(&mut self) -> bool {
        if self.close_mode_latched {
            return false;
        }
        if !self.close_modifier_held {
            return false;
        }
        if !self.close_hold_can_disengage() {
            return false;
        }
        if self.d_release_pending {
            self.disengage_close_mode();
            return false;
        }
        let Some(last) = self.d_last_active else {
            self.disengage_close_mode();
            return false;
        };
        // Terminals often omit key-release events; treat silence after the last
        // d press/repeat as release. Pointer hover must not extend this window.
        if self.d_key_down || self.d_repeat_gap.is_none() {
            return false;
        }
        if last.elapsed() >= self.close_hold_silence_limit() {
            self.disengage_close_mode();
        }
        false
    }
    pub(crate) fn touch_close_hold(&mut self) {
        if self.close_modifier_held && self.d_key_down {
            self.d_last_active = Some(Instant::now());
        }
    }
    pub(crate) fn session_row_under_mouse(
        &self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) -> Option<usize> {
        if !ui::pointer_in_list_body(mouse.column, metrics) {
            return None;
        }
        ui::row_from_mouse(
            mouse.row,
            metrics.list_top_y,
            metrics.list_height,
            self.scroll,
            self.rows.len(),
        )
        .filter(|&row_idx| {
            matches!(
                self.rows.get(row_idx),
                Some(RowKind::Session { .. } | RowKind::GroupToggle { .. })
            )
        })
    }
    pub(crate) fn close_session_under_mouse(
        &self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) -> Option<usize> {
        self.session_row_under_mouse(mouse, metrics)
            .filter(|&row_idx| matches!(self.rows.get(row_idx), Some(RowKind::Session { .. })))
    }
    pub(crate) fn close_note_under_mouse(
        &self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) -> Option<usize> {
        if !ui::pointer_in_list_body(mouse.column, metrics) {
            return None;
        }
        let hit = ui::notepad_hit_from_mouse(
            mouse.column,
            mouse.row,
            metrics,
            self.scroll,
            self.sidebar_trail_base(),
            &self.notepad_list_state(),
        )?;
        match hit {
            ui::NotepadHit::NoteTitle { note_index } => ui::notepad_note_title_row_index(
                note_index,
                self.sidebar_trail_base(),
                &self.notepad_list_state(),
            ),
            _ => None,
        }
    }
    pub(crate) fn row_is_close_target_note(&self, row_idx: usize) -> bool {
        let size = self
            .render_cache
            .size
            .map(|(width, height)| ratatui::layout::Size::new(width, height))
            .unwrap_or(ratatui::layout::Size::new(ui::DEFAULT_PANE_WIDTH, 24));
        let metrics = self.layout_metrics(size);
        ui::note_close_target_row(
            row_idx,
            self.sidebar_trail_base(),
            self.close_modifier_held,
            self.close_target_row,
            &self.notepad_list_state(),
            metrics.list_line_width,
        )
    }
    pub(crate) fn active_note_title_row(&self) -> Option<usize> {
        let note_index = self.active_note_index()?;
        ui::notepad_note_title_row_index(
            note_index,
            self.sidebar_trail_base(),
            &self.notepad_list_state(),
        )
    }
    pub(crate) fn seed_close_hover(&mut self, metrics: &ui::LayoutMetrics) {
        if let Some(mouse) = self.last_mouse {
            if let Some(row) = self.close_session_under_mouse(&mouse, metrics) {
                self.close_target_row = Some(row);
                return;
            }
            if let Some(row) = self.close_note_under_mouse(&mouse, metrics) {
                self.close_target_row = Some(row);
                return;
            }
        }
        if matches!(self.rows.get(self.selected), Some(RowKind::Session { .. })) {
            self.close_target_row = Some(self.selected);
            return;
        }
        if let Some(row) = self.selectable.iter().copied().find(|&row| {
            self.session_at(row)
                .is_some_and(|session| session.is_active)
        }) {
            self.close_target_row = Some(row);
            return;
        }
        if let Some(row) = self
            .selectable
            .iter()
            .copied()
            .find(|&row| matches!(self.rows.get(row), Some(RowKind::Session { .. })))
        {
            self.close_target_row = Some(row);
            return;
        }
        if let Some(row) = self.active_note_title_row() {
            self.close_target_row = Some(row);
        }
    }
    pub(crate) fn update_close_target_hover(
        &mut self,
        mouse: &MouseEvent,
        metrics: &ui::LayoutMetrics,
    ) {
        if !self.close_modifier_held {
            return;
        }
        if let Some(row) = self.close_session_under_mouse(mouse, metrics) {
            if self.close_target_row != Some(row) {
                self.close_target_row = Some(row);
            }
            return;
        }
        if let Some(row) = self.close_note_under_mouse(mouse, metrics) {
            if self.close_target_row != Some(row) {
                self.close_target_row = Some(row);
            }
        }
    }
    pub(crate) fn reconcile_close_hover(&mut self, metrics: Option<&ui::LayoutMetrics>) {
        if !self.close_modifier_held {
            return;
        }
        if let (Some(metrics), Some(mouse)) = (metrics, self.last_mouse.as_ref()) {
            if let Some(row) = self.close_session_under_mouse(mouse, metrics) {
                self.close_target_row = Some(row);
                return;
            }
            if let Some(row) = self.close_note_under_mouse(mouse, metrics) {
                self.close_target_row = Some(row);
                return;
            }
        }
        let line_width = metrics
            .map(|metrics| metrics.list_line_width)
            .unwrap_or(ui::DEFAULT_PANE_WIDTH as usize);
        let target_still_valid = self.close_target_row.is_some_and(|row| {
            matches!(self.rows.get(row), Some(RowKind::Session { .. }))
                || ui::row_is_note_title(
                    row,
                    self.sidebar_trail_base(),
                    &self.notepad_list_state(),
                    line_width,
                )
        });
        if target_still_valid {
            return;
        }
        if matches!(self.rows.get(self.selected), Some(RowKind::Session { .. })) {
            self.close_target_row = Some(self.selected);
            return;
        }
        if let Some(row) = self
            .selectable
            .iter()
            .copied()
            .find(|&row| matches!(self.rows.get(row), Some(RowKind::Session { .. })))
        {
            self.close_target_row = Some(row);
            return;
        }
        self.close_target_row = self.active_note_title_row();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bar::app::App;
    use crate::config::Config;
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::backend::CrosstermBackend;
    use ratatui::Terminal;
    use std::io;

    pub(crate) fn engage_close_mode_for_test(app: &mut App) {
        app.close_modifier_held = true;
        app.close_mode_latched = false;
        app.d_last_active = Some(Instant::now());
        app.close_hold_engaged_at =
            Some(Instant::now() - CLOSE_HOLD_MIN_SETTLE - Duration::from_millis(1));
    }

    pub(crate) fn engage_close_mode_hold_for_test(app: &mut App) {
        app.close_modifier_held = true;
        app.close_mode_latched = false;
        app.d_last_active = Some(Instant::now());
        app.close_hold_engaged_at =
            Some(Instant::now() - CLOSE_HOLD_MIN_SETTLE - Duration::from_millis(1));
    }

    #[test]
    fn close_mode_stays_engaged_while_d_activity_is_recent() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_for_test(&mut app);
        app.d_key_down = true;
        app.d_last_active = Some(Instant::now());
        app.refresh_close_hold_state();
        assert!(app.close_modifier_held);
    }

    #[test]
    fn notepad_hold_d_repeat_unfocuses_and_engages_close_mode() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.notepad_focused = true;
        app.notepad_expanded = true;
        let key = crossterm::event::KeyEvent::new_with_kind(
            KeyCode::Char('d'),
            KeyModifiers::empty(),
            KeyEventKind::Repeat,
        );
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout())).expect("terminal");
        assert!(!app
            .handle_notepad_d_for_close_mode(key, &mut terminal)
            .unwrap());
        assert!(!app.notepad_focused);
        assert!(app.close_modifier_held);
    }

    #[test]
    fn latched_close_mode_ignores_hold_silence_timeout() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_for_test(&mut app);
        app.close_mode_latched = true;
        app.d_key_down = false;
        app.d_seen_repeat = true;
        app.d_repeat_gap = Some(Duration::from_millis(33));
        app.d_last_active =
            Some(Instant::now() - app.close_hold_silence_limit() - Duration::from_millis(1));
        app.refresh_close_hold_state();
        assert!(app.close_modifier_held);
        assert!(app.close_mode_latched);
    }

    #[test]
    fn close_mode_disengages_after_d_silence_even_if_release_missing() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_hold_for_test(&mut app);
        app.d_key_down = false;
        app.d_seen_repeat = true;
        app.d_repeat_gap = Some(Duration::from_millis(33));
        app.d_last_active =
            Some(Instant::now() - app.close_hold_silence_limit() - Duration::from_millis(1));
        app.refresh_close_hold_state();
        assert!(!app.close_modifier_held);
        assert!(!app.d_key_down);
    }

    #[test]
    fn close_mode_survives_initial_hold_before_key_repeat() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_hold_for_test(&mut app);
        app.d_repeat_gap = None;
        app.d_last_active = Some(Instant::now() - Duration::from_millis(120));
        app.refresh_close_hold_state();
        assert!(app.close_modifier_held);
    }

    #[test]
    fn pending_release_waits_for_settle_before_disengage() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.close_modifier_held = true;
        app.d_release_pending = true;
        app.close_hold_engaged_at = Some(Instant::now());
        app.refresh_close_hold_state();
        assert!(app.close_modifier_held);
        assert!(app.d_release_pending);

        app.close_hold_engaged_at =
            Some(Instant::now() - CLOSE_HOLD_MIN_SETTLE - Duration::from_millis(1));
        app.refresh_close_hold_state();
        assert!(!app.close_modifier_held);
        assert!(!app.d_release_pending);
    }

    #[test]
    fn pending_release_disengages_after_settle_without_repeat_gap() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_hold_for_test(&mut app);
        app.d_repeat_gap = None;
        app.d_release_pending = true;
        app.d_last_active = Some(Instant::now() - Duration::from_millis(120));
        app.refresh_close_hold_state();
        assert!(!app.close_modifier_held);
    }

    #[test]
    fn repeat_clears_pending_release_and_keeps_close_mode() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_hold_for_test(&mut app);
        app.d_release_pending = true;
        app.d_repeat_gap = None;
        app.d_key_down = true;
        app.d_last_active = Some(Instant::now() - Duration::from_millis(40));
        app.d_release_pending = false;
        app.note_d_key_activity(true);
        app.refresh_close_hold_state();
        assert!(app.close_modifier_held);
        assert!(!app.d_release_pending);
        assert!(app.d_repeat_gap.is_some());
    }

    #[test]
    fn first_press_while_holding_keeps_repeat_tracking() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.d_key_down = true;
        app.d_seen_repeat = true;
        app.note_d_key_activity(false);
        assert!(app.d_seen_repeat);
        assert!(app.d_repeat_gap.is_none());
    }

    #[test]
    fn close_mode_live_extends_while_repeat_is_recent() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_for_test(&mut app);
        app.d_seen_repeat = true;
        app.d_repeat_gap = Some(Duration::from_millis(33));
        app.d_last_active = Some(Instant::now());
        assert!(app.close_mode_live());
    }

    #[test]
    fn close_mode_survives_single_missed_repeat_cycle() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_for_test(&mut app);
        app.d_key_down = true;
        app.d_seen_repeat = true;
        app.d_repeat_gap = Some(Duration::from_millis(33));
        app.d_last_active = Some(Instant::now() - Duration::from_millis(40));
        app.refresh_close_hold_state();
        assert!(app.close_modifier_held);
        assert!(app.close_mode_live());
    }

    #[test]
    fn close_mode_live_expires_after_sustained_silence() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_hold_for_test(&mut app);
        app.d_seen_repeat = true;
        app.d_repeat_gap = Some(Duration::from_millis(30));
        app.d_last_active =
            Some(Instant::now() - app.close_hold_silence_limit() - Duration::from_millis(1));
        app.refresh_close_hold_state();
        assert!(!app.close_modifier_held);
        assert!(!app.close_mode_live());
    }

    #[test]
    fn repeated_press_events_keep_close_mode_engaged() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.note_d_key_activity(false);
        app.close_modifier_held = true;
        app.d_last_active = Some(Instant::now() - Duration::from_millis(150));
        app.note_d_key_activity(false);
        assert!(app.close_modifier_held);
        assert!(app.d_seen_repeat);
        assert!(app.d_repeat_gap.is_some());
    }

    #[test]
    fn duplicate_press_does_not_learn_repeat_gap() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.note_d_key_activity(false);
        app.d_last_active = Some(Instant::now() - Duration::from_millis(40));
        app.note_d_key_activity(false);
        assert!(app.d_repeat_gap.is_none());
    }

    #[test]
    fn release_with_repeat_gap_disengages_after_settle() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_hold_for_test(&mut app);
        app.d_repeat_gap = Some(Duration::from_millis(33));
        app.d_release_pending = true;
        app.d_last_active = Some(Instant::now() - Duration::from_millis(10));
        app.refresh_close_hold_state();
        assert!(!app.close_modifier_held);
        assert!(!app.d_release_pending);
    }

    #[test]
    fn pointer_hover_does_not_extend_close_hold_after_d_release() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_hold_for_test(&mut app);
        app.d_key_down = false;
        app.d_seen_repeat = true;
        app.d_repeat_gap = Some(Duration::from_millis(33));
        app.d_last_active =
            Some(Instant::now() - app.close_hold_silence_limit() - Duration::from_millis(1));
        app.touch_close_hold();
        app.refresh_close_hold_state();
        assert!(!app.close_modifier_held);
    }

    #[test]
    fn tap_release_clears_pending_hold_without_engaging_close_mode() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.note_d_key_activity(false);
        assert!(!app.close_modifier_held);
        app.d_last_active = None;
        app.d_seen_repeat = false;
        app.d_repeat_gap = None;
        app.refresh_close_hold_state();
        assert!(!app.close_modifier_held);
    }

    #[test]
    fn close_mode_survives_two_missed_repeat_cycles() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        engage_close_mode_hold_for_test(&mut app);
        app.d_seen_repeat = true;
        app.d_repeat_gap = Some(Duration::from_millis(33));
        app.d_last_active = Some(Instant::now() - Duration::from_millis(70));
        app.refresh_close_hold_state();
        assert!(app.close_modifier_held);
        assert!(app.close_mode_live());
    }

    #[test]
    fn pointer_hover_keeps_close_target_when_sidebar_loses_focus_in_close_mode() {
        use crate::bar::ui::ToolbarAction;
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.close_modifier_held = true;
        app.close_target_row = Some(2);
        app.group_hover_row = Some(1);
        app.toolbar_hover = Some(ToolbarAction::NewSession);
        app.settings_hover = true;

        app.clear_pointer_hover_states();

        assert_eq!(app.close_target_row, Some(2));
        assert_eq!(app.group_hover_row, None);
        assert_eq!(app.toolbar_hover, None);
        assert!(!app.settings_hover);
    }

    #[test]
    fn close_target_stays_when_pointer_leaves_list() {
        use crate::bar::app::test_fixtures::sample_session;
        use crate::bar::ui::{self, RowKind};
        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", false),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();
        app.close_modifier_held = true;
        let first = app
            .rows
            .iter()
            .position(|row| matches!(row, RowKind::Session { .. }))
            .unwrap();
        app.close_target_row = Some(first);
        let metrics = ui::layout_metrics(ratatui::layout::Size::new(40, 20), &app.rows);
        let over_gap = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 1,
            row: metrics.list_top_y.saturating_sub(1),
            modifiers: KeyModifiers::empty(),
        };
        app.update_close_target_hover(&over_gap, &metrics);
        assert_eq!(app.close_target_row, Some(first));
    }

    #[test]
    #[test]
    fn seed_close_hover_falls_back_to_active_note_title() {
        use crate::bar::notepad::Note;
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        let note = Note::new("Scratch", "body", false);
        let note_id = note.id.clone();
        app.notes = vec![note];
        app.active_note_id = Some(note_id);
        app.notepad_expanded = true;
        app.notes_list_expanded = true;
        app.sessions = Vec::new();
        app.rebuild_rows();
        let metrics = ui::layout_metrics_with_notepad(
            ratatui::layout::Size::new(40, 30),
            &app.rows,
            false,
            &app.notes,
            true,
            true,
        );
        app.seed_close_hover(&metrics);
        let expected = ui::notepad_note_title_row_index(
            0,
            app.sidebar_trail_base(),
            &app.notepad_list_state(),
        );
        assert_eq!(app.close_target_row, expected);
    }

    fn reconcile_close_hover_falls_back_to_selected_session() {
        use crate::bar::app::test_fixtures::sample_session;
        use crate::bar::ui::RowKind;
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", false),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();
        app.close_modifier_held = true;
        app.close_target_row = Some(999);
        app.reconcile_close_hover(None);
        assert_eq!(
            app.close_target_row,
            app.rows
                .iter()
                .position(|row| matches!(row, RowKind::Session { .. }))
        );
    }
}