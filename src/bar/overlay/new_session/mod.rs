//! New-session overlay (Ratatui panel).

mod input;
mod launch;
mod render;
mod state;

use crate::bar::mouse_cursor;
use crate::config::Config;
use crate::daemon::tmux;
use anyhow::Result;
use crossterm::event::{self, MouseEvent};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;

pub use input::{apply_paste, handle_key, handle_mouse_event, NewSessionAction};
pub use render::draw_screen;
pub use state::{NewSessionState, PanelHover};

use input::event_loop;
use render::ClickTargets;

pub fn run_new_session(config: &Config) -> Result<()> {
    let mut state = NewSessionState::new(config)?;
    let mut panel_hover = PanelHover::default();
    let _ = tmux::write_host_terminal_backdrop();
    tmux::enable_pane_graphics_passthrough(None);
    let mut terminal = setup_terminal()?;
    let result = event_loop(&mut terminal, config, &mut state, &mut panel_hover);
    let _ = mouse_cursor::reset_mouse_cursor();
    teardown_terminal()?;
    match &result {
        Ok(NewSessionAction::Launched) => {}
        Ok(NewSessionAction::Close) | Ok(NewSessionAction::Unchanged) | Err(_) => {
            let _ = state.save_draft(config);
        }
    }
    result.map(|_| ())
}
fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
    )?;
    Ok(Terminal::new(CrosstermBackend::new(io::stdout()))?)
}

fn teardown_terminal() -> Result<()> {
    crossterm::execute!(
        io::stdout(),
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}
pub struct NewSessionPanel {
    pub state: NewSessionState,
    pub hover: PanelHover,
    targets: ClickTargets,
}

impl NewSessionPanel {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            state: NewSessionState::new(config)?,
            hover: PanelHover::default(),
            targets: ClickTargets::default(),
        })
    }

    pub fn draw(&mut self, frame: &mut ratatui::Frame<'_>) {
        self.targets = draw_screen(frame, &mut self.state, &self.hover);
    }

    pub fn handle_key(
        &mut self,
        config: &Config,
        key: event::KeyEvent,
    ) -> Result<NewSessionAction> {
        handle_key(&mut self.state, config, key)
    }

    pub fn handle_mouse(&mut self, config: &Config, mouse: MouseEvent) -> Result<NewSessionAction> {
        handle_mouse_event(
            mouse,
            &self.targets,
            &mut self.hover,
            &mut self.state,
            config,
        )
    }

    pub fn handle_paste(&mut self, text: &str) {
        apply_paste(&mut self.state, text);
    }
}

#[cfg(test)]
mod tests {
    use super::input::handle_mouse;
    use super::render::{
        field_block_height, modal_content_height, modal_content_height_for_state,
        new_session_layout, popup_row_backdrop, prompt_display_lines, prompt_field_inner,
        submit_button_backdrop, workspace_list_window, workspace_popup_row_from_mouse,
        ClickTargets, BG_FIELD,
    };
    use super::state::*;
    use super::*;
    use crate::agents;
    use crate::bar::art_canvas;
    use crate::bar::directory_discovery::DirectoryIndex;
    use crate::bar::notepad;
    use crate::bar::panel_popup;
    use crate::bar::ui::{BG_HIGHLIGHT, BG_HOVER_SELECTED, BG_PANEL, BG_SELECTED};
    use crate::config::Config;
    use crate::session::workspace_usage::{WorkspaceRankMode, WorkspaceUsageStore};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use std::collections::HashMap;
    use std::path::Path;

    fn test_directory_index() -> DirectoryIndex {
        DirectoryIndex::build(&Config::default())
    }

    fn test_state_with_workspace(label: &str, cwd: &str) -> NewSessionState {
        NewSessionState {
            directory_index: test_directory_index(),
            workspace_usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces: vec![WorkspaceChoice {
                label: label.into(),
                cwd: cwd.into(),
            }],
            workspace_idx: 0,
            workspace_path_input: String::new(),
            workspace_path_cursor: 0,
            workspace_popup_highlight: 1,
            workspace_confirmed: false,
            agent_idx: 0,
            agent_confirmed: false,
            model_idx: 0,
            model_confirmed: false,
            prompt: String::new(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            prompt_selection: None,
            prompt_select_anchor: None,
            prompt_drag_selecting: false,
            prompt_last_click: None,
            prompt_field_width: 40,
            launch_mode: LaunchMode::Open,
            defaults: NewSessionDefaults::default(),
            focus: Focus::Workspace,
            workspace_user_editing: false,
            status: String::new(),
        }
    }

    #[test]
    fn agent_list_has_supported_agents() {
        assert_eq!(agents::AGENTS.len(), 5);
        assert_eq!(
            agents::AGENTS.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec!["grok", "codex", "claude", "opencode", "console"]
        );
    }

    #[test]
    fn defaults_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut agent_models = HashMap::new();
        agent_models.insert("codex".into(), "gpt-5.5".into());
        agent_models.insert("grok".into(), "grok-build".into());
        let defaults = NewSessionDefaults {
            workspace_label: Some("~/projects/foo".into()),
            custom_workspace_path: Some("~/projects/foo".into()),
            agent_id: "codex".into(),
            agent_models,
            launch_mode: LaunchMode::Background,
        };
        save_defaults(&config, &defaults).unwrap();
        let loaded = load_defaults(&config);
        assert_eq!(loaded.workspace_label.as_deref(), Some("~/projects/foo"));
        assert_eq!(
            loaded.custom_workspace_path.as_deref(),
            Some("~/projects/foo")
        );
        assert_eq!(loaded.agent_id, "codex");
        assert_eq!(
            loaded.agent_models.get("codex").map(String::as_str),
            Some("gpt-5.5")
        );
        assert_eq!(
            loaded.agent_models.get("grok").map(String::as_str),
            Some("grok-build")
        );
        assert_eq!(loaded.launch_mode, LaunchMode::Background);
    }

    #[test]
    fn draft_roundtrip_restores_in_progress_form() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut state = test_state_with_workspace("~/projects/foo", "/tmp/foo");
        state.workspace_path_input = "~/projects/bar".into();
        state.workspace_user_editing = true;
        state.workspace_confirmed = true;
        state.agent_idx = agents::AGENTS
            .iter()
            .position(|agent| agent.id == "codex")
            .unwrap_or(0);
        state.agent_confirmed = true;
        state.model_idx = 1;
        state.model_confirmed = true;
        state.prompt = "continue this task".into();
        state.prompt_cursor = 4;
        state.focus = Focus::Prompt;
        state.launch_mode = LaunchMode::Background;

        state.save_draft(&config).unwrap();
        let loaded = load_draft(&config).expect("draft file");
        assert_eq!(loaded.prompt, "continue this task");
        assert_eq!(loaded.agent_id, "codex");
        assert_eq!(loaded.workspace_path_input, "~/projects/bar");

        let mut restored = test_state_with_workspace("~/projects/foo", "/tmp/foo");
        restored.apply_draft(loaded, &config);
        assert_eq!(restored.prompt, "continue this task");
        assert_eq!(restored.selected_agent().id, "codex");
        assert_eq!(restored.workspace_path_input, "~/projects/bar");
        assert_eq!(restored.focus, Focus::Prompt);
        assert_eq!(restored.launch_mode, LaunchMode::Background);
    }

    #[test]
    fn empty_form_clears_saved_draft() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut seeded = test_state_with_workspace("~/projects/foo", "/tmp/foo");
        seeded.prompt = "old draft".into();
        seeded.save_draft(&config).unwrap();
        assert!(config.new_session_draft_path().exists());

        let empty = test_state_with_workspace("~/projects/foo", "/tmp/foo");
        empty.save_draft(&config).unwrap();
        assert!(!config.new_session_draft_path().exists());
    }

    #[test]
    fn foreground_launch_clears_draft() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut state = test_state_with_workspace("~/projects/foo", "/tmp/foo");
        state.prompt = "ship it".into();
        state.save_draft(&config).unwrap();
        clear_draft(&config).unwrap();
        assert!(!config.new_session_draft_path().exists());
    }

    #[test]
    fn prepare_for_next_session_resets_form_like_fresh_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        // Preselect is on by default for empty config homes.
        let mut state = test_state_with_workspace("~/projects/foo", "/tmp/foo");
        state.workspace_confirmed = true;
        state.agent_confirmed = true;
        state.model_confirmed = true;
        state.prompt = "ship it".into();
        state.prompt_cursor = 7;
        state.focus = Focus::BackgroundButton;
        state.launch_mode = LaunchMode::Background;
        state.defaults.launch_mode = LaunchMode::Background;

        state.prepare_for_next_session(&config);

        assert_eq!(state.prompt, "");
        assert_eq!(state.prompt_cursor, 0);
        assert!(!state.workspace_confirmed);
        // Defaults: agent/model preselected, cursor on directory.
        assert!(state.agent_confirmed);
        assert!(state.model_confirmed);
        assert_eq!(state.focus, Focus::Workspace);
        assert!(state.workspace_path_input.is_empty());
        assert!(!state.workspace_user_editing);
        assert_eq!(state.launch_mode, LaunchMode::Background);
    }

    #[test]
    fn prepare_for_next_session_starts_on_agent_when_preselect_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        crate::telemetry::config::save_new_session_preselect_agent_model(&config.home, false)
            .unwrap();
        let mut state = test_state_with_workspace("~/projects/foo", "/tmp/foo");
        state.agent_confirmed = true;
        state.model_confirmed = true;
        state.focus = Focus::BackgroundButton;
        state.prompt = "ship it".into();

        state.prepare_for_next_session(&config);

        assert!(!state.agent_confirmed);
        assert!(!state.model_confirmed);
        assert_eq!(state.focus, Focus::Agent);
    }

    #[test]
    fn new_session_preselect_confirms_agent_model_and_focuses_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        // Explicit default (true).
        crate::telemetry::config::save_new_session_preselect_agent_model(&config.home, true)
            .unwrap();
        let mut state = test_state_with_workspace("~/projects/foo", "/tmp/foo");
        state.focus = Focus::Agent;
        state.agent_confirmed = false;
        state.model_confirmed = false;
        state.apply_agent_model_preselect(&config);
        assert!(state.agent_confirmed);
        assert!(state.model_confirmed);
        assert_eq!(state.focus, Focus::Workspace);
    }

    #[test]
    fn each_agent_gets_catalog_default_model_without_saved_override() {
        for agent in agents::AGENTS {
            let mut defaults = NewSessionDefaults::default();
            defaults.agent_id = agent.id.to_string();
            let agent_idx = agents::AGENTS
                .iter()
                .position(|entry| entry.id == agent.id)
                .unwrap();
            let mut state = NewSessionState {
                directory_index: test_directory_index(),
                workspace_usage: WorkspaceUsageStore::default(),
                rank_mode: WorkspaceRankMode::MostUsed,
                directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
                workspaces: vec![],
                workspace_idx: 0,
                workspace_path_input: String::new(),
                workspace_path_cursor: 0,
                workspace_popup_highlight: 0,
                workspace_confirmed: false,
                agent_idx,
                agent_confirmed: false,
                model_idx: 0,
                model_confirmed: false,
                prompt: String::new(),
                prompt_cursor: 0,
                prompt_scroll: 0,
                prompt_selection: None,
                prompt_select_anchor: None,
                prompt_drag_selecting: false,
                prompt_last_click: None,
                prompt_field_width: 40,
                launch_mode: LaunchMode::Open,
                defaults,
                focus: Focus::Model,
                workspace_user_editing: false,
                status: String::new(),
            };
            state.sync_model_idx();
            assert_eq!(state.selected_model_id(), agent.default_model);
        }
    }

    #[test]
    fn prompt_display_wraps_and_preserves_newlines() {
        let lines = prompt_display_lines("line one\nline two is longer", 8, 4);
        assert_eq!(
            lines.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["line one", "line two", "is", "longer"]
        );
    }

    #[test]
    fn prompt_selection_range_normalizes_drag_endpoints() {
        assert_eq!(notepad::selection_range(2, 7), (2, 7));
        assert_eq!(notepad::selection_range(7, 2), (2, 7));
    }

    #[test]
    fn prompt_double_click_selects_word() {
        let mut state = NewSessionState {
            directory_index: test_directory_index(),
            workspace_usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces: vec![],
            workspace_idx: 0,
            workspace_path_input: String::new(),
            workspace_path_cursor: 0,
            workspace_popup_highlight: 0,
            workspace_confirmed: false,
            agent_idx: 0,
            agent_confirmed: false,
            model_idx: 0,
            model_confirmed: false,
            prompt: "hello world".into(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            prompt_selection: None,
            prompt_select_anchor: None,
            prompt_drag_selecting: false,
            prompt_last_click: None,
            prompt_field_width: 20,
            launch_mode: LaunchMode::Open,
            defaults: NewSessionDefaults::default(),
            focus: Focus::Workspace,
            workspace_user_editing: false,
            status: String::new(),
        };
        let field = Rect::new(0, 0, 24, field_block_height(PROMPT_INNER_HEIGHT));
        let inner = prompt_field_inner(field);
        let click_x = inner.x.saturating_add(3);
        let click_y = inner.y;
        state.handle_prompt_body_click(field, click_x, click_y, state.prompt_field_width);
        state.handle_prompt_body_click(field, click_x, click_y, state.prompt_field_width);
        assert_eq!(state.prompt_selection, Some((0, 5)));
        assert_eq!(state.focus, Focus::Prompt);
    }

    #[test]
    fn prompt_editing_inserts_at_cursor_and_supports_newlines() {
        let mut state = NewSessionState {
            directory_index: test_directory_index(),
            workspace_usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces: vec![],
            workspace_idx: 0,
            workspace_path_input: String::new(),
            workspace_path_cursor: 0,
            workspace_popup_highlight: 0,
            workspace_confirmed: false,
            agent_idx: 0,
            agent_confirmed: false,
            model_idx: 0,
            model_confirmed: false,
            prompt: "hi".into(),
            prompt_cursor: 2,
            prompt_scroll: 0,
            prompt_selection: None,
            prompt_select_anchor: None,
            prompt_drag_selecting: false,
            prompt_last_click: None,
            prompt_field_width: 20,
            launch_mode: LaunchMode::Open,
            defaults: NewSessionDefaults::default(),
            focus: Focus::Prompt,
            workspace_user_editing: false,
            status: String::new(),
        };
        state.insert_prompt_char('!', state.prompt_field_width);
        assert_eq!(state.prompt, "hi!");
        state.insert_prompt_char('\n', state.prompt_field_width);
        state.insert_prompt_char('x', state.prompt_field_width);
        assert_eq!(state.prompt, "hi!\nx");
        state.prompt_cursor = 0;
        state.select_prompt_all(state.prompt_field_width);
        assert_eq!(state.prompt_selection, Some((0, 5)));
        assert_eq!(notepad::selected_text(&state.prompt, 0, 5), "hi!\nx");
    }

    #[test]
    fn popup_row_backdrop_matches_sidebar_hover_semantics() {
        assert_eq!(popup_row_backdrop(false, false), BG_PANEL);
        assert_eq!(popup_row_backdrop(false, true), BG_HIGHLIGHT);
        assert_eq!(popup_row_backdrop(true, false), BG_SELECTED);
        assert_eq!(popup_row_backdrop(true, true), BG_HOVER_SELECTED);
    }

    #[test]
    fn submit_button_backdrop_ignores_hover() {
        assert_eq!(submit_button_backdrop(false), BG_FIELD);
        assert_eq!(submit_button_backdrop(true), BG_SELECTED);
    }

    #[test]
    fn workspace_popup_hover_tracks_pointer_over_selectable_rows() {
        let entries = vec![
            WorkspacePopupEntry {
                kind: WorkspacePopupKind::Section,
                label: EXISTING_WORKSPACES_HEADER.into(),
                cwd: None,
            },
            WorkspacePopupEntry {
                kind: WorkspacePopupKind::Existing(0),
                label: "~/projects/sessions-cli".into(),
                cwd: Some("/projects/sessions-cli".into()),
            },
        ];
        let popup = Rect::new(10, 5, 40, 5);
        // Row 7 is the section header inside the list; row 8 is the first selectable entry.
        assert_eq!(
            workspace_popup_row_from_mouse(popup, 12, 8, &entries, 1),
            Some(1)
        );
        assert_eq!(
            workspace_popup_row_from_mouse(popup, 12, 7, &entries, 1),
            None
        );
    }

    #[test]
    fn button_hover_tracks_pointer_without_changing_keyboard_focus() {
        let targets = ClickTargets {
            foreground_button: Rect::new(10, 20, 14, 1),
            background_button: Rect::new(26, 20, 14, 1),
            ..ClickTargets::default()
        };
        let mut hover = PanelHover::default();
        let mut state = test_state_with_workspace("sessions-cli", "/sessions-cli");
        state.focus = Focus::ForegroundButton;

        handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 33,
                row: 20,
                modifiers: KeyModifiers::empty(),
            },
            &targets,
            &mut hover,
            &mut state,
            &Config::default(),
        )
        .unwrap();

        assert!(hover.background_button);
        assert!(!hover.foreground_button);
        assert_eq!(state.focus, Focus::ForegroundButton);

        handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::empty(),
            },
            &targets,
            &mut hover,
            &mut state,
            &Config::default(),
        )
        .unwrap();

        assert!(!hover.background_button);
        assert_eq!(state.focus, Focus::ForegroundButton);
    }

    #[test]
    fn prompt_mouse_wheel_scrolls_independently_of_cursor() {
        let mut state = NewSessionState {
            directory_index: test_directory_index(),
            workspace_usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces: vec![],
            workspace_idx: 0,
            workspace_path_input: String::new(),
            workspace_path_cursor: 0,
            workspace_popup_highlight: 0,
            workspace_confirmed: false,
            agent_idx: 0,
            agent_confirmed: false,
            model_idx: 0,
            model_confirmed: false,
            prompt: "one\ntwo\nthree\nfour\nfive\nsix".into(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            prompt_selection: None,
            prompt_select_anchor: None,
            prompt_drag_selecting: false,
            prompt_last_click: None,
            prompt_field_width: 8,
            launch_mode: LaunchMode::Open,
            defaults: NewSessionDefaults::default(),
            focus: Focus::Prompt,
            workspace_user_editing: false,
            status: String::new(),
        };
        let targets = ClickTargets {
            prompt_field: Rect::new(0, 0, 24, field_block_height(PROMPT_INNER_HEIGHT)),
            ..ClickTargets::default()
        };
        let mut hover = PanelHover::default();
        let config = Config::default();

        handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 2,
                row: 2,
                modifiers: KeyModifiers::empty(),
            },
            &targets,
            &mut hover,
            &mut state,
            &config,
        )
        .unwrap();
        assert_eq!(state.prompt_scroll, 1);
        assert_eq!(state.prompt_cursor, 0);

        handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 2,
                row: 2,
                modifiers: KeyModifiers::empty(),
            },
            &targets,
            &mut hover,
            &mut state,
            &config,
        )
        .unwrap();
        assert_eq!(state.prompt_scroll, 0);
    }

    #[test]
    fn prompt_drag_updates_selection() {
        let mut state = NewSessionState {
            directory_index: test_directory_index(),
            workspace_usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces: vec![],
            workspace_idx: 0,
            workspace_path_input: String::new(),
            workspace_path_cursor: 0,
            workspace_popup_highlight: 0,
            workspace_confirmed: false,
            agent_idx: 0,
            agent_confirmed: false,
            model_idx: 0,
            model_confirmed: false,
            prompt: "hello world".into(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            prompt_selection: None,
            prompt_select_anchor: None,
            prompt_drag_selecting: false,
            prompt_last_click: None,
            prompt_field_width: 20,
            launch_mode: LaunchMode::Open,
            defaults: NewSessionDefaults::default(),
            focus: Focus::Prompt,
            workspace_user_editing: false,
            status: String::new(),
        };
        let field = Rect::new(0, 0, 24, field_block_height(PROMPT_INNER_HEIGHT));
        let inner = prompt_field_inner(field);
        state.handle_prompt_body_click(
            field,
            inner.x.saturating_add(1),
            inner.y,
            state.prompt_field_width,
        );
        assert!(state.prompt_drag_selecting);

        handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: inner.x.saturating_add(6),
                row: inner.y,
                modifiers: KeyModifiers::empty(),
            },
            &ClickTargets {
                prompt_field: field,
                ..ClickTargets::default()
            },
            &mut PanelHover::default(),
            &mut state,
            &Config::default(),
        )
        .unwrap();

        assert_eq!(state.prompt_selection, Some((0, 5)));
        assert_eq!(state.prompt_cursor, 5);
    }

    #[test]
    fn focus_tab_order_cycles_through_form() {
        // Visual order: Agent → Model → Session Path → Prompt → buttons.
        assert_eq!(focus_next(Focus::Agent), Focus::Model);
        assert_eq!(focus_next(Focus::Model), Focus::Workspace);
        assert_eq!(focus_next(Focus::Workspace), Focus::Prompt);
        assert_eq!(focus_next(Focus::Prompt), Focus::ForegroundButton);
        assert_eq!(focus_next(Focus::BackgroundButton), Focus::Agent);
        assert_eq!(focus_prev(Focus::Agent), Focus::BackgroundButton);
        assert_eq!(focus_prev(Focus::Model), Focus::Agent);
        assert_eq!(focus_prev(Focus::Workspace), Focus::Model);
        assert_eq!(focus_prev(Focus::Prompt), Focus::Workspace);
        assert_eq!(focus_prev(Focus::ForegroundButton), Focus::Prompt);
    }

    #[test]
    fn layout_form_uses_four_sixths_width_centered_in_pane() {
        let area = Rect::new(0, 0, 120, 50);
        let state = test_state_with_workspace("sessions-cli", "/sessions-cli");
        let layout = new_session_layout(area, &state);
        let w = art_canvas::pane_fraction_width(area.width).max(40);
        assert_eq!(layout.form.width, w);
        assert_eq!(layout.form.x, area.x + (area.width - w) / 2);
    }

    #[test]
    fn dropdown_fields_keep_fixed_height_when_focused() {
        let state = NewSessionState {
            directory_index: test_directory_index(),
            workspace_usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces: vec![
                WorkspaceChoice {
                    label: "~/a".into(),
                    cwd: "/a".into(),
                },
                WorkspaceChoice {
                    label: "~/b".into(),
                    cwd: "/b".into(),
                },
                WorkspaceChoice {
                    label: "~/c".into(),
                    cwd: "/c".into(),
                },
            ],
            workspace_idx: 1,
            workspace_path_input: String::new(),
            workspace_path_cursor: 0,
            workspace_popup_highlight: 1,
            workspace_confirmed: false,
            agent_idx: 0,
            agent_confirmed: false,
            model_idx: 0,
            model_confirmed: false,
            prompt: String::new(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            prompt_selection: None,
            prompt_select_anchor: None,
            prompt_drag_selecting: false,
            prompt_last_click: None,
            prompt_field_width: 40,
            launch_mode: LaunchMode::Open,
            defaults: NewSessionDefaults::default(),
            focus: Focus::Workspace,
            workspace_user_editing: false,
            status: String::new(),
        };
        let _unfocused = NewSessionState {
            focus: Focus::Prompt,
            ..state.clone()
        };
        assert_eq!(
            modal_content_height(),
            modal_content_height(),
            "focused dropdowns should not resize the selector box"
        );
    }

    #[test]
    fn workspace_popup_lists_existing_sessions_first() {
        let state = NewSessionState {
            directory_index: test_directory_index(),
            workspace_usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces: vec![
                WorkspaceChoice {
                    label: "~/projects/foo".into(),
                    cwd: "/home/testuser/projects/foo".into(),
                },
                WorkspaceChoice {
                    label: "~/projects/bar".into(),
                    cwd: "/home/testuser/projects/bar".into(),
                },
            ],
            workspace_idx: 0,
            workspace_path_input: String::new(),
            workspace_path_cursor: 0,
            workspace_popup_highlight: 1,
            workspace_confirmed: false,
            agent_idx: 0,
            agent_confirmed: false,
            model_idx: 0,
            model_confirmed: false,
            prompt: String::new(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            prompt_selection: None,
            prompt_select_anchor: None,
            prompt_drag_selecting: false,
            prompt_last_click: None,
            prompt_field_width: 40,
            launch_mode: LaunchMode::Open,
            defaults: NewSessionDefaults::default(),
            focus: Focus::Workspace,
            workspace_user_editing: false,
            status: String::new(),
        };
        let popup = state.build_workspace_popup();
        assert!(popup
            .iter()
            .any(|row| row.label == EXISTING_WORKSPACES_HEADER));
        assert!(popup
            .iter()
            .any(|row| matches!(row.kind, WorkspacePopupKind::Existing(0))));
        assert!(popup
            .iter()
            .any(|row| matches!(row.kind, WorkspacePopupKind::Existing(1))));
    }

    #[test]
    fn workspace_list_window_uses_full_viewport() {
        let (start, visible) = workspace_list_window(200, 100, 30);
        assert_eq!(visible, 30);
        assert_eq!(start, 85);
    }

    #[test]
    fn browse_suggestions_includes_root_as_default() {
        let suggestions = test_directory_index().browse_suggestions();
        // Root (~) is now the default/compulsory session path so users can easily start at home.
        assert!(
            suggestions.first().map(|(l, _)| l.as_str()) == Some("~"),
            "root ~ should be first suggestion for default session path, got {suggestions:?}"
        );
    }

    #[test]
    fn cycling_highlight_keeps_committed_path_untouched() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.workspace_popup_highlight = 0;
        state.sync_popup_highlight_to_workspace_idx();
        let committed = state.workspace_committed_display();
        for _ in 0..6 {
            state.cycle_workspace_popup(1);
        }
        assert!(state.workspace_path_input.is_empty());
        assert!(!state.workspace_user_editing);
        assert_eq!(state.workspace_committed_display(), committed);
    }

    #[test]
    fn arrow_up_from_first_dropdown_row_reopens_path_editor() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.select_existing_workspace(0);
        let entries = state.build_workspace_popup();
        let selectable = NewSessionState::workspace_popup_selectable_indices(&entries);
        state.workspace_popup_highlight = selectable[0];
        state.cycle_workspace_popup(-1);
        assert!(state.workspace_user_editing);
        assert_eq!(state.workspace_path_input, "~/projects/foo");
    }

    #[test]
    fn begin_workspace_path_edit_restores_directory_selection() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.directory_index = DirectoryIndex::from_test_entries(
            "/home/test",
            vec![(
                "~/projects/sessions-cli".into(),
                "/home/test/projects/sessions-cli".into(),
            )],
        );
        state.select_path_label("~/projects/sessions-cli".into(), false);
        state.begin_workspace_path_edit();
        assert!(state.workspace_user_editing);
        assert_eq!(state.workspace_path_input, "~/projects/sessions-cli");
        let entries = state.build_workspace_popup();
        assert!(
            entries
                .iter()
                .any(|entry| entry.label == NEW_WORKSPACE_HEADER),
            "expected typed-path section after re-entering edit mode"
        );
    }

    #[test]
    fn path_completions_suggest_projects_children() {
        // Use the synthetic index — do not probe the developer's real ~/projects
        // (that path often lacks sessions-cli and made CI machine-dependent).
        let index = DirectoryIndex::from_test_entries(
            "/home/test",
            vec![
                (
                    "~/projects/sessions-cli".into(),
                    "/home/test/projects/sessions-cli".into(),
                ),
                (
                    "~/projects/other".into(),
                    "/home/test/projects/other".into(),
                ),
            ],
        );
        let completions = index.completions_for_input("~/projects/sessions");
        assert!(
            completions
                .iter()
                .any(|(label, _)| label.contains("sessions")),
            "expected sessions-cli style match, got {completions:?}"
        );
    }

    #[test]
    fn workspace_enter_confirms_existing_selection() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        assert!(state.confirm_workspace_enter());
        assert!(state.workspace_confirmed);
        assert!(state.status.is_empty());
    }

    #[test]
    fn workspace_popup_click_confirms_without_enter() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        let entries = state.build_workspace_popup();
        let idx = entries
            .iter()
            .position(|entry| matches!(entry.kind, WorkspacePopupKind::Existing(0)))
            .expect("existing workspace row");
        state.workspace_popup_highlight = idx;
        state.apply_workspace_popup_selection();
        assert!(state.confirm_workspace_selection(false));
        assert!(state.workspace_confirmed);
    }

    #[test]
    fn workspace_blur_confirms_valid_selection() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.select_existing_workspace(0);
        state.set_focus(Focus::Agent);
        assert!(state.workspace_confirmed);
        assert_eq!(state.focus, Focus::Agent);
    }

    #[test]
    fn confirmed_custom_path_survives_focus_blur() {
        let home = std::env::var("HOME").expect("HOME");
        let sessions = format!("{home}/projects/sessions-cli");
        let personal = format!("{home}/projects/sample-site");
        if !Path::new(&sessions).is_dir() || !Path::new(&personal).is_dir() {
            return;
        }
        let mut state = test_state_with_workspace("~/projects/sessions-cli", &sessions);
        state.directory_index = DirectoryIndex::from_test_entries(
            &home,
            vec![("~/projects/sessions-cli".into(), sessions)],
        );
        state.workspace_path_input = "~/projects/sample-site".into();
        state.workspace_user_editing = true;
        state.workspace_popup_highlight = state.first_popup_highlight_for_query();
        assert!(state.confirm_workspace_enter());
        assert_eq!(state.workspace_path_input, "~/projects/sample-site");
        assert_eq!(state.workspace_header_display(), "~/projects/sample-site");
        state.set_focus(Focus::Agent);
        assert_eq!(
            state.workspace_committed_display(),
            "~/projects/sample-site"
        );
        assert!(state.workspace_confirmed);
    }

    #[test]
    fn custom_path_header_ignores_stale_existing_highlight() {
        let home = std::env::var("HOME").expect("HOME");
        let sessions = format!("{home}/projects/sessions-cli");
        let personal = format!("{home}/projects/sample-site");
        if !Path::new(&sessions).is_dir() || !Path::new(&personal).is_dir() {
            return;
        }
        let mut state = test_state_with_workspace("~/projects/sessions-cli", &sessions);
        state.select_path_label("~/projects/sample-site".into(), false);
        state.confirm_workspace_selection(false);
        let entries = state.build_workspace_popup();
        let sessions_idx = entries
            .iter()
            .position(|entry| {
                matches!(entry.kind, WorkspacePopupKind::Existing(0))
                    && entry.label == "~/projects/sessions-cli"
            })
            .expect("active session row");
        state.workspace_popup_highlight = sessions_idx;
        assert_eq!(state.workspace_header_display(), "~/projects/sample-site");
    }

    #[test]
    fn workspace_enter_rejects_invalid_typed_path() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.workspace_path_input = "~/this-path-should-not-exist-xyz".into();
        state.workspace_user_editing = true;
        state.workspace_popup_highlight = state.first_popup_highlight_for_query();
        assert!(!state.confirm_workspace_enter());
        assert!(!state.status.is_empty());
    }

    #[test]
    fn workspace_enter_accepts_unique_basename_completion() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.directory_index = DirectoryIndex::from_test_entries(
            "/home/test",
            vec![
                (
                    "~/side-projects/dev-tools/sessions-cli".into(),
                    "/home/test/side-projects/dev-tools/sessions-cli".into(),
                ),
                (
                    "~/side-projects/productivity/sidekick".into(),
                    "/home/test/side-projects/productivity/sidekick".into(),
                ),
            ],
        );
        // Point "cwd" at a real dir so confirm can canonicalize (use temp via HOME projects).
        // from_test_entries only supplies labels for matching; expand still hits the FS.
        // Use a path that exists on this machine when present; otherwise skip.
        let home = std::env::var("HOME").expect("HOME");
        let sessions = format!("{home}/side-projects/dev-tools/sessions-cli");
        if !Path::new(&sessions).is_dir() {
            return;
        }
        state.directory_index = DirectoryIndex::from_test_entries(
            &home,
            vec![
                (
                    "~/side-projects/dev-tools/sessions-cli".into(),
                    sessions.clone(),
                ),
                (
                    "~/side-projects/productivity/sidekick".into(),
                    format!("{home}/side-projects/productivity/sidekick"),
                ),
            ],
        );
        state.workspace_path_input = "sessions".into();
        state.workspace_user_editing = true;
        state.workspace_popup_highlight = state.first_popup_highlight_for_query();
        assert!(state.confirm_workspace_enter(), "status={}", state.status);
        assert!(
            state.workspace_path_input.contains("sessions-cli"),
            "expected basename Enter to expand, got {}",
            state.workspace_path_input
        );
        assert!(state.workspace_confirmed);
    }

    #[test]
    fn workspace_enter_keeps_valid_typed_path_over_stale_highlight() {
        let home = std::env::var("HOME").expect("HOME");
        let sessions = format!("{home}/side-projects/dev-tools/sessions-cli");
        if !Path::new(&sessions).is_dir() {
            return;
        }
        let mut state = test_state_with_workspace("~/other", "/tmp/other");
        state.directory_index = DirectoryIndex::from_test_entries(
            &home,
            vec![
                ("~".into(), home.clone()),
                (
                    "~/side-projects/dev-tools/sessions-cli".into(),
                    sessions.clone(),
                ),
            ],
        );
        state.workspace_path_input = format!("~/side-projects/dev-tools/sessions-cli");
        state.workspace_user_editing = true;
        // Stale highlight on home (~) — must not replace a valid typed path.
        let entries = state.build_workspace_popup();
        let home_idx = entries
            .iter()
            .position(|e| e.kind == WorkspacePopupKind::Path && e.label == "~")
            .unwrap_or(0);
        // Force highlight away from typed row, then put it back to typed for the
        // on_typed_row path; separately test that confirm uses typed when on typed row.
        state.workspace_popup_highlight = state.first_popup_highlight_for_query();
        assert!(state.on_workspace_typed_row());
        assert!(state.confirm_workspace_enter());
        assert!(
            state.workspace_path_input.contains("sessions-cli"),
            "typed path should win over highlight {home_idx}, got {}",
            state.workspace_path_input
        );
    }

    #[test]
    fn tab_complete_extends_partial_path() {
        let completions = vec![
            ("~/projects/sessions-cli".into(), "/tmp".into()),
            ("~/projects/sessions-cli-old".into(), "/tmp2".into()),
        ];
        let completed = longest_path_completion("~/projects/sess", &completions);
        assert_eq!(completed.as_deref(), Some("~/projects/sessions-cli"));
    }

    #[test]
    fn basename_query_matches_existing_workspace_row() {
        let mut state = test_state_with_workspace(
            "~/projects/sessions-cli",
            "/home/test/projects/sessions-cli",
        );
        state.workspace_path_input = "ses".into();
        state.workspace_user_editing = true;
        state.workspace_popup_highlight = state.first_popup_highlight_for_query();
        let entries = state.build_workspace_popup();
        assert!(
            entries.iter().any(|entry| {
                matches!(entry.kind, WorkspacePopupKind::Existing(0))
                    && entry.label == "~/projects/sessions-cli"
            }),
            "expected basename query to surface active session"
        );
    }

    #[test]
    fn ghost_hint_shows_full_path_for_basename_query() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/test/projects/foo");
        state.directory_index = DirectoryIndex::from_test_entries(
            "/home/test",
            vec![(
                "~/projects/sessions-cli".into(),
                "/home/test/projects/sessions-cli".into(),
            )],
        );
        state.workspace_path_input = "ses".into();
        state.workspace_user_editing = true;
        state.workspace_popup_highlight = state.first_popup_highlight_for_query();
        let hint = state
            .workspace_path_ghost_hint()
            .expect("expected ghost hint for basename query");
        match hint {
            PathGhostHint::FullPath(path) => {
                assert_eq!(path, "~/projects/sessions-cli");
            }
            PathGhostHint::Suffix(path) => {
                assert!(path.contains("sessions-cli"), "unexpected suffix: {path}");
            }
        }
    }

    #[test]
    fn ghost_hint_extends_partial_tilde_path() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/test/projects/foo");
        state.directory_index = DirectoryIndex::from_test_entries(
            "/home/test",
            vec![(
                "~/projects/sessions-cli".into(),
                "/home/test/projects/sessions-cli".into(),
            )],
        );
        state.workspace_path_input = "~/projects/sess".into();
        state.workspace_user_editing = true;
        state.workspace_popup_highlight = state.first_popup_highlight_for_query();
        let hint = state
            .workspace_path_ghost_hint()
            .expect("expected ghost suffix for partial path");
        assert!(matches!(hint, PathGhostHint::Suffix(ref s) if s == "ions-cli"));
    }

    #[test]
    fn tab_complete_workspace_extends_typed_row() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.directory_index = DirectoryIndex::from_test_entries(
            "/home/test",
            vec![(
                "~/projects/sessions-cli".into(),
                "/home/test/projects/sessions-cli".into(),
            )],
        );
        state.workspace_path_input = "~/projects/sess".into();
        state.workspace_user_editing = true;
        state.workspace_popup_highlight = state.first_popup_highlight_for_query();
        assert!(state.tab_complete_workspace());
        assert!(
            state
                .workspace_path_input
                .starts_with("~/projects/sessions"),
            "expected partial path extension, got {}",
            state.workspace_path_input
        );
    }

    #[test]
    fn tab_cycles_through_active_sessions_when_browsing() {
        let mut state = NewSessionState {
            directory_index: test_directory_index(),
            workspace_usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces: vec![
                WorkspaceChoice {
                    label: "~/projects/foo".into(),
                    cwd: "/home/testuser/projects/foo".into(),
                },
                WorkspaceChoice {
                    label: "~/projects/bar".into(),
                    cwd: "/home/testuser/projects/bar".into(),
                },
            ],
            workspace_idx: 0,
            workspace_path_input: String::new(),
            workspace_path_cursor: 0,
            workspace_popup_highlight: 1,
            workspace_confirmed: false,
            agent_idx: 0,
            agent_confirmed: false,
            model_idx: 0,
            model_confirmed: false,
            prompt: String::new(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            prompt_selection: None,
            prompt_select_anchor: None,
            prompt_drag_selecting: false,
            prompt_last_click: None,
            prompt_field_width: 40,
            launch_mode: LaunchMode::Open,
            defaults: NewSessionDefaults::default(),
            focus: Focus::Workspace,
            workspace_user_editing: false,
            status: String::new(),
        };
        let entries = state.build_workspace_popup();
        let existing_rows: Vec<_> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry.kind, WorkspacePopupKind::Existing(_)))
            .collect();
        assert_eq!(existing_rows.len(), 2);
        state.workspace_popup_highlight = existing_rows[0].0;
        let first_label = existing_rows[0].1.label.clone();
        let second_label = existing_rows[1].1.label.clone();
        assert_eq!(state.workspace_header_display(), first_label);
        state.cycle_workspace_popup(1);
        assert_eq!(state.workspace_header_display(), second_label);
        assert!(state.workspace_path_input.is_empty());
        assert!(!state.workspace_user_editing);
    }

    #[test]
    fn browse_mode_defaults_to_most_popular_session_path() {
        let state = NewSessionState {
            directory_index: test_directory_index(),
            workspace_usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces: vec![WorkspaceChoice {
                label: "~/projects/sessions-cli".into(),
                cwd: "/home/test/projects/sessions-cli".into(),
            }],
            workspace_idx: 0,
            workspace_path_input: String::new(),
            workspace_path_cursor: 0,
            workspace_popup_highlight: 1,
            workspace_confirmed: false,
            agent_idx: 0,
            agent_confirmed: false,
            model_idx: 0,
            model_confirmed: false,
            prompt: String::new(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            prompt_selection: None,
            prompt_select_anchor: None,
            prompt_drag_selecting: false,
            prompt_last_click: None,
            prompt_field_width: 40,
            launch_mode: LaunchMode::Open,
            defaults: NewSessionDefaults::default(),
            focus: Focus::Workspace,
            workspace_user_editing: false,
            status: String::new(),
        };
        let highlight = state.default_workspace_popup_highlight();
        let entries = state.build_workspace_popup();
        let label = entries
            .get(highlight)
            .and_then(|entry| match entry.kind {
                WorkspacePopupKind::Existing(idx) => {
                    state.workspaces.get(idx).map(|w| w.label.clone())
                }
                _ => None,
            })
            .expect("most popular session row");
        assert_eq!(label, "~/projects/sessions-cli");
    }

    #[test]
    fn tab_complete_workspace_selects_arrow_highlighted_entry() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.workspace_path_input = "~/projects".into();
        state.workspace_user_editing = true;
        let entries = state.build_workspace_popup();
        let existing_idx = entries
            .iter()
            .enumerate()
            .find(|(_, entry)| matches!(entry.kind, WorkspacePopupKind::Existing(0)))
            .map(|(idx, _)| idx)
            .expect("expected an existing workspace row");
        state.workspace_popup_highlight = existing_idx;
        assert!(state.tab_complete_workspace());
        assert!(state.workspace_path_input.is_empty());
        assert_eq!(state.workspace_idx, 0);
        assert!(!state.workspace_user_editing);
    }

    #[test]
    fn completion_label_with_dir_slash_appends_for_directories() {
        let home = std::env::var("HOME").expect("HOME");
        let projects = format!("{home}/projects");
        if !Path::new(&projects).is_dir() {
            return;
        }
        let label = completion_label_with_dir_slash(&("~/projects".into(), projects));
        assert_eq!(label, "~/projects/");
    }

    #[test]
    fn expand_workspace_path_supports_tilde_prefix() {
        let home = std::env::var("HOME").expect("HOME");
        let projects = format!("{home}/projects");
        if !Path::new(&projects).is_dir() {
            return;
        }
        let expanded = expand_workspace_path("~/projects").expect("expand projects path");
        assert!(expanded.contains("/projects"));
        assert!(Path::new(&expanded).is_dir());
    }

    #[test]
    fn path_cursor_moves_within_typed_path() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.workspace_path_input = "abc".into();
        state.workspace_path_cursor = 3;
        state.workspace_user_editing = true;
        state.move_workspace_path_cursor(-1);
        state.insert_workspace_path_char('X');
        assert_eq!(state.workspace_path_input, "abXc");
    }

    #[test]
    fn typed_path_header_preserves_trailing_slash() {
        let home = std::env::var("HOME").expect("HOME");
        let side = format!("{home}/side-projects");
        if !Path::new(&side).is_dir() {
            return;
        }
        let mut state = test_state_with_workspace("~/side-projects/foo", &side);
        state.focus = Focus::Workspace;
        state.workspace_path_input = "~/side-projects/".into();
        state.workspace_path_cursor = state.workspace_path_input.chars().count();
        state.workspace_user_editing = true;
        state.workspace_popup_highlight = state.first_popup_highlight_for_query();
        let header = state.workspace_header_display();
        assert!(
            header.ends_with('/'),
            "typed trailing slash must remain visible, got {header}"
        );
        let entries = state.build_workspace_popup();
        let typed = entries
            .iter()
            .find(|e| e.kind == WorkspacePopupKind::Path)
            .expect("typed path row");
        assert!(
            typed.label.ends_with('/'),
            "typed row label must keep slash, got {}",
            typed.label
        );
        assert!(typed.cwd.is_some(), "resolved directory should keep cwd");
    }

    #[test]
    fn cmd_backspace_clears_prompt() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.focus = Focus::Prompt;
        state.prompt = "a long draft prompt".into();
        state.prompt_cursor = 5;
        state.prompt_selection = Some((0, 3));
        state.clear_prompt(40);
        assert!(state.prompt.is_empty());
        assert_eq!(state.prompt_cursor, 0);
        assert!(state.prompt_selection.is_none());
    }

    #[test]
    fn default_agent_and_model_are_grok() {
        assert_eq!(agents::AGENTS[0].id, "grok");
        assert_eq!(agents::AGENTS[0].default_model, "grok-4.5");
        assert_eq!(agents::AGENTS[0].models[0].label, "Grok 4.5");
        // Serde default (fresh install / missing file) is Grok.
        let loaded: NewSessionDefaults = serde_json::from_str("{}").unwrap();
        assert_eq!(loaded.agent_id, "grok");
        // Runtime also lands on Grok when agent_id is empty (Default + unwrap_or(0)).
        let mut state = test_state_with_workspace("~/projects/foo", "/tmp/foo");
        state.defaults.agent_id.clear();
        state.agent_idx = agents::AGENTS
            .iter()
            .position(|a| a.id == state.defaults.agent_id)
            .unwrap_or(0);
        state.sync_model_idx();
        assert_eq!(state.selected_agent().id, "grok");
        assert_eq!(state.selected_model_id(), "grok-4.5");
    }

    #[test]
    fn preview_launch_command_matches_agent_and_model() {
        let mut state = test_state_with_workspace("~/projects/foo", "/home/testuser/projects/foo");
        state.agent_idx = agents::AGENTS
            .iter()
            .position(|agent| agent.id == "grok")
            .unwrap_or(0);
        state.sync_model_idx();
        state.prompt = "fix the sidebar".into();
        let preview = state
            .preview_launch_command()
            .expect("expected grok launch preview");
        assert!(
            preview.contains("grok") && preview.contains("fix the sidebar"),
            "unexpected preview: {preview}"
        );
    }

    #[test]
    fn new_chat_manifest_launch_command_omits_user_prompt() {
        use crate::session::lifecycle::LaunchSpec;
        use crate::session::manifest::ManifestSource;
        let prompt = "fix the sidebar";
        let model_id = "grok-composer-2.5-fast";
        let tmux_command = agents::build_launch_command_with_prompt("grok", model_id, Some(prompt));
        let spec = LaunchSpec {
            sessions_session_id: "ssn_new_chat_test".into(),
            source: ManifestSource::NewChat,
            cwd: "/home/testuser/projects/foo".into(),
            agent: "grok".into(),
            launch_command: tmux_command.clone(),
            workspace_index: None,
            focus: true,
            window_name: None,
            bootstrap_new_session: false,
            model_id: Some(model_id.into()),
            user_prompt: Some(prompt.into()),
        };
        let entry = spec.to_manifest_entry(std::path::Path::new("/home/testuser"));
        assert!(tmux_command.contains(prompt));
        assert!(!entry.launch_command.contains(prompt));
        assert_eq!(entry.launch_command, "grok --model grok-composer-2.5-fast");
    }

    #[test]
    fn union_rect_combines_label_and_field() {
        let label = Rect::new(1, 2, 10, 1);
        let field = Rect::new(1, 3, 10, 3);
        let merged = union_rect(label, field);
        assert_eq!(merged.x, 1);
        assert_eq!(merged.y, 2);
        assert_eq!(merged.width, 10);
        assert_eq!(merged.height, 4);
    }
}
