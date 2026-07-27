//! New-session overlay state and business logic.

use crate::agents::{self, AgentEntry};
use crate::bar::directory_discovery::{path_query_matches_label, DirectoryIndex};
use crate::bar::notepad;
use crate::bar::settings::point_in_rect;
use crate::config::Config;
use crate::model::{ClientCommand, Session};
use crate::session::workspace_usage::{load_rank_mode, WorkspaceRankMode, WorkspaceUsageStore};
use anyhow::{Context, Result};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(in crate::bar::overlay::new_session) const FIELD_INNER_HEIGHT: u16 = 1;
pub(in crate::bar::overlay::new_session) const PROMPT_INNER_HEIGHT: u16 = 4;
pub(in crate::bar::overlay::new_session) const SECTION_GAP: u16 = 1;
pub(in crate::bar::overlay::new_session) const MAX_DROPDOWN_VISIBLE: usize = 8;
pub(in crate::bar::overlay::new_session) const DIRECTORY_DISPLAY_INITIAL: usize = 48;
pub(in crate::bar::overlay::new_session) const DIRECTORY_DISPLAY_PAGE: usize = 48;
pub(in crate::bar::overlay::new_session) const WORKSPACE_HEADER_ROWS: u16 = 1;
pub(in crate::bar::overlay::new_session) const TITLE_ROWS: u16 = 2;
pub(in crate::bar::overlay::new_session) const EXISTING_WORKSPACES_HEADER: &str = "Active Sessions";
pub(in crate::bar::overlay::new_session) const NEW_WORKSPACE_HEADER: &str = "New session";
pub(in crate::bar::overlay::new_session) const PATH_SUGGESTIONS_HEADER: &str = "Directories";
const MAX_CLOSED_DIRECTORY_SUGGESTIONS: usize = 24;
pub(in crate::bar::overlay::new_session) const CLOSE_BUTTON_COLS: u16 = 5;
pub(in crate::bar::overlay::new_session) const CLOSE_BUTTON_LABEL: &str = "[esc]";
const PROMPT_DOUBLE_CLICK_TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub(in crate::bar::overlay::new_session) enum LaunchMode {
    #[default]
    Open,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bar::overlay::new_session) enum LaunchOutcome {
    Failed,
    Opened,
    Backgrounded,
}

/// In-progress new-session form state, restored when the workspace pane is reopened.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(in crate::bar::overlay::new_session) struct NewSessionDraft {
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) workspace_path_input: String,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) workspace_user_editing: bool,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) workspace_label: Option<String>,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) workspace_confirmed: bool,
    #[serde(default = "default_agent_id")]
    pub(in crate::bar::overlay::new_session) agent_id: String,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) agent_confirmed: bool,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) model_id: String,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) model_confirmed: bool,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) prompt: String,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) prompt_cursor: usize,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) focus: Focus,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) launch_mode: LaunchMode,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(in crate::bar::overlay::new_session) struct NewSessionDefaults {
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) workspace_label: Option<String>,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) custom_workspace_path: Option<String>,
    #[serde(default = "default_agent_id")]
    pub(in crate::bar::overlay::new_session) agent_id: String,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) agent_models: HashMap<String, String>,
    #[serde(default)]
    pub(in crate::bar::overlay::new_session) launch_mode: LaunchMode,
}

fn default_agent_id() -> String {
    "grok".into()
}

#[derive(Clone)]
pub(in crate::bar::overlay::new_session) struct WorkspaceChoice {
    pub(in crate::bar::overlay::new_session) label: String,
    pub(in crate::bar::overlay::new_session) cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bar::overlay::new_session) enum WorkspacePopupKind {
    Section,
    Existing(usize),
    Path,
}

pub(in crate::bar::overlay::new_session) enum PathGhostHint {
    /// Grey suffix continuing the typed prefix (e.g. `sess` + `ions-cli`).
    Suffix(String),
    /// Grey full tilde path when the match is basename-only (e.g. `ses` + ` ~/projects/sessions-cli`).
    FullPath(String),
}

#[derive(Clone)]
pub(in crate::bar::overlay::new_session) struct WorkspacePopupEntry {
    pub(in crate::bar::overlay::new_session) kind: WorkspacePopupKind,
    pub(in crate::bar::overlay::new_session) label: String,
    pub(in crate::bar::overlay::new_session) cwd: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::bar::overlay::new_session) enum Focus {
    /// Form field order starts with Agent; runtime open may jump to Workspace when
    /// `new_session_preselect_agent_model` is enabled (the default).
    #[default]
    Agent,
    Model,
    Workspace,
    Prompt,
    ForegroundButton,
    BackgroundButton,
}

pub struct NewSessionState {
    pub(in crate::bar::overlay::new_session) directory_index: DirectoryIndex,
    pub(in crate::bar::overlay::new_session) workspace_usage: WorkspaceUsageStore,
    pub(in crate::bar::overlay::new_session) rank_mode: WorkspaceRankMode,
    /// How many directory suggestions to show; grows as the user scrolls down.
    pub(in crate::bar::overlay::new_session) directory_display_limit: usize,
    pub(in crate::bar::overlay::new_session) workspaces: Vec<WorkspaceChoice>,
    pub(in crate::bar::overlay::new_session) workspace_idx: usize,
    pub(in crate::bar::overlay::new_session) workspace_path_input: String,
    pub(in crate::bar::overlay::new_session) workspace_path_cursor: usize,
    pub(in crate::bar::overlay::new_session) workspace_user_editing: bool,
    pub(in crate::bar::overlay::new_session) workspace_popup_highlight: usize,
    pub(in crate::bar::overlay::new_session) workspace_confirmed: bool,
    pub(in crate::bar::overlay::new_session) agent_idx: usize,
    pub(in crate::bar::overlay::new_session) agent_confirmed: bool,
    pub(in crate::bar::overlay::new_session) model_idx: usize,
    pub(in crate::bar::overlay::new_session) model_confirmed: bool,
    pub(in crate::bar::overlay::new_session) prompt: String,
    pub(in crate::bar::overlay::new_session) prompt_cursor: usize,
    pub(in crate::bar::overlay::new_session) prompt_scroll: usize,
    pub(in crate::bar::overlay::new_session) prompt_selection: Option<(usize, usize)>,
    pub(in crate::bar::overlay::new_session) prompt_select_anchor: Option<usize>,
    pub(in crate::bar::overlay::new_session) prompt_drag_selecting: bool,
    pub(in crate::bar::overlay::new_session) prompt_last_click: Option<(Instant, u16, u16, u8)>,
    pub(in crate::bar::overlay::new_session) prompt_field_width: usize,
    pub(in crate::bar::overlay::new_session) launch_mode: LaunchMode,
    pub(in crate::bar::overlay::new_session) defaults: NewSessionDefaults,
    pub(in crate::bar::overlay::new_session) focus: Focus,
    pub(in crate::bar::overlay::new_session) status: String,
}

impl Clone for NewSessionState {
    fn clone(&self) -> Self {
        Self {
            directory_index: self.directory_index.clone(),
            workspace_usage: self.workspace_usage.clone(),
            rank_mode: self.rank_mode,
            directory_display_limit: self.directory_display_limit,
            workspaces: self.workspaces.clone(),
            workspace_idx: self.workspace_idx,
            workspace_path_input: self.workspace_path_input.clone(),
            workspace_path_cursor: self.workspace_path_cursor,
            workspace_user_editing: self.workspace_user_editing,
            workspace_popup_highlight: self.workspace_popup_highlight,
            workspace_confirmed: self.workspace_confirmed,
            agent_idx: self.agent_idx,
            agent_confirmed: self.agent_confirmed,
            model_idx: self.model_idx,
            model_confirmed: self.model_confirmed,
            prompt: self.prompt.clone(),
            prompt_cursor: self.prompt_cursor,
            prompt_scroll: self.prompt_scroll,
            prompt_selection: self.prompt_selection,
            prompt_select_anchor: self.prompt_select_anchor,
            prompt_drag_selecting: self.prompt_drag_selecting,
            prompt_last_click: self.prompt_last_click,
            prompt_field_width: self.prompt_field_width,
            launch_mode: self.launch_mode,
            defaults: self.defaults.clone(),
            focus: self.focus,
            status: self.status.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(in crate::bar::overlay::new_session) struct ClickTargets {
    form: Rect,
    workspace: Rect,
    workspace_field: Rect,
    workspace_popup: Rect,
    agent: Rect,
    agent_field: Rect,
    agent_popup: Rect,
    model: Rect,
    model_field: Rect,
    model_popup: Rect,
    prompt: Rect,
    prompt_field: Rect,
    foreground_button: Rect,
    pub(in crate::bar::overlay::new_session) background_button: Rect,
    pub(in crate::bar::overlay::new_session) close: Rect,
}

#[derive(Debug, Clone, Default)]
pub struct PanelHover {
    pub(in crate::bar::overlay::new_session) foreground_button: bool,
    pub(in crate::bar::overlay::new_session) background_button: bool,
    pub(in crate::bar::overlay::new_session) close: bool,
    pub(in crate::bar::overlay::new_session) workspace_popup_row: Option<usize>,
}
impl NewSessionState {
    pub fn new(config: &Config) -> Result<Self> {
        let defaults = load_defaults(config);
        let rank_mode = load_rank_mode(&config.home);
        let workspace_usage = WorkspaceUsageStore::load(&config.home);
        let directory_index = DirectoryIndex::build(config);
        let workspaces = load_workspaces(config, &workspace_usage, rank_mode);
        let preset_label = defaults.workspace_label.as_deref();
        let preset_path = defaults
            .custom_workspace_path
            .as_deref()
            .filter(|path| !path.is_empty());
        let workspace_idx = preset_label
            .and_then(|label| workspaces.iter().position(|w| w.label == label))
            .unwrap_or(0);
        let (mut workspace_path_input, mut workspace_user_editing) = if let Some(path) = preset_path
        {
            (path.to_string(), true)
        } else if !workspaces.is_empty() {
            // Browse active sessions first; header shows the most popular pwd in grey.
            (String::new(), false)
        } else {
            ("~/".to_string(), true)
        };
        // Do not prefill a custom path default that no longer exists on disk.
        if !workspace_path_input.trim().is_empty()
            && expand_workspace_path(&workspace_path_input).is_err()
        {
            workspace_path_input = if workspaces.is_empty() {
                "~/".to_string()
            } else {
                String::new()
            };
            workspace_user_editing = workspaces.is_empty();
        } else if workspace_path_input.trim().is_empty() && workspaces.is_empty() {
            workspace_path_input = "~/".to_string();
            workspace_user_editing = true;
        }
        let agent_idx = agents::AGENTS
            .iter()
            .position(|a| a.id == defaults.agent_id)
            .unwrap_or(0);
        let mut state = Self {
            directory_index,
            workspace_usage,
            rank_mode,
            directory_display_limit: DIRECTORY_DISPLAY_INITIAL,
            workspaces,
            workspace_idx,
            workspace_path_cursor: workspace_path_input.chars().count(),
            workspace_path_input,
            workspace_user_editing,
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
            launch_mode: defaults.launch_mode,
            defaults,
            focus: Focus::Agent,
            status: String::new(),
        };
        state.sync_model_idx();
        state.workspace_popup_highlight = state.default_workspace_popup_highlight();
        if let Some(draft) = load_draft(config) {
            state.apply_draft(draft, config);
        } else {
            state.apply_agent_model_preselect(config);
        }
        Ok(state)
    }

    /// When settings enable it, confirm saved agent/model defaults and land focus
    /// on the directory field so the user can type or pick a path immediately.
    pub(in crate::bar::overlay::new_session) fn apply_agent_model_preselect(
        &mut self,
        config: &Config,
    ) {
        if !crate::telemetry::config::load_new_session_preselect_agent_model(&config.home) {
            return;
        }
        self.agent_confirmed = true;
        self.model_confirmed = self.selected_agent().id != "console";
        // Keep mid-prompt draft focus; otherwise start on the path field.
        if matches!(self.focus, Focus::Prompt) && !self.prompt.trim().is_empty() {
            return;
        }
        if matches!(self.focus, Focus::Agent | Focus::Model) {
            self.focus_workspace();
        }
    }

    pub(in crate::bar::overlay::new_session) fn to_draft(&self) -> NewSessionDraft {
        let workspace_label = if self.uses_custom_path() {
            None
        } else {
            self.selected_workspace().map(|w| w.label.clone())
        };
        NewSessionDraft {
            workspace_path_input: self.workspace_path_input.clone(),
            workspace_user_editing: self.workspace_user_editing,
            workspace_label,
            workspace_confirmed: self.workspace_confirmed,
            agent_id: self.selected_agent().id.to_string(),
            agent_confirmed: self.agent_confirmed,
            model_id: self.selected_model_id().to_string(),
            model_confirmed: self.model_confirmed,
            prompt: self.prompt.clone(),
            prompt_cursor: self.prompt_cursor,
            focus: self.focus,
            launch_mode: self.launch_mode,
        }
    }

    pub(in crate::bar::overlay::new_session) fn apply_draft(
        &mut self,
        draft: NewSessionDraft,
        config: &Config,
    ) {
        self.workspace_path_input = draft.workspace_path_input;
        self.workspace_path_cursor = self.workspace_path_input.chars().count();
        self.workspace_user_editing = draft.workspace_user_editing;
        self.workspace_confirmed = draft.workspace_confirmed;

        if self.workspace_path_input.trim().is_empty() {
            if let Some(label) = draft.workspace_label.as_deref() {
                if let Some(idx) = self.workspaces.iter().position(|w| w.label == label) {
                    self.workspace_idx = idx;
                    self.workspace_user_editing = false;
                }
            }
        } else if expand_workspace_path(&self.workspace_path_input).is_err() {
            self.workspace_confirmed = false;
        }

        if let Some(idx) = agents::AGENTS
            .iter()
            .position(|agent| agent.id == draft.agent_id)
        {
            self.agent_idx = idx;
        }
        self.agent_confirmed = draft.agent_confirmed;
        self.sync_model_idx();
        let agent = self.selected_agent();
        self.model_idx = agents::model_index(agent, &draft.model_id);
        self.model_confirmed = draft.model_confirmed;

        self.prompt = draft.prompt;
        self.prompt_cursor = notepad::clamp_cursor(&self.prompt, draft.prompt_cursor);
        self.prompt_scroll = 0;
        self.prompt_selection = None;
        self.prompt_select_anchor = None;
        self.prompt_drag_selecting = false;
        self.prompt_last_click = None;
        self.launch_mode = draft.launch_mode;
        // Prefer restoring mid-prompt draft focus; otherwise start on Agent (or
        // Workspace when preselect is enabled — see apply_agent_model_preselect).
        self.focus = if !self.prompt.trim().is_empty()
            && matches!(draft.focus, Focus::Prompt)
            && self.selected_agent().id != "console"
        {
            Focus::Prompt
        } else {
            Focus::Agent
        };
        if self.selected_agent().id == "console"
            && matches!(self.focus, Focus::Model | Focus::Prompt)
        {
            self.focus = Focus::Agent;
        }
        self.status.clear();
        self.reset_directory_display_limit();
        if self.workspace_user_editing {
            self.workspace_popup_highlight = self.first_popup_highlight_for_query();
        } else {
            self.sync_popup_highlight_to_workspace_idx();
        }
        self.sync_prompt_scroll(self.prompt_field_width);
        self.apply_agent_model_preselect(config);
    }

    pub(in crate::bar::overlay::new_session) fn draft_worth_saving(&self) -> bool {
        !self.prompt.trim().is_empty()
            || self.uses_custom_path()
            || self.workspace_user_editing
            || self.workspace_confirmed
            || self.agent_confirmed
            || self.model_confirmed
    }

    pub(in crate::bar::overlay::new_session) fn save_draft(&self, config: &Config) -> Result<()> {
        if !self.draft_worth_saving() {
            return clear_draft(config);
        }
        let path = config.new_session_draft_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(&self.to_draft())?;
        std::fs::write(path, raw)?;
        Ok(())
    }

    pub(in crate::bar::overlay::new_session) fn uses_custom_path(&self) -> bool {
        !self.workspace_path_input.is_empty()
    }

    pub(in crate::bar::overlay::new_session) fn selected_workspace(
        &self,
    ) -> Option<&WorkspaceChoice> {
        if self.uses_custom_path() {
            return None;
        }
        self.workspaces.get(self.workspace_idx)
    }

    pub(in crate::bar::overlay::new_session) fn is_typing_path(&self) -> bool {
        self.workspace_user_editing
    }

    /// Returns an error message if the user is currently typing a custom path
    /// that does not exist on disk. Used for live UI feedback (no need to press
    /// Enter first).
    pub(in crate::bar::overlay::new_session) fn path_input_error(&self) -> Option<String> {
        let input = self.workspace_path_input.trim();
        if input.is_empty() || !self.uses_custom_path() {
            return None;
        }
        // ~/ and ~ are always valid (home)
        if input == "~" || input == "~/" {
            return None;
        }
        expand_workspace_path(input).err().map(|e| e.to_string())
    }

    pub(in crate::bar::overlay::new_session) fn workspace_committed_display(&self) -> String {
        if self.uses_custom_path() {
            return self.workspace_path_input.clone();
        }
        self.workspaces
            .get(self.workspace_idx)
            .map(|workspace| workspace.label.clone())
            .unwrap_or_else(|| "pick a session or directory".into())
    }

    pub(in crate::bar::overlay::new_session) fn directory_completions(
        &self,
    ) -> Vec<(String, String)> {
        let mut out = if self.workspace_user_editing {
            self.directory_index
                .completions_for_input(&self.workspace_path_input)
        } else {
            self.directory_index.browse_suggestions()
        };
        if self.workspace_path_input.trim().is_empty() {
            let active_cwds: std::collections::HashSet<String> =
                self.workspaces.iter().map(|w| w.cwd.clone()).collect();
            for (label, cwd) in self.workspace_usage.closed_suggestions(
                &active_cwds,
                self.rank_mode,
                MAX_CLOSED_DIRECTORY_SUGGESTIONS,
            ) {
                if !out
                    .iter()
                    .any(|(existing, path)| existing == &label || path == &cwd)
                {
                    out.push((label, cwd));
                }
            }
            self.sort_directory_completions(&mut out);
        }
        out
    }

    pub(in crate::bar::overlay::new_session) fn sort_directory_completions(
        &self,
        completions: &mut [(String, String)],
    ) {
        completions.sort_by(|left, right| {
            let left_score = self
                .workspace_usage
                .rank_score(&left.1, None, self.rank_mode);
            let right_score = self
                .workspace_usage
                .rank_score(&right.1, None, self.rank_mode);
            right_score
                .cmp(&left_score)
                .then_with(|| left.0.cmp(&right.0))
        });
    }

    pub(in crate::bar::overlay::new_session) fn reset_directory_display_limit(&mut self) {
        self.directory_display_limit = DIRECTORY_DISPLAY_INITIAL;
    }

    pub(in crate::bar::overlay::new_session) fn maybe_expand_directory_list(&mut self) {
        let total = self.directory_completions().len();
        if self.directory_display_limit >= total {
            return;
        }
        let entries = self.build_workspace_popup();
        let last_directory_idx = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.kind == WorkspacePopupKind::Path && entry.cwd.is_some())
            .map(|(idx, _)| idx)
            .next_back();
        if let Some(last_idx) = last_directory_idx {
            if self.workspace_popup_highlight >= last_idx.saturating_sub(2) {
                self.directory_display_limit = self
                    .directory_display_limit
                    .saturating_add(DIRECTORY_DISPLAY_PAGE)
                    .min(total);
            }
        }
    }

    pub(in crate::bar::overlay::new_session) fn workspace_header_display(&self) -> String {
        if self.focus == Focus::Workspace {
            // A committed custom path must not be replaced by whichever existing
            // workspace row happens to share the old popup index after edit mode ends.
            if !self.workspace_user_editing && self.uses_custom_path() {
                return self.workspace_committed_display();
            }
            // While actively typing, always show the raw input (preserves `/` and cursor).
            // Arrow-selected suggestions still replace the header via the popup highlight.
            if self.workspace_user_editing && self.on_workspace_typed_row() {
                let typed = self.workspace_path_input.trim();
                if typed == "~" {
                    return "~/".into();
                }
                if !typed.is_empty() {
                    return self.workspace_path_input.clone();
                }
            }
            let entries = self.build_workspace_popup();
            if let Some(entry) = entries.get(self.workspace_popup_highlight) {
                match entry.kind {
                    WorkspacePopupKind::Existing(idx) => {
                        if let Some(workspace) = self.workspaces.get(idx) {
                            return workspace.label.clone();
                        }
                    }
                    WorkspacePopupKind::Path => return entry.label.clone(),
                    WorkspacePopupKind::Section => {}
                }
            }
            if self.workspace_user_editing && !self.workspace_path_input.is_empty() {
                return self.workspace_path_input.clone();
            }
        }
        self.workspace_committed_display()
    }

    pub(in crate::bar::overlay::new_session) fn build_workspace_popup(
        &self,
    ) -> Vec<WorkspacePopupEntry> {
        let mut rows = Vec::new();
        let query = if self.workspace_user_editing {
            self.workspace_path_input.trim().to_lowercase()
        } else {
            String::new()
        };
        let typing = self.is_typing_path();

        if typing {
            rows.push(WorkspacePopupEntry {
                kind: WorkspacePopupKind::Section,
                label: NEW_WORKSPACE_HEADER.into(),
                cwd: None,
            });
            let typed = self.workspace_path_input.trim();
            if !typed.is_empty() {
                // Keep the user's typed text as the row label (including a trailing `/`).
                // Normalized tilde labels strip trailing slashes, which made `/` "disappear"
                // while typing through directories. Still attach cwd when the path resolves
                // so Enter accepts a valid directory without "not a directory".
                let cwd = expand_workspace_path(typed).ok();
                let label = if typed == "~" {
                    // Default the search bar to ~/ — user must backspace to remove.
                    "~/".to_string()
                } else {
                    typed.to_string()
                };
                rows.push(WorkspacePopupEntry {
                    kind: WorkspacePopupKind::Path,
                    label,
                    cwd,
                });
            } else {
                rows.push(WorkspacePopupEntry {
                    kind: WorkspacePopupKind::Section,
                    label: "  type ~/path or project name".into(),
                    cwd: None,
                });
            }
        }

        rows.push(WorkspacePopupEntry {
            kind: WorkspacePopupKind::Section,
            label: EXISTING_WORKSPACES_HEADER.into(),
            cwd: None,
        });

        let mut matched_existing = 0usize;
        for (idx, workspace) in self.ranked_workspaces() {
            let matches = query.is_empty()
                || path_query_matches_label(&query, &workspace.label)
                || workspace.cwd.to_lowercase().contains(&query);
            if matches {
                matched_existing += 1;
                rows.push(WorkspacePopupEntry {
                    kind: WorkspacePopupKind::Existing(idx),
                    label: workspace.label.clone(),
                    cwd: Some(workspace.cwd.clone()),
                });
            }
        }
        if matched_existing == 0 {
            rows.push(WorkspacePopupEntry {
                kind: WorkspacePopupKind::Section,
                label: if typing {
                    "  (no match)".into()
                } else {
                    "  (none yet — type a path or pick a directory below)".into()
                },
                cwd: None,
            });
        }

        let all_completions = self.directory_completions();
        let total_completions = all_completions.len();
        let shown_count = self.directory_display_limit.min(total_completions);
        rows.push(WorkspacePopupEntry {
            kind: WorkspacePopupKind::Section,
            label: PATH_SUGGESTIONS_HEADER.into(),
            cwd: None,
        });
        if total_completions == 0 {
            rows.push(WorkspacePopupEntry {
                kind: WorkspacePopupKind::Section,
                label: if typing {
                    "  Tab to complete".into()
                } else {
                    "  type ~/path or pick below".into()
                },
                cwd: None,
            });
        } else {
            for (label, cwd) in all_completions.into_iter().take(shown_count) {
                rows.push(WorkspacePopupEntry {
                    kind: WorkspacePopupKind::Path,
                    label,
                    cwd: Some(cwd),
                });
            }
            if shown_count < total_completions {
                rows.push(WorkspacePopupEntry {
                    kind: WorkspacePopupKind::Section,
                    label: format!(
                        "  ↓ {} more — press ↓ to load",
                        total_completions - shown_count
                    ),
                    cwd: None,
                });
            }
        }

        rows
    }

    pub(in crate::bar::overlay::new_session) fn sync_popup_highlight_to_path_label(
        &mut self,
        label: &str,
    ) {
        let entries = self.build_workspace_popup();
        self.workspace_popup_highlight = entries
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.kind == WorkspacePopupKind::Path && entry.label == label)
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| self.first_popup_highlight_for_query());
    }

    pub(in crate::bar::overlay::new_session) fn sync_popup_highlight_to_workspace_idx(&mut self) {
        if !self.workspace_user_editing && self.uses_custom_path() {
            let label = self.workspace_path_input.clone();
            self.sync_popup_highlight_to_path_label(&label);
            return;
        }
        let entries = self.build_workspace_popup();
        self.workspace_popup_highlight = entries
            .iter()
            .enumerate()
            .find(|(_, entry)| {
                matches!(
                    entry.kind,
                    WorkspacePopupKind::Existing(idx) if idx == self.workspace_idx
                )
            })
            .map(|(idx, _)| idx)
            .or_else(|| {
                entries.iter().enumerate().find_map(|(idx, entry)| {
                    if entry.kind != WorkspacePopupKind::Section {
                        Some(idx)
                    } else {
                        None
                    }
                })
            })
            .unwrap_or(0);
    }

    pub(in crate::bar::overlay::new_session) fn select_existing_workspace(&mut self, idx: usize) {
        if idx < self.workspaces.len() {
            self.workspace_idx = idx;
            self.workspace_path_input.clear();
            self.workspace_user_editing = false;
            self.workspace_confirmed = false;
            self.sync_popup_highlight_to_workspace_idx();
        }
    }

    pub(in crate::bar::overlay::new_session) fn select_path_label(
        &mut self,
        label: String,
        from_user_edit: bool,
    ) {
        self.workspace_path_input = label.clone();
        self.workspace_path_cursor = label.chars().count();
        self.workspace_user_editing = from_user_edit;
        self.workspace_confirmed = false;
        if from_user_edit {
            self.workspace_popup_highlight = self.first_popup_highlight_for_query();
        } else {
            self.sync_popup_highlight_to_path_label(&label);
        }
    }

    /// Apply the highlighted popup row to workspace state (selection only).
    pub(in crate::bar::overlay::new_session) fn apply_workspace_popup_selection(&mut self) {
        let entries = self.build_workspace_popup();
        if let Some(entry) = entries.get(self.workspace_popup_highlight) {
            match entry.kind {
                WorkspacePopupKind::Existing(idx) => {
                    self.select_existing_workspace(idx);
                }
                WorkspacePopupKind::Path => {
                    let from_edit = entry.cwd.is_none();
                    self.select_path_label(entry.label.clone(), from_edit);
                }
                WorkspacePopupKind::Section => {}
            }
        }
    }

    /// Validate the current workspace selection and set `workspace_confirmed` when valid.
    pub(in crate::bar::overlay::new_session) fn confirm_workspace_selection(
        &mut self,
        show_errors: bool,
    ) -> bool {
        self.status.clear();
        match self.resolve_workspace() {
            Ok(_) => {
                self.workspace_confirmed = true;
                if self.uses_custom_path() {
                    // Normalize user-typed bare/exact name (e.g. "pictures") to the
                    // resolved nice label with real on-disk casing and ~/ (e.g. "~/Pictures").
                    if let Ok((_, nice_label)) = self.resolve_workspace() {
                        self.workspace_path_input = nice_label;
                    }
                }
                true
            }
            Err(error) => {
                self.workspace_confirmed = false;
                if self.uses_custom_path() || self.workspaces.is_empty() {
                    if show_errors {
                        self.status = error.to_string();
                    }
                    false
                } else if self.workspace_idx < self.workspaces.len() {
                    self.workspace_confirmed = true;
                    true
                } else if show_errors {
                    self.status = "pick a session or type ~/path".into();
                    false
                } else {
                    false
                }
            }
        }
    }

    /// Commit the highlighted popup row when leaving the field without pressing Enter.
    pub(in crate::bar::overlay::new_session) fn confirm_workspace_on_blur(&mut self) {
        if self.workspace_confirmed {
            return;
        }
        // Keep a typed/selected custom path even when the popup highlight drifted
        // onto an active session row after edit mode closed.
        if self.uses_custom_path() && self.confirm_workspace_selection(false) {
            return;
        }
        self.apply_workspace_popup_selection();
        let _ = self.confirm_workspace_selection(false);
    }

    pub(in crate::bar::overlay::new_session) fn set_focus(&mut self, next: Focus) {
        if self.focus == Focus::Workspace && next != Focus::Workspace {
            self.confirm_workspace_on_blur();
        }
        self.focus = next;
    }

    /// Enter on workspace: apply the highlighted row, validate, and advance when ready.
    ///
    /// While typing on the live path row:
    /// - Prefer the typed text when it resolves to a real directory (never let a
    ///   stale highlight silently replace it — that is how users landed on `~` / hub roots).
    /// - If typed text does not resolve, accept a unique completion (basename search
    ///   like `sessions` → `~/side-projects/dev-tools/sessions-cli`).
    /// - If the user arrowed to another row, honor that highlight.
    pub(in crate::bar::overlay::new_session) fn confirm_workspace_enter(&mut self) -> bool {
        if self.workspace_user_editing && self.on_workspace_typed_row() {
            let typed = self.workspace_path_input.trim().to_string();
            if !typed.is_empty() {
                if expand_workspace_path(&typed).is_ok() {
                    return self.confirm_workspace_selection(true);
                }
                if let Some(completion) = self.unique_enter_path_completion() {
                    self.apply_workspace_path_completion(&completion);
                    return self.confirm_workspace_selection(true);
                }
            }
        }
        self.apply_workspace_popup_selection();
        self.confirm_workspace_selection(true)
    }

    /// Single best path match for Enter when the typed string is not yet a real directory.
    pub(in crate::bar::overlay::new_session) fn unique_enter_path_completion(
        &self,
    ) -> Option<(String, String)> {
        let input = self.workspace_path_input.trim();
        if input.is_empty() {
            return None;
        }
        let input_lower = input.to_lowercase();
        let candidates = self.path_completion_candidates();
        // Prefer an exact label match when present.
        let exact: Vec<_> = candidates
            .iter()
            .filter(|(label, _)| label.to_lowercase() == input_lower)
            .cloned()
            .collect();
        if exact.len() == 1 {
            return Some(exact[0].clone());
        }
        // Unique completion whose label continues or basename-matches the query.
        let matching: Vec<_> = candidates
            .iter()
            .filter(|(label, cwd)| {
                let label_l = label.to_lowercase();
                label_l != input_lower
                    && (label_l.starts_with(&input_lower)
                        || path_query_matches_label(input, label)
                        || cwd.to_lowercase().contains(&input_lower))
            })
            .cloned()
            .collect();
        if matching.len() == 1 {
            return Some(matching[0].clone());
        }
        // Ghost-style longest common completion that resolves to exactly one path.
        if let Some(completed) = longest_path_completion(input, &candidates) {
            let completed_lower = completed.to_lowercase();
            if completed_lower != input_lower {
                let resolved: Vec<_> = candidates
                    .iter()
                    .filter(|(label, _)| label.to_lowercase().starts_with(&completed_lower))
                    .cloned()
                    .collect();
                if resolved.len() == 1 {
                    return Some(resolved[0].clone());
                }
            }
        }
        None
    }

    pub(in crate::bar::overlay::new_session) fn confirm_agent_enter(&mut self) {
        self.status.clear();
        self.agent_confirmed = true;
    }

    pub(in crate::bar::overlay::new_session) fn confirm_model_enter(&mut self) {
        self.status.clear();
        self.model_confirmed = true;
    }

    pub(in crate::bar::overlay::new_session) fn tab_complete_workspace(&mut self) -> bool {
        if self.try_filesystem_tab_completion() {
            return true;
        }
        let entries = self.build_workspace_popup();
        if let Some(entry) = entries.get(self.workspace_popup_highlight) {
            match entry.kind {
                WorkspacePopupKind::Existing(idx) => {
                    self.select_existing_workspace(idx);
                    return true;
                }
                WorkspacePopupKind::Path => {
                    let from_edit = entry.cwd.is_none();
                    self.select_path_label(entry.label.clone(), from_edit);
                    return true;
                }
                WorkspacePopupKind::Section => {}
            }
        }
        false
    }

    /// Shell-style Tab completion while the user is editing the typed path row.
    pub(in crate::bar::overlay::new_session) fn on_workspace_typed_row(&self) -> bool {
        if !self.is_typing_path() {
            return false;
        }
        let entries = self.build_workspace_popup();
        // First Path row while typing is always the live typed input (not a suggestion).
        let typed_row = entries
            .iter()
            .position(|entry| entry.kind == WorkspacePopupKind::Path);
        typed_row == Some(self.workspace_popup_highlight)
    }

    pub(in crate::bar::overlay::new_session) fn path_completion_candidates(
        &self,
    ) -> Vec<(String, String)> {
        let mut out = self.directory_completions();
        let query = self.workspace_path_input.trim();
        if query.is_empty() {
            return out;
        }
        for workspace in &self.workspaces {
            if path_query_matches_label(query, &workspace.label)
                && !out
                    .iter()
                    .any(|(label, cwd)| label == &workspace.label || cwd == &workspace.cwd)
            {
                out.push((workspace.label.clone(), workspace.cwd.clone()));
            }
        }
        out
    }

    pub(in crate::bar::overlay::new_session) fn workspace_path_ghost_hint(
        &self,
    ) -> Option<PathGhostHint> {
        if !self.on_workspace_typed_row() {
            return None;
        }
        let input = self.workspace_path_input.trim();
        if input.is_empty() {
            return None;
        }
        let completions = self.path_completion_candidates();
        if completions.is_empty() {
            return None;
        }
        let input_lower = input.to_lowercase();
        if let Some(completed) = longest_path_completion(input, &completions) {
            let completed_lower = completed.to_lowercase();
            if completed_lower != input_lower {
                if completed_lower.starts_with(&input_lower) {
                    let suffix = completed
                        .chars()
                        .skip(input.chars().count())
                        .collect::<String>();
                    if !suffix.is_empty() {
                        return Some(PathGhostHint::Suffix(suffix));
                    }
                }
                return Some(PathGhostHint::FullPath(completed));
            }
        }
        let matching: Vec<_> = completions
            .iter()
            .filter(|(label, _)| path_query_matches_label(input, label))
            .collect();
        if matching.len() == 1 {
            let label = &matching[0].0;
            let label_lower = label.to_lowercase();
            if label_lower != input_lower {
                if label_lower.starts_with(&input_lower) {
                    let suffix = label
                        .chars()
                        .skip(input.chars().count())
                        .collect::<String>();
                    if !suffix.is_empty() {
                        return Some(PathGhostHint::Suffix(suffix));
                    }
                }
                return Some(PathGhostHint::FullPath(label.clone()));
            }
        }
        None
    }

    pub(in crate::bar::overlay::new_session) fn try_filesystem_tab_completion(&mut self) -> bool {
        if !self.is_typing_path() {
            return false;
        }
        if !self.on_workspace_typed_row() {
            return false;
        }
        let input = self.workspace_path_input.trim();
        if input.is_empty() {
            return false;
        }
        let completions = self.path_completion_candidates();
        if completions.is_empty() {
            return false;
        }
        let input_lower = input.to_lowercase();
        let exact_matches: Vec<_> = completions
            .iter()
            .filter(|(label, _)| label.to_lowercase() == input_lower)
            .collect();
        if exact_matches.len() == 1 {
            self.apply_workspace_path_completion(exact_matches[0]);
            return true;
        }
        if completions.len() == 1 {
            self.apply_workspace_path_completion(&completions[0]);
            return true;
        }
        if let Some(completed) = longest_path_completion(&self.workspace_path_input, &completions) {
            let completed_lower = completed.to_lowercase();
            let matching: Vec<_> = completions
                .iter()
                .filter(|(label, _)| label.to_lowercase().starts_with(&completed_lower))
                .collect();
            if matching.len() == 1 {
                self.apply_workspace_path_completion(matching[0]);
            } else {
                self.workspace_path_cursor = completed.chars().count();
                self.workspace_path_input = completed;
                self.workspace_user_editing = true;
                self.workspace_confirmed = false;
                self.workspace_popup_highlight = self.first_popup_highlight_for_query();
            }
            return true;
        }
        false
    }

    pub(in crate::bar::overlay::new_session) fn apply_workspace_path_completion(
        &mut self,
        completion: &(String, String),
    ) {
        self.workspace_path_input = completion_label_with_dir_slash(completion);
        self.workspace_path_cursor = self.workspace_path_input.chars().count();
        self.workspace_user_editing = true;
        self.workspace_confirmed = false;
        self.reset_directory_display_limit();
        self.workspace_popup_highlight = self.first_popup_highlight_for_query();
    }

    pub(in crate::bar::overlay::new_session) fn sync_workspace_path_cursor(&mut self) {
        self.workspace_path_cursor =
            notepad::clamp_cursor(&self.workspace_path_input, self.workspace_path_cursor);
    }

    pub(in crate::bar::overlay::new_session) fn move_workspace_path_cursor(&mut self, delta: i32) {
        let len = self.workspace_path_input.chars().count();
        let cursor = self.workspace_path_cursor as i32;
        self.workspace_path_cursor = (cursor + delta).clamp(0, len as i32) as usize;
    }

    pub(in crate::bar::overlay::new_session) fn insert_workspace_path_char(&mut self, ch: char) {
        if !self.workspace_user_editing {
            self.workspace_path_input.clear();
            self.workspace_user_editing = true;
            self.workspace_path_cursor = 0;
        }
        if self.workspace_path_input.trim() == "~" {
            self.workspace_path_input = "~/".to_string();
            self.workspace_path_cursor = 2;
        }
        self.sync_workspace_path_cursor();
        let cursor = self.workspace_path_cursor;
        let byte_idx = self
            .workspace_path_input
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.workspace_path_input.len());
        self.workspace_path_input.insert(byte_idx, ch);
        self.workspace_path_cursor = cursor + 1;
        self.workspace_confirmed = false;
        self.status.clear();
        self.reset_directory_display_limit();
        self.workspace_popup_highlight = self.first_popup_highlight_for_query();
    }

    pub(in crate::bar::overlay::new_session) fn on_workspace_type(&mut self, ch: char) {
        self.insert_workspace_path_char(ch);
    }

    pub(in crate::bar::overlay::new_session) fn apply_paste(&mut self, raw: &str) {
        let allow_newlines = self.focus == Focus::Prompt;
        let text = crate::clipboard::sanitize_paste_text(raw, allow_newlines);
        match self.focus {
            Focus::Prompt => self.insert_prompt_str(&text, self.prompt_field_width),
            Focus::Workspace if !text.is_empty() => {
                self.workspace_path_cursor = text.chars().count();
                self.workspace_path_input = text;
                self.workspace_user_editing = true;
                self.workspace_confirmed = false;
                self.status.clear();
                self.reset_directory_display_limit();
                self.workspace_popup_highlight = self.first_popup_highlight_for_query();
            }
            _ => {}
        }
    }

    pub(in crate::bar::overlay::new_session) fn on_workspace_backspace(&mut self) {
        if !self.workspace_user_editing {
            return;
        }
        self.sync_workspace_path_cursor();
        if self.workspace_path_cursor == 0 {
            return;
        }
        let cursor = self.workspace_path_cursor;
        let byte_idx = self
            .workspace_path_input
            .char_indices()
            .nth(cursor - 1)
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let next_byte = self
            .workspace_path_input
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.workspace_path_input.len());
        self.workspace_path_input
            .replace_range(byte_idx..next_byte, "");
        self.workspace_path_cursor = cursor - 1;
        if self.workspace_path_input.is_empty() {
            self.workspace_user_editing = false;
            self.workspace_path_cursor = 0;
            self.sync_popup_highlight_to_workspace_idx();
        } else {
            self.reset_directory_display_limit();
            self.workspace_popup_highlight = self.first_popup_highlight_for_query();
        }
        self.workspace_confirmed = false;
        self.status.clear();
    }

    pub(in crate::bar::overlay::new_session) fn on_workspace_forward_delete(&mut self) {
        if !self.workspace_user_editing {
            return;
        }
        self.sync_workspace_path_cursor();
        let len = self.workspace_path_input.chars().count();
        if self.workspace_path_cursor >= len {
            return;
        }
        let cursor = self.workspace_path_cursor;
        let byte_idx = self
            .workspace_path_input
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.workspace_path_input.len());
        let next_byte = self
            .workspace_path_input
            .char_indices()
            .nth(cursor + 1)
            .map(|(idx, _)| idx)
            .unwrap_or(self.workspace_path_input.len());
        self.workspace_path_input
            .replace_range(byte_idx..next_byte, "");
        if self.workspace_path_input.is_empty() {
            self.workspace_user_editing = false;
            self.workspace_path_cursor = 0;
            self.sync_popup_highlight_to_workspace_idx();
        } else {
            self.reset_directory_display_limit();
            self.workspace_popup_highlight = self.first_popup_highlight_for_query();
        }
        self.workspace_confirmed = false;
        self.status.clear();
    }

    pub(in crate::bar::overlay::new_session) fn ranked_workspaces(
        &self,
    ) -> Vec<(usize, &WorkspaceChoice)> {
        let mut ranked: Vec<(usize, &WorkspaceChoice, (i64, i64))> = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(idx, workspace)| {
                let score = self
                    .workspace_usage
                    .rank_score(&workspace.cwd, None, self.rank_mode);
                (idx, workspace, score)
            })
            .collect();
        ranked.sort_by(|left, right| {
            right
                .2
                .cmp(&left.2)
                .then_with(|| left.1.label.cmp(&right.1.label))
        });
        ranked
            .into_iter()
            .map(|(idx, workspace, _)| (idx, workspace))
            .collect()
    }

    pub(in crate::bar::overlay::new_session) fn default_workspace_popup_highlight(&self) -> usize {
        let entries = self.build_workspace_popup();
        if self.workspace_path_input.trim().is_empty() {
            if let Some((idx, _)) = entries
                .iter()
                .enumerate()
                .find(|(_, entry)| matches!(entry.kind, WorkspacePopupKind::Existing(_)))
            {
                return idx;
            }
        }
        self.first_popup_highlight_for_query()
    }

    pub(in crate::bar::overlay::new_session) fn first_popup_highlight_for_query(&self) -> usize {
        let entries = self.build_workspace_popup();
        let query_empty = self.workspace_path_input.trim().is_empty();
        if query_empty {
            if let Some((idx, _)) = entries
                .iter()
                .enumerate()
                .find(|(_, entry)| matches!(entry.kind, WorkspacePopupKind::Existing(_)))
            {
                return idx;
            }
        }
        entries
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.kind == WorkspacePopupKind::Path)
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    pub(in crate::bar::overlay::new_session) fn resolve_workspace(
        &self,
    ) -> Result<(String, String)> {
        if self.uses_custom_path() {
            let cwd = expand_workspace_path(&self.workspace_path_input)?;
            let label = format_tilde_path(&cwd);
            Ok((cwd, label))
        } else {
            let workspace = self
                .workspaces
                .get(self.workspace_idx)
                .context("pick an existing workspace or type a path")?;
            Ok((workspace.cwd.clone(), workspace.label.clone()))
        }
    }

    pub(in crate::bar::overlay::new_session) fn selected_agent(&self) -> &'static AgentEntry {
        &agents::AGENTS[self.agent_idx % agents::AGENTS.len()]
    }

    pub(in crate::bar::overlay::new_session) fn model_id_for_agent(
        &self,
        agent: &AgentEntry,
    ) -> &str {
        self.defaults
            .agent_models
            .get(agent.id)
            .map(String::as_str)
            .unwrap_or(agent.default_model)
    }

    pub(in crate::bar::overlay::new_session) fn sync_model_idx(&mut self) {
        let agent = self.selected_agent();
        let model_id = self.model_id_for_agent(agent);
        self.model_idx = agents::model_index(agent, model_id);
    }

    pub(in crate::bar::overlay::new_session) fn selected_model_id(&self) -> &str {
        let agent = self.selected_agent();
        agent
            .models
            .get(self.model_idx % agent.models.len())
            .map(|model| model.id)
            .unwrap_or(agent.default_model)
    }

    /// Shell command Foreground/Background will run (mirrors launch orchestration).
    pub(in crate::bar::overlay::new_session) fn preview_launch_command(&self) -> Option<String> {
        let agent = self.selected_agent();
        if agent.id == "console" {
            return Some(std::env::var("SHELL").unwrap_or_else(|_| "zsh".into()));
        }
        let cwd = self
            .resolve_workspace()
            .ok()
            .map(|(cwd, _)| cwd)
            .unwrap_or_default();
        let model_id = self.selected_model_id();
        let prompt = self.prompt.trim();
        let deliver_prompt_via_tmux =
            !prompt.is_empty() && agents::deliver_prompt_via_tmux(agent.id, model_id, &cwd, prompt);
        let cmd = agents::build_launch_command_with_prompt(
            agent.id,
            model_id,
            if deliver_prompt_via_tmux {
                None
            } else {
                Some(prompt)
            },
        );
        if cmd.is_empty() {
            return None;
        }
        if deliver_prompt_via_tmux {
            Some(format!("{cmd}  (+ prompt via tmux)"))
        } else {
            Some(cmd)
        }
    }

    pub(in crate::bar::overlay::new_session) fn model_count(&self) -> usize {
        self.selected_agent().models.len()
    }

    pub(in crate::bar::overlay::new_session) fn focus_workspace(&mut self) {
        self.focus = Focus::Workspace;
        if !self.workspace_user_editing {
            self.sync_popup_highlight_to_workspace_idx();
        }
    }

    /// Re-enter the editable session-path header after picking from the dropdown.
    pub(in crate::bar::overlay::new_session) fn begin_workspace_path_edit(&mut self) {
        if self.workspace_user_editing {
            return;
        }
        let display = if self.focus == Focus::Workspace {
            self.workspace_header_display()
        } else {
            self.workspace_committed_display()
        };
        if display == "pick a session or directory" {
            self.workspace_path_input = "~/".to_string();
        } else {
            self.workspace_path_input = display;
        }
        self.workspace_user_editing = true;
        self.workspace_path_cursor = self.workspace_path_input.chars().count();
        self.workspace_confirmed = false;
        self.status.clear();
        self.reset_directory_display_limit();
        self.workspace_popup_highlight = self.first_popup_highlight_for_query();
    }

    pub(in crate::bar::overlay::new_session) fn workspace_popup_selectable_indices(
        entries: &[WorkspacePopupEntry],
    ) -> Vec<usize> {
        entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                if entry.kind != WorkspacePopupKind::Section {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    pub(in crate::bar::overlay::new_session) fn cycle_workspace_popup(&mut self, delta: i32) {
        if delta > 0 {
            self.maybe_expand_directory_list();
        }
        let entries = self.build_workspace_popup();
        let selectable = Self::workspace_popup_selectable_indices(&entries);
        if selectable.is_empty() {
            return;
        }
        let current = selectable
            .iter()
            .position(|&idx| idx == self.workspace_popup_highlight)
            .unwrap_or(0);
        if delta < 0 && current == 0 && !self.workspace_user_editing {
            self.begin_workspace_path_edit();
            return;
        }
        let next = (current as i32 + delta).rem_euclid(selectable.len() as i32) as usize;
        self.workspace_popup_highlight = selectable[next];
    }

    pub(in crate::bar::overlay::new_session) fn cycle_agent(&mut self, delta: i32) {
        let len = agents::AGENTS.len() as i32;
        self.agent_idx = (self.agent_idx as i32 + delta).rem_euclid(len) as usize;
        self.agent_confirmed = false;
        self.sync_model_idx();
        if self.selected_agent().id == "console"
            && matches!(self.focus, Focus::Model | Focus::Prompt)
        {
            self.focus = Focus::ForegroundButton;
        }
    }

    pub(in crate::bar::overlay::new_session) fn cycle_model(&mut self, delta: i32) {
        let len = self.model_count() as i32;
        if len == 0 {
            return;
        }
        self.model_idx = (self.model_idx as i32 + delta).rem_euclid(len) as usize;
        self.model_confirmed = false;
    }

    pub(in crate::bar::overlay::new_session) fn cycle_focused_dropdown(&mut self, delta: i32) {
        match self.focus {
            Focus::Workspace => self.cycle_workspace_popup(delta),
            Focus::Agent => self.cycle_agent(delta),
            Focus::Model => self.cycle_model(delta),
            Focus::Prompt | Focus::ForegroundButton | Focus::BackgroundButton => {}
        }
    }

    pub(in crate::bar::overlay::new_session) fn next_focus(&self) -> Focus {
        match self.focus {
            Focus::Agent => {
                if self.selected_agent().id == "console" {
                    Focus::Workspace
                } else {
                    Focus::Model
                }
            }
            Focus::Model => Focus::Workspace,
            Focus::Workspace => {
                if self.selected_agent().id == "console" {
                    Focus::ForegroundButton
                } else {
                    Focus::Prompt
                }
            }
            Focus::Prompt => Focus::ForegroundButton,
            Focus::ForegroundButton => Focus::BackgroundButton,
            Focus::BackgroundButton => Focus::Agent,
        }
    }

    pub(in crate::bar::overlay::new_session) fn prev_focus(&self) -> Focus {
        match self.focus {
            Focus::Agent => Focus::BackgroundButton,
            Focus::Model => Focus::Agent,
            Focus::Workspace => {
                if self.selected_agent().id == "console" {
                    Focus::Agent
                } else {
                    Focus::Model
                }
            }
            Focus::Prompt => Focus::Workspace,
            Focus::ForegroundButton => {
                if self.selected_agent().id == "console" {
                    Focus::Workspace
                } else {
                    Focus::Prompt
                }
            }
            Focus::BackgroundButton => Focus::ForegroundButton,
        }
    }

    pub(in crate::bar::overlay::new_session) fn set_default_for_focus(&mut self, config: &Config) {
        match self.focus {
            Focus::Workspace => {
                if self.uses_custom_path() {
                    let input = self.workspace_path_input.trim();
                    if !input.is_empty() {
                        if let Ok((cwd, label)) = self.resolve_workspace() {
                            self.defaults.custom_workspace_path = Some(input.to_string());
                            self.defaults.workspace_label = Some(label);
                            let _ = cwd;
                        }
                    }
                } else if let Some(workspace) = self.selected_workspace() {
                    self.defaults.workspace_label = Some(workspace.label.clone());
                    self.defaults.custom_workspace_path = None;
                }
            }
            Focus::Agent => {
                let agent = self.selected_agent();
                self.defaults.agent_id = agent.id.to_string();
                self.defaults
                    .agent_models
                    .insert(agent.id.to_string(), self.selected_model_id().to_string());
            }
            Focus::Model => {
                let agent = self.selected_agent();
                self.defaults
                    .agent_models
                    .insert(agent.id.to_string(), self.selected_model_id().to_string());
            }
            Focus::ForegroundButton => {
                self.defaults.launch_mode = LaunchMode::Open;
            }
            Focus::BackgroundButton => {
                self.defaults.launch_mode = LaunchMode::Background;
            }
            Focus::Prompt => return,
        }
        if save_defaults(config, &self.defaults).is_ok() {
            self.status = "default saved".into();
        } else {
            self.status = "could not save default".into();
        }
    }

    pub(in crate::bar::overlay::new_session) fn is_default_focus(&self) -> bool {
        match self.focus {
            Focus::Workspace => {
                if self.uses_custom_path() {
                    self.defaults.custom_workspace_path.as_deref()
                        == Some(self.workspace_path_input.trim())
                } else {
                    self.defaults.custom_workspace_path.is_none()
                        && self.defaults.workspace_label.as_ref().is_some_and(|label| {
                            self.selected_workspace().is_some_and(|w| &w.label == label)
                        })
                }
            }
            Focus::Agent => self.defaults.agent_id == self.selected_agent().id,
            Focus::Model => {
                let agent = self.selected_agent();
                self.model_id_for_agent(agent) == self.selected_model_id()
            }
            Focus::ForegroundButton => self.defaults.launch_mode == LaunchMode::Open,
            Focus::BackgroundButton => self.defaults.launch_mode == LaunchMode::Background,
            Focus::Prompt => false,
        }
    }

    pub(in crate::bar::overlay::new_session) fn prompt_content_width(
        &self,
        inner_width: u16,
    ) -> usize {
        inner_width.saturating_sub(2) as usize
    }

    pub(in crate::bar::overlay::new_session) fn sync_prompt_scroll(
        &mut self,
        content_width: usize,
    ) {
        let cursor_line =
            notepad::display_line_index(&self.prompt, self.prompt_cursor, content_width);
        self.prompt_scroll = crate::bar::ui::notepad_scroll_for_cursor(
            self.prompt_scroll,
            cursor_line,
            PROMPT_INNER_HEIGHT,
        );
    }

    pub(in crate::bar::overlay::new_session) fn scroll_prompt_lines(
        &mut self,
        delta: i32,
        content_width: usize,
    ) {
        let line_count = notepad::wrapped_display_lines(&self.prompt, content_width).len();
        let viewport_rows = PROMPT_INNER_HEIGHT as usize;
        let max_scroll = line_count.saturating_sub(viewport_rows);
        let next = (self.prompt_scroll as i32 + delta).clamp(0, max_scroll as i32) as usize;
        if next != self.prompt_scroll {
            self.prompt_scroll = next;
        }
    }

    pub(in crate::bar::overlay::new_session) fn clear_prompt_selection(&mut self) {
        self.prompt_selection = None;
    }

    /// Clear the entire prompt (⌘⌫).
    pub(in crate::bar::overlay::new_session) fn clear_prompt(&mut self, content_width: usize) {
        self.prompt.clear();
        self.prompt_cursor = 0;
        self.prompt_scroll = 0;
        self.prompt_selection = None;
        self.prompt_select_anchor = None;
        self.prompt_drag_selecting = false;
        self.sync_prompt_scroll(content_width);
        self.status.clear();
    }

    pub(in crate::bar::overlay::new_session) fn prompt_delete_selection(
        &mut self,
        content_width: usize,
    ) -> bool {
        let Some((start, end)) = self.prompt_selection.filter(|(start, end)| start < end) else {
            return false;
        };
        notepad::delete_char_range(&mut self.prompt, start, end);
        self.prompt_cursor = start;
        self.prompt_selection = None;
        self.sync_prompt_scroll(content_width);
        true
    }

    pub(in crate::bar::overlay::new_session) fn insert_prompt_char(
        &mut self,
        ch: char,
        content_width: usize,
    ) {
        self.prompt_delete_selection(content_width);
        let cursor = notepad::clamp_cursor(&self.prompt, self.prompt_cursor);
        let byte_idx = self
            .prompt
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.prompt.len());
        self.prompt.insert(byte_idx, ch);
        self.prompt_cursor = cursor + 1;
        self.sync_prompt_scroll(content_width);
    }

    pub(in crate::bar::overlay::new_session) fn insert_prompt_str(
        &mut self,
        text: &str,
        content_width: usize,
    ) {
        if text.is_empty() {
            return;
        }
        self.prompt_delete_selection(content_width);
        let cursor = notepad::clamp_cursor(&self.prompt, self.prompt_cursor);
        let byte_idx = self
            .prompt
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.prompt.len());
        self.prompt.insert_str(byte_idx, text);
        self.prompt_cursor = cursor + text.chars().count();
        self.sync_prompt_scroll(content_width);
    }

    pub(in crate::bar::overlay::new_session) fn prompt_backspace(&mut self, content_width: usize) {
        let cursor = notepad::clamp_cursor(&self.prompt, self.prompt_cursor);
        if cursor == 0 {
            return;
        }
        let byte_idx = self
            .prompt
            .char_indices()
            .nth(cursor - 1)
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let next_byte = self
            .prompt
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.prompt.len());
        self.prompt.replace_range(byte_idx..next_byte, "");
        self.prompt_cursor = cursor - 1;
        self.sync_prompt_scroll(content_width);
    }

    pub(in crate::bar::overlay::new_session) fn prompt_forward_delete(
        &mut self,
        content_width: usize,
    ) {
        let cursor = notepad::clamp_cursor(&self.prompt, self.prompt_cursor);
        if cursor >= self.prompt.chars().count() {
            return;
        }
        let byte_idx = self
            .prompt
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.prompt.len());
        let next_byte = self
            .prompt
            .char_indices()
            .nth(cursor + 1)
            .map(|(idx, _)| idx)
            .unwrap_or(self.prompt.len());
        self.prompt.replace_range(byte_idx..next_byte, "");
        self.sync_prompt_scroll(content_width);
    }

    pub(in crate::bar::overlay::new_session) fn move_prompt_cursor(
        &mut self,
        delta: i32,
        content_width: usize,
    ) {
        let len = self.prompt.chars().count();
        let cursor = notepad::clamp_cursor(&self.prompt, self.prompt_cursor) as i32;
        let next = (cursor + delta).clamp(0, len as i32) as usize;
        if next != self.prompt_cursor {
            self.prompt_cursor = next;
            self.sync_prompt_scroll(content_width);
        }
    }

    pub(in crate::bar::overlay::new_session) fn move_prompt_cursor_vertical(
        &mut self,
        delta: i32,
        content_width: usize,
    ) {
        let wrapped = notepad::wrapped_display_lines(&self.prompt, content_width);
        if wrapped.is_empty() {
            return;
        }
        let display_line =
            notepad::display_line_index(&self.prompt, self.prompt_cursor, content_width);
        let current = &wrapped[display_line];
        let col_in_line = self.prompt_cursor.saturating_sub(current.start);
        let target_line =
            (display_line as i32 + delta).clamp(0, wrapped.len().saturating_sub(1) as i32) as usize;
        let target = &wrapped[target_line];
        let new_col = col_in_line.min(target.text.chars().count());
        self.prompt_cursor =
            notepad::clamp_cursor(&self.prompt, target.start.saturating_add(new_col));
        self.sync_prompt_scroll(content_width);
    }

    pub(in crate::bar::overlay::new_session) fn copy_prompt_selection(&mut self) -> bool {
        let Some((start, end)) = self.prompt_selection.filter(|(start, end)| start < end) else {
            return false;
        };
        let text = notepad::selected_text(&self.prompt, start, end);
        crate::clipboard::copy(&text).is_ok()
    }

    pub(in crate::bar::overlay::new_session) fn cut_prompt_selection(
        &mut self,
        content_width: usize,
    ) -> bool {
        if self
            .prompt_selection
            .is_none_or(|(start, end)| start >= end)
        {
            return false;
        }
        if !self.copy_prompt_selection() {
            return false;
        }
        self.prompt_delete_selection(content_width)
    }

    pub(in crate::bar::overlay::new_session) fn select_prompt_all(&mut self, content_width: usize) {
        if let Some((start, end)) = notepad::select_all_range(&self.prompt) {
            self.prompt_selection = Some((start, end));
            self.prompt_cursor = end;
            self.sync_prompt_scroll(content_width);
        }
    }

    pub(in crate::bar::overlay::new_session) fn focus_prompt_at_cursor(
        &mut self,
        cursor: Option<usize>,
        content_width: usize,
    ) {
        self.set_focus(Focus::Prompt);
        if let Some(cursor) = cursor {
            self.prompt_cursor = notepad::clamp_cursor(&self.prompt, cursor);
        }
        self.sync_prompt_scroll(content_width);
    }

    pub(in crate::bar::overlay::new_session) fn focus_prompt_from_keyboard(
        &mut self,
        content_width: usize,
    ) {
        self.focus_prompt_at_cursor(None, content_width);
        self.clear_prompt_selection();
        self.prompt_drag_selecting = false;
        self.prompt_select_anchor = None;
    }

    pub(in crate::bar::overlay::new_session) fn prompt_register_click(
        &mut self,
        column: u16,
        row: u16,
    ) -> u8 {
        let count = if let Some((instant, col, y, clicks)) = self.prompt_last_click {
            if instant.elapsed() <= PROMPT_DOUBLE_CLICK_TIMEOUT && col == column && y == row {
                clicks.saturating_add(1)
            } else {
                1
            }
        } else {
            1
        };
        let count = if count > 3 { 1 } else { count };
        self.prompt_last_click = Some((Instant::now(), column, row, count));
        count
    }

    pub(in crate::bar::overlay::new_session) fn handle_prompt_body_click(
        &mut self,
        field_area: Rect,
        col: u16,
        row: u16,
        content_width: usize,
    ) {
        let click_count = self.prompt_register_click(col, row);
        let cursor = prompt_cursor_from_mouse(
            field_area,
            col,
            row,
            &self.prompt,
            self.prompt_scroll,
            content_width,
        );
        if click_count >= 3 {
            self.prompt_drag_selecting = false;
            self.prompt_select_anchor = None;
            if let Some(cursor) = cursor {
                let (line, _) = notepad::cursor_line_col(&self.prompt, cursor);
                let (start, end) = notepad::line_range_at(&self.prompt, line);
                self.prompt_selection = Some((start, end));
                self.focus_prompt_at_cursor(Some(end), content_width);
                self.copy_prompt_selection();
            }
            return;
        }
        if click_count == 2 {
            self.prompt_drag_selecting = false;
            self.prompt_select_anchor = None;
            if let Some(cursor) = cursor {
                if let Some((start, end)) = notepad::word_range_at(&self.prompt, cursor) {
                    self.prompt_selection = Some((start, end));
                    self.focus_prompt_at_cursor(Some(end), content_width);
                    self.copy_prompt_selection();
                }
            }
            return;
        }

        self.prompt_drag_selecting = true;
        self.prompt_select_anchor = cursor;
        self.prompt_selection = None;
        self.focus_prompt_at_cursor(cursor, content_width);
    }

    pub(in crate::bar::overlay::new_session) fn update_prompt_drag_selection(
        &mut self,
        field_area: Rect,
        col: u16,
        row: u16,
        content_width: usize,
    ) {
        let Some(head) = prompt_selection_cursor_from_mouse(
            field_area,
            col,
            row,
            &self.prompt,
            self.prompt_scroll,
            content_width,
        ) else {
            return;
        };
        let anchor = self.prompt_select_anchor.unwrap_or(head);
        let (start, end) = notepad::selection_range(anchor, head);
        if self.prompt_selection == Some((start, end)) && self.prompt_cursor == head {
            return;
        }
        self.prompt_selection = Some((start, end));
        self.prompt_cursor = head;
        self.sync_prompt_scroll(content_width);
    }

    pub(in crate::bar::overlay::new_session) fn finish_prompt_drag_selection(&mut self) {
        self.prompt_drag_selecting = false;
        self.prompt_select_anchor = None;
        if self
            .prompt_selection
            .is_some_and(|(start, end)| start == end)
        {
            self.prompt_selection = None;
        } else {
            self.copy_prompt_selection();
        }
    }
}

pub(in crate::bar::overlay::new_session) fn focus_next(focus: Focus) -> Focus {
    match focus {
        Focus::Agent => Focus::Model,
        Focus::Model => Focus::Workspace,
        Focus::Workspace => Focus::Prompt,
        Focus::Prompt => Focus::ForegroundButton,
        Focus::ForegroundButton => Focus::BackgroundButton,
        Focus::BackgroundButton => Focus::Agent,
    }
}

pub(in crate::bar::overlay::new_session) fn focus_prev(focus: Focus) -> Focus {
    match focus {
        Focus::Agent => Focus::BackgroundButton,
        Focus::Model => Focus::Agent,
        Focus::Workspace => Focus::Model,
        Focus::Prompt => Focus::Workspace,
        Focus::ForegroundButton => Focus::Prompt,
        Focus::BackgroundButton => Focus::ForegroundButton,
    }
}

pub(in crate::bar::overlay::new_session) fn union_rect(a: Rect, b: Rect) -> Rect {
    if a.width == 0 || a.height == 0 {
        return b;
    }
    if b.width == 0 || b.height == 0 {
        return a;
    }
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    Rect {
        x,
        y,
        width: right.saturating_sub(x),
        height: bottom.saturating_sub(y),
    }
}

pub(in crate::bar::overlay::new_session) fn load_defaults(config: &Config) -> NewSessionDefaults {
    let path = config.new_session_defaults_path();
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(defaults) = serde_json::from_str(&raw) {
            return defaults;
        }
    }
    let legacy = config.legacy_new_chat_defaults_path();
    if let Ok(raw) = std::fs::read_to_string(&legacy) {
        if let Ok(defaults) = serde_json::from_str::<NewSessionDefaults>(&raw) {
            let _ = save_defaults(config, &defaults);
            return defaults;
        }
    }
    NewSessionDefaults::default()
}

pub(in crate::bar::overlay::new_session) fn save_defaults(
    config: &Config,
    defaults: &NewSessionDefaults,
) -> Result<()> {
    let path = config.new_session_defaults_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(defaults)?;
    std::fs::write(path, raw)?;
    Ok(())
}

pub(in crate::bar::overlay::new_session) fn load_draft(config: &Config) -> Option<NewSessionDraft> {
    let path = config.new_session_draft_path();
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(in crate::bar::overlay::new_session) fn clear_draft(config: &Config) -> Result<()> {
    let path = config.new_session_draft_path();
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub(in crate::bar::overlay::new_session) fn refresh_daemon(config: &Config) {
    if crate::daemon::server::socket_responds(&config.socket_path) {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        if let Ok(mut stream) = UnixStream::connect(&config.socket_path) {
            if let Ok(line) = serde_json::to_string(&ClientCommand::Refresh) {
                let _ = stream.write_all((line + "\n").as_bytes());
                let mut reader = BufReader::new(stream);
                let mut response = String::new();
                let _ = reader.read_line(&mut response);
            }
        }
    }
}

pub(in crate::bar::overlay::new_session) fn load_workspaces(
    config: &Config,
    usage: &WorkspaceUsageStore,
    rank_mode: WorkspaceRankMode,
) -> Vec<WorkspaceChoice> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let Ok(mut stream) = UnixStream::connect(&config.socket_path) else {
        return default_workspaces();
    };
    let line = match serde_json::to_string(&ClientCommand::List) {
        Ok(line) => line + "\n",
        Err(_) => return default_workspaces(),
    };
    if stream.write_all(line.as_bytes()).is_err() {
        return default_workspaces();
    }
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    if reader.read_line(&mut response).is_err() {
        return default_workspaces();
    }
    let Ok(sessions) = serde_json::from_str::<Vec<Session>>(response.trim()) else {
        return default_workspaces();
    };

    let mut by_cwd: HashMap<String, (String, Option<chrono::DateTime<chrono::Utc>>)> =
        HashMap::new();
    for session in sessions {
        if session.cwd.is_empty() {
            continue;
        }
        let mut label = session.cwd_label.clone();
        if label.len() > 2
            && label.as_bytes()[1] == b' '
            && label.as_bytes()[0].is_ascii_alphabetic()
        {
            label = label[2..].to_string();
        }
        by_cwd
            .entry(session.cwd.clone())
            .and_modify(|(existing_label, recent)| {
                if session
                    .messaged_at
                    .is_some_and(|at| recent.is_none_or(|prev| at > prev))
                {
                    *recent = session.messaged_at;
                }
                if existing_label.is_empty() {
                    *existing_label = label.clone();
                }
            })
            .or_insert((label, session.messaged_at));
    }

    if by_cwd.is_empty() {
        return default_workspaces();
    }

    let mut workspaces: Vec<(WorkspaceChoice, Option<chrono::DateTime<chrono::Utc>>)> = by_cwd
        .into_iter()
        .map(|(cwd, (label, recent))| (WorkspaceChoice { label, cwd }, recent))
        .collect();
    workspaces.sort_by(|left, right| {
        let left_score = usage.rank_score(&left.0.cwd, left.1, rank_mode);
        let right_score = usage.rank_score(&right.0.cwd, right.1, rank_mode);
        right_score
            .cmp(&left_score)
            .then_with(|| left.0.label.cmp(&right.0.label))
    });
    workspaces
        .into_iter()
        .map(|(workspace, _)| workspace)
        .collect()
}

pub(in crate::bar::overlay::new_session) fn default_workspaces() -> Vec<WorkspaceChoice> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    vec![WorkspaceChoice {
        label: "~".into(),
        cwd: home,
    }]
}

pub(in crate::bar::overlay::new_session) fn path_entry_char(ch: char) -> bool {
    // Spaces appear in real project folder names; keep the set shell-path-ish.
    ch.is_ascii_alphanumeric() || matches!(ch, '~' | '/' | '.' | '-' | '_' | ' ' | '+')
}

pub(in crate::bar::overlay::new_session) fn expand_workspace_path(input: &str) -> Result<String> {
    crate::bar::path_picker::expand_and_validate(input)
}

pub(in crate::bar::overlay::new_session) fn completion_label_with_dir_slash(
    completion: &(String, String),
) -> String {
    crate::bar::path_picker::completion_label_with_dir_slash(completion)
}

pub(in crate::bar::overlay::new_session) fn longest_path_completion(
    input: &str,
    completions: &[(String, String)],
) -> Option<String> {
    crate::bar::path_picker::longest_path_completion(input, completions)
}

pub(in crate::bar::overlay::new_session) fn format_tilde_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    crate::bar::directory_discovery::format_tilde_path(&home, path)
}

fn prompt_field_inner(area: Rect) -> Rect {
    use ratatui::style::Style;
    use ratatui::widgets::{Block, Borders};
    Block::default()
        .borders(Borders::ALL)
        .style(Style::default())
        .inner(area)
}

fn prompt_cursor_from_mouse(
    area: Rect,
    col: u16,
    row: u16,
    text: &str,
    scroll: usize,
    content_width: usize,
) -> Option<usize> {
    let inner = prompt_field_inner(area);
    if inner.width == 0 || inner.height == 0 || !point_in_rect(col, row, inner) {
        return None;
    }
    let rel_row = row.saturating_sub(inner.y) as usize;
    let rel_col = col.saturating_sub(inner.x.saturating_add(1)) as usize;
    let display_line_idx = scroll.saturating_add(rel_row);
    let wrapped = notepad::wrapped_display_lines(text, content_width);
    let line = wrapped.get(display_line_idx)?;
    let col_in_line = rel_col.min(line.text.chars().count());
    Some(notepad::clamp_cursor(
        text,
        line.start.saturating_add(col_in_line),
    ))
}

fn prompt_selection_cursor_from_mouse(
    area: Rect,
    col: u16,
    row: u16,
    text: &str,
    scroll: usize,
    content_width: usize,
) -> Option<usize> {
    if let Some(cursor) = prompt_cursor_from_mouse(area, col, row, text, scroll, content_width) {
        return Some(cursor);
    }
    let inner = prompt_field_inner(area);
    if inner.width == 0 || inner.height == 0 || !point_in_rect(col, row, area) {
        return None;
    }
    let wrapped = notepad::wrapped_display_lines(text, content_width);
    if row < inner.y {
        let first = wrapped.first()?;
        return Some(first.start);
    }
    if row >= inner.y.saturating_add(inner.height) {
        let last = wrapped.last()?;
        return Some(last.start.saturating_add(last.text.chars().count()));
    }
    let rel_row = if row < inner.y.saturating_add(inner.height / 2) {
        0usize
    } else {
        PROMPT_INNER_HEIGHT.saturating_sub(1) as usize
    };
    let display_line_idx = scroll.saturating_add(rel_row);
    let line = wrapped.get(display_line_idx)?;
    let rel_col = col.saturating_sub(inner.x.saturating_add(1)) as usize;
    let col_in_line = if col < inner.x.saturating_add(1) {
        0
    } else {
        rel_col.min(line.text.chars().count())
    };
    Some(notepad::clamp_cursor(
        text,
        line.start.saturating_add(col_in_line),
    ))
}
