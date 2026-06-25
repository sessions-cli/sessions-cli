use crate::bar::client::{ClientEvent, DaemonClient};
use crate::bar::group_order::{self, SidebarGroupOrder};
use crate::bar::mouse_cursor::{self, MouseCursorShape};
use crate::bar::notepad::{self, Note, SidebarNotepad};
use crate::bar::ui::{self, GroupDragState, NoteDragState, NotepadHit, RowKind, ToolbarAction};
use crate::config::Config;
use crate::model::{AgentState, ServerEvent, Session};
use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::cursor::SetCursorStyle;
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::{HashMap, HashSet};
use std::io;
use std::time::{Duration, Instant};

pub(crate) const DIGIT_JUMP_TIMEOUT: Duration = Duration::from_millis(400);
pub(crate) const NOTEPAD_DOUBLE_CLICK_TIMEOUT: Duration = Duration::from_millis(400);
pub(crate) const POINTER_EXIT_HOVER_CLEAR: Duration = Duration::from_millis(32);
pub(crate) const SIDEBAR_POINTER_CURSOR_HOLD: Duration = Duration::from_millis(500);
/// Press-and-hold before PWD/note drag visuals engage; quick clicks stay click-only.
pub(crate) const DRAG_HOLD_MIN: Duration = Duration::from_millis(150);
pub(crate) const CLIPBOARD_NOTICE_DURATION: Duration = Duration::from_millis(1500);
pub(crate) const TELEMETRY_FLUSH_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub(crate) const NOTEPAD_SAVE_DEBOUNCE: Duration = Duration::from_millis(750);
pub(crate) const SIDEBAR_UI_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
pub(crate) const WORKSPACE_PANEL_PROBE_INTERVAL: Duration = Duration::from_millis(500);
pub(crate) const SIDEBAR_FOCUS_PROBE_INTERVAL: Duration = Duration::from_millis(300);
pub(crate) const SIDEBAR_ENGAGE_THROTTLE: Duration = Duration::from_millis(50);
pub(crate) const AGENTS_WINDOW_PROBE_INTERVAL: Duration = Duration::from_millis(80);

use crate::bar::editor::{self, TextEditor};

mod chrome;
mod close_mode;
mod input;
mod notepad_section;
mod render;
mod render_cache;
mod session_list;

pub(crate) use close_mode::{
    CLOSE_HOLD_MIN_SETTLE, CLOSE_HOLD_MISSED_REPEAT_TOLERANCE, CLOSE_HOLD_RELEASE_SLACK,
    CLOSE_HOLD_REPEAT_LEARN_MIN, CLOSE_HOLD_SILENCE_MIN,
};
pub(crate) use render_cache::RenderCache;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AcknowledgedCompletion {
    thread: String,
    at: DateTime<Utc>,
}

pub(crate) fn is_fresh_unacknowledged_completion(
    session: &Session,
    acknowledged: &HashMap<String, AcknowledgedCompletion>,
) -> bool {
    if !session.thread_is_complete() {
        return false;
    }
    let Some(completed_at) = session.completed_at else {
        return true;
    };
    let Some(thread) = session.completed_thread.as_deref() else {
        return true;
    };
    match acknowledged.get(&session.id) {
        None => true,
        Some(ack) => completed_at > ack.at || thread != ack.thread.as_str(),
    }
}

pub(crate) fn load_update_banner() -> Option<ui::UpdateBannerView> {
    let cfg = crate::telemetry::config::SessionsConfig::load(&crate::paths::home()).ok()?;
    let info = cfg.update_info()?;
    let version = info.available_version?;
    let critical = info.urgency == crate::telemetry::config::UpdateUrgency::Critical;
    let label = if info.message.is_empty() {
        format!("Update available: {version}")
    } else {
        info.message
    };
    Some(ui::UpdateBannerView {
        version,
        label,
        critical,
    })
}

pub struct App {
    config: Config,
    client: DaemonClient,
    sessions: Vec<Session>,
    version: u64,
    rows: Vec<RowKind>,
    selectable: Vec<usize>,
    selected: usize,
    scroll: usize,
    /// When true, the next redraw pins scroll so the selected row stays visible.
    selection_scroll_sync: bool,
    selection_initialized: bool,
    rows_version: u64,
    render_cache: RenderCache,
    last_time_tick: Instant,
    digit_buffer: String,
    digit_deadline: Option<Instant>,
    expanded_groups: HashSet<String>,
    sidebar_ui_selected_sessions_session_id: Option<String>,
    sidebar_ui_save_deadline: Option<Instant>,
    folded_groups: HashSet<String>,
    group_order: Vec<String>,
    group_drag: GroupDragState,
    pending_focus_tab_index: Option<u32>,
    last_tracked_agents_window: Option<u32>,
    last_agents_window_probe: Instant,
    /// Completions the user visited — keyed by completion timestamp + thread title.
    acknowledged_completions: HashMap<String, AcknowledgedCompletion>,
    close_modifier_held: bool,
    /// When true, delete mode ignores hold-d silence auto-exit (reserved; hold-d uses false).
    close_mode_latched: bool,
    d_key_down: bool,
    d_last_active: Option<Instant>,
    d_seen_repeat: bool,
    d_repeat_gap: Option<Duration>,
    d_release_pending: bool,
    close_hold_engaged_at: Option<Instant>,
    anim_frame: usize,
    last_anim_tick: Instant,
    hover_row: Option<usize>,
    close_target_row: Option<usize>,
    group_hover_row: Option<usize>,
    /// After a PWD drag-drop, ignore list hovers until the pointer leaves the release row.
    suppress_list_hover_after_group_drag: bool,
    suppress_list_hover_y: Option<u16>,
    toolbar_hover: Option<ToolbarAction>,
    coming_soon_anims: HashMap<ToolbarAction, Instant>,
    settings_hover: bool,
    leave_hover: bool,
    workspace_settings_open: bool,
    workspace_new_session_open: bool,
    last_workspace_panel_probe: Instant,
    sidebar_pane_focused: bool,
    last_workspace_pane_focused: bool,
    last_sidebar_focus_probe: Instant,
    last_sidebar_engage: Instant,
    pointer_near_exit: bool,
    last_mouse_activity: Instant,
    last_mouse: Option<MouseEvent>,
    pointer_hover_refresh_pending: bool,
    last_synced_mouse_cursor: Option<MouseCursorShape>,
    context_menu: Option<ui::ContextMenu>,
    rename: Option<ui::RenameState>,
    delete_note_confirm: Option<ui::DeleteNoteConfirmState>,
    sessions_expanded: bool,
    notepad_welcome_seeded: bool,
    notepad_expanded: bool,
    notes_list_expanded: bool,
    notes: Vec<Note>,
    notes_preview: Vec<Note>,
    note_drag: NoteDragState,
    active_note_id: Option<String>,
    notepad_editor: TextEditor,
    notepad_focused: bool,
    notepad_section_header_hover: bool,
    notepad_section_add_hover: bool,
    notepad_note_hover: Option<usize>,
    notepad_last_click: Option<(Instant, u16, u16, u8)>,
    sessions_title_hover: bool,
    sessions_title_add_hover: bool,
    notepad_scroll_pending: bool,
    clipboard_notice_until: Option<Instant>,
    clipboard_notice_text: Option<String>,
    list_select_anchor: Option<ui::ListTextPoint>,
    list_select_head: Option<ui::ListTextPoint>,
    list_text_selecting: bool,
    user_pane_width: Option<u16>,
    update_banner: Option<ui::UpdateBannerView>,
    update_upgrade_hover: bool,
    update_dismiss_hover: bool,
    last_telemetry_flush: Instant,
    /// When a debounced note-body save should flush to disk.
    notepad_save_deadline: Option<Instant>,
    notepad_last_saved_at: Option<DateTime<Utc>>,
}

impl App {
    pub fn new(config: &Config) -> Result<Self> {
        let client = DaemonClient::from_config(config);
        let group_prefs = group_order::load(config);
        let folded_groups = group_order::load_folded(config);
        let sidebar_ui = crate::bar::sidebar_ui::load(config);
        let expanded_groups: HashSet<String> = sidebar_ui.expanded_groups.into_iter().collect();
        let notepad_prefs = notepad::load(config);
        let active_text = notepad_prefs
            .active_note()
            .map(|note| note.text.as_str())
            .unwrap_or("");
        let notepad_editor = TextEditor::for_text(active_text);
        let group_order = group_prefs.groups;
        let app = Self {
            config: config.clone(),
            client,
            sessions: Vec::new(),
            version: 0,
            rows: Vec::new(),
            selectable: Vec::new(),
            selected: 0,
            scroll: 0,
            selection_scroll_sync: false,
            selection_initialized: false,
            rows_version: 0,
            render_cache: RenderCache::default(),
            last_time_tick: Instant::now(),
            digit_buffer: String::new(),
            digit_deadline: None,
            expanded_groups,
            sidebar_ui_selected_sessions_session_id: sidebar_ui.selected_sessions_session_id,
            sidebar_ui_save_deadline: None,
            folded_groups,
            group_order,
            group_drag: GroupDragState::default(),
            pending_focus_tab_index: None,
            last_tracked_agents_window: None,
            last_agents_window_probe: Instant::now(),
            acknowledged_completions: HashMap::new(),
            close_modifier_held: false,
            close_mode_latched: false,
            d_key_down: false,
            d_last_active: None,
            d_seen_repeat: false,
            d_repeat_gap: None,
            d_release_pending: false,
            close_hold_engaged_at: None,
            anim_frame: 0,
            last_anim_tick: Instant::now(),
            hover_row: None,
            close_target_row: None,
            group_hover_row: None,
            suppress_list_hover_after_group_drag: false,
            suppress_list_hover_y: None,
            toolbar_hover: None,
            coming_soon_anims: HashMap::new(),
            settings_hover: false,
            leave_hover: false,
            workspace_settings_open: false,
            workspace_new_session_open: false,
            last_workspace_panel_probe: Instant::now()
                .checked_sub(WORKSPACE_PANEL_PROBE_INTERVAL)
                .unwrap_or_else(Instant::now),
            sidebar_pane_focused: true,
            last_workspace_pane_focused: false,
            last_sidebar_focus_probe: Instant::now(),
            last_sidebar_engage: Instant::now()
                .checked_sub(SIDEBAR_ENGAGE_THROTTLE)
                .unwrap_or_else(Instant::now),
            pointer_near_exit: false,
            last_mouse_activity: Instant::now(),
            last_mouse: None,
            pointer_hover_refresh_pending: false,
            last_synced_mouse_cursor: None,
            context_menu: None,
            rename: None,
            delete_note_confirm: None,
            sessions_expanded: notepad_prefs.sessions_expanded,
            notepad_welcome_seeded: notepad_prefs.welcome_seeded,
            notepad_expanded: notepad_prefs.expanded,
            notes_list_expanded: notepad_prefs.notes_list_expanded,
            notes: notepad_prefs.notes,
            notes_preview: Vec::new(),
            note_drag: NoteDragState::default(),
            active_note_id: notepad_prefs.active_note_id,
            notepad_editor,
            notepad_focused: false,
            notepad_section_header_hover: false,
            notepad_section_add_hover: false,
            notepad_note_hover: None,
            notepad_last_click: None,
            sessions_title_hover: false,
            sessions_title_add_hover: false,
            notepad_scroll_pending: false,
            clipboard_notice_until: None,
            clipboard_notice_text: None,
            list_select_anchor: None,
            list_select_head: None,
            list_text_selecting: false,
            user_pane_width: crate::daemon::tmux::current_pane_width(),
            update_banner: load_update_banner(),
            update_upgrade_hover: false,
            update_dismiss_hover: false,
            last_telemetry_flush: Instant::now(),
            notepad_save_deadline: None,
            notepad_last_saved_at: notepad::last_saved_at(config),
        };
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        crate::telemetry::record_feature(
            crate::telemetry::FeatureId::SidebarAttach,
            crate::telemetry::feature::Source::Cli,
        );
        let _ = crate::daemon::server::ensure_daemon_running(&self.config);
        let events = self.client.subscribe()?;
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            EnableMouseCapture,
            EnableFocusChange,
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            ),
        )?;
        let _ = crate::daemon::tmux::write_host_terminal_backdrop();
        crate::daemon::tmux::enable_pane_graphics_passthrough(None);
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        self.ensure_sidebar_width();
        self.sync_workspace_panel_state(true);
        self.last_synced_mouse_cursor = None;
        self.sync_sidebar_mouse_cursor(None);

        let result = self.event_loop(&mut terminal, &events);
        if crate::telemetry::counters::save_pending_to_file(&self.config.home).unwrap_or(false) {
            self.client.telemetry_flush_async();
        }
        self.flush_notepad_save_pending();
        self.flush_sidebar_ui_save_pending();
        let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
        let _ = mouse_cursor::reset_mouse_cursor();
        let _ = execute!(io::stdout(), SetCursorStyle::DefaultUserShape);
        crossterm::execute!(
            io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            DisableFocusChange,
            crossterm::terminal::LeaveAlternateScreen
        )?;
        crossterm::terminal::disable_raw_mode()?;
        result
    }

    pub(crate) fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        events: &crate::bar::client::EventReceiver,
    ) -> Result<()> {
        loop {
            self.drain_client_events(events);

            self.flush_digit_buffer_if_due();
            self.expire_coming_soon_anims_if_due();
            self.expire_clipboard_notice_if_due();
            let engaged = self.refresh_close_hold_state();
            if engaged {
                let size = terminal.size()?;
                let metrics = self.layout_metrics(size);
                self.seed_close_hover(&metrics);
            }
            self.advance_anim_frame();
            self.maybe_flush_telemetry();
            self.flush_notepad_save_if_due();
            self.flush_sidebar_ui_save_if_due();

            let mut poll_timeout = if self.close_modifier_held {
                Duration::from_millis(8)
            } else if self.needs_continuous_animation() {
                Duration::from_millis(self.animation_interval_ms())
            } else {
                Duration::from_millis(50)
            };
            if let Some(remaining) = self.notepad_save_poll_cap() {
                poll_timeout = poll_timeout.min(remaining);
            }
            if let Some(remaining) = self.sidebar_ui_save_poll_cap() {
                poll_timeout = poll_timeout.min(remaining);
            }
            if let Some(remaining) = self.drag_hold_poll_cap() {
                poll_timeout = poll_timeout.min(remaining);
            }
            let close_before = self.close_modifier_held;
            if event::poll(poll_timeout)? {
                loop {
                    let event = event::read()?;
                    let keep_draining =
                        matches!(&event, Event::Key(key) if key.code == KeyCode::Char('d'));
                    self.handle_event(&event, terminal)?;
                    self.coalesce_mouse_moves(terminal)?;
                    if !keep_draining || !event::poll(Duration::from_millis(0))? {
                        break;
                    }
                }
                if self.refresh_close_hold_state() {
                    let size = terminal.size()?;
                    let metrics = self.layout_metrics(size);
                    self.seed_close_hover(&metrics);
                }
                self.redraw_if_needed(terminal)?;
            } else {
                self.maybe_engage_pending_drag_from_hold(terminal)?;
            }
            if self.refresh_close_hold_state() {
                let size = terminal.size()?;
                let metrics = self.layout_metrics(size);
                self.seed_close_hover(&metrics);
            }
            if close_before && !self.close_modifier_held {
                self.redraw_if_needed(terminal)?;
            }
            self.sync_workspace_panel_state(false);
            self.sync_sidebar_pane_focus();
            self.sync_external_active_window();
            let size = terminal.size()?;
            let metrics = self.layout_metrics(size);
            if self.pointer_hover_refresh_pending {
                self.refresh_pointer_hover_from_mouse(&metrics);
                self.pointer_hover_refresh_pending = false;
            }
            self.clear_stale_pointer_hover_after_exit(&metrics);
            self.redraw_if_needed(terminal)?;
            self.sync_sidebar_mouse_cursor(Some(&metrics));
        }
    }
}

pub(crate) fn parse_digit_ordinal(buffer: &str) -> Option<u32> {
    if buffer.is_empty() {
        return None;
    }
    if buffer == "0" {
        return Some(10);
    }
    let value: u32 = buffer.parse().ok()?;
    (value > 0).then_some(value)
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    use super::*;
    use crate::model::{AgentState, Session};
    use chrono::Utc;

    pub(crate) fn sample_session(id: &str, tab_index: u32, description: &str, active: bool) -> Session {
        Session {
            id: id.into(),
            kitty_window_id: tab_index as u64,
            kitty_tab_id: 0,
            kitty_os_window_id: 0,
            tab_index,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: format!("app · {description}"),
            description: description.into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now()),
            prompt_submitted: true,
            title_manual: false,
            is_active: active,
            last_event_at: Utc::now(),
            ..Default::default()
        }
    }

    pub(crate) fn sample_session_in_group(
        id: &str,
        tab_index: u32,
        description: &str,
        cwd_label: &str,
        active: bool,
    ) -> Session {
        let mut session = sample_session(id, tab_index, description, active);
        session.cwd_label = cwd_label.into();
        session
    }

    pub(crate) fn completed_session(id: &str, tab_index: u32, description: &str) -> Session {
        let mut session = sample_session(id, tab_index, description, false);
        session.state = AgentState::Done;
        session.completed_thread = Some(description.into());
        session.completed_at = Some(Utc::now());
        session
    }
}
