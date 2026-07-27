//! Automations panel state.

use crate::agents::{self, AgentEntry};
use crate::automation::{
    self, humanize_schedule, next_fire_after, prompt_result_hint, slugify_id, Automation,
    AutomationRun, AutomationStatus, SchedulePreset,
};
use crate::bar::path_picker::PathPickerState;
use crate::config::Config;
use anyhow::Result;
use chrono::{DateTime, Local, Utc};

pub const CLOSE_BUTTON_COLS: u16 = 5;
pub const CLOSE_BUTTON_LABEL: &str = "[esc]";
pub const FIELD_INNER_HEIGHT: u16 = 1;
pub const PROMPT_INNER_HEIGHT: u16 = 5;
pub const SECTION_GAP: u16 = 1;
pub const TITLE_ROWS: u16 = 2;
pub const MAX_DROPDOWN_VISIBLE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationsAction {
    Unchanged,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    List,
    Editor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFilter {
    All,
    Active,
    Paused,
    Runs,
}

impl ListFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Active => "Active",
            Self::Paused => "Paused",
            Self::Runs => "Runs",
        }
    }

    pub fn all() -> &'static [ListFilter] {
        &[Self::All, Self::Active, Self::Paused, Self::Runs]
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::All => Self::Active,
            Self::Active => Self::Paused,
            Self::Paused => Self::Runs,
            Self::Runs => Self::All,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorFocus {
    Name,
    Cwd,
    Agent,
    Model,
    Schedule,
    Prompt,
    Save,
    SaveRun,
    Cancel,
}

impl EditorFocus {
    pub fn is_dropdown(self) -> bool {
        matches!(self, Self::Cwd | Self::Agent | Self::Model | Self::Schedule)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PanelHover {
    pub close: bool,
    pub filter: Option<ListFilter>,
    pub row: Option<usize>,
    pub new_btn: bool,
    pub run_btn: bool,
    pub pause_btn: bool,
    pub edit_btn: bool,
    pub save_btn: bool,
    pub save_run_btn: bool,
    pub cancel_btn: bool,
    pub dropdown_item: Option<usize>,
}

pub struct AutomationsState {
    pub mode: Mode,
    pub filter: ListFilter,
    pub items: Vec<Automation>,
    pub runs: Vec<AutomationRun>,
    pub selected: usize,
    pub list_scroll: usize,
    pub status: String,
    pub unread: usize,
    pub salt: String,
    /// Shared path picker (same intelligence as New Session → Session Path).
    pub path: PathPickerState,
    pub editing_id: Option<String>,
    pub name: String,
    pub name_cursor: usize,
    pub agent_idx: usize,
    pub model_idx: usize,
    pub schedule_idx: usize,
    pub prompt: String,
    pub prompt_cursor: usize,
    pub prompt_scroll: u16,
    pub editor_focus: EditorFocus,
    pub dropdown_open: bool,
}

impl AutomationsState {
    pub fn load(config: &Config) -> Result<Self> {
        let salt = automation::load_or_create_jitter_salt(config).unwrap_or_default();
        let mut state = Self {
            mode: Mode::List,
            filter: ListFilter::All,
            items: Vec::new(),
            runs: Vec::new(),
            selected: 0,
            list_scroll: 0,
            status: String::new(),
            unread: 0,
            salt,
            path: PathPickerState::load(config),
            editing_id: None,
            name: String::new(),
            name_cursor: 0,
            agent_idx: 0,
            model_idx: 0,
            schedule_idx: 2,
            prompt: String::new(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            editor_focus: EditorFocus::Name,
            dropdown_open: false,
        };
        state.reload(config)?;
        Ok(state)
    }

    pub fn reload(&mut self, config: &Config) -> Result<()> {
        let _ = automation::ensure_root(config);
        self.items = automation::list_automations(config).unwrap_or_default();
        self.runs = automation::list_all_runs(config, 40).unwrap_or_default();
        self.unread = automation::unread_count(config).unwrap_or(0);
        self.path.refresh_sources(config);
        let max = self.list_len().saturating_sub(1);
        if self.selected > max {
            self.selected = max;
        }
        Ok(())
    }

    pub fn filtered_items(&self) -> Vec<&Automation> {
        self.items
            .iter()
            .filter(|a| match self.filter {
                ListFilter::All | ListFilter::Runs => true,
                ListFilter::Active => a.is_active(),
                ListFilter::Paused => matches!(a.status, AutomationStatus::Paused),
            })
            .collect()
    }

    pub fn list_len(&self) -> usize {
        match self.filter {
            ListFilter::Runs => self.runs.len(),
            _ => self.filtered_items().len(),
        }
    }

    pub fn agent_choices(&self) -> Vec<&'static AgentEntry> {
        agents::AGENTS
            .iter()
            .filter(|a| a.id != "console")
            .collect()
    }

    pub fn selected_agent(&self) -> &'static AgentEntry {
        let choices = self.agent_choices();
        choices
            .get(self.agent_idx.min(choices.len().saturating_sub(1)))
            .copied()
            .unwrap_or(&agents::AGENTS[0])
    }

    pub fn selected_model_id(&self) -> &str {
        let agent = self.selected_agent();
        agent
            .models
            .get(self.model_idx.min(agent.models.len().saturating_sub(1)))
            .map(|m| m.id)
            .unwrap_or(agent.default_model)
    }

    pub fn selected_model_label(&self) -> &str {
        let agent = self.selected_agent();
        agent
            .models
            .get(self.model_idx.min(agent.models.len().saturating_sub(1)))
            .map(|m| m.label)
            .unwrap_or(agent.default_model)
    }

    pub fn schedule_presets() -> &'static [SchedulePreset] {
        SchedulePreset::all()
    }

    pub fn selected_schedule_label(&self) -> String {
        Self::schedule_presets()[self.schedule_idx.min(Self::schedule_presets().len() - 1)]
            .label()
            .to_string()
    }

    pub fn selected_rrule(&self) -> String {
        let presets = Self::schedule_presets();
        let p = presets[self.schedule_idx.min(presets.len() - 1)];
        p.rrule().to_string()
    }

    pub fn confirm_path_selection(&mut self) -> bool {
        match self.path.confirm(true) {
            Ok(_) => {
                self.dropdown_open = false;
                self.status.clear();
                true
            }
            Err(msg) => {
                self.status = msg;
                false
            }
        }
    }

    pub fn next_run_label(&self, automation: &Automation) -> String {
        if !automation.is_active() {
            return "paused".into();
        }
        match next_fire_after(automation, Utc::now(), &self.salt) {
            Ok(Some(t)) => format_next(t),
            Ok(None) => "—".into(),
            Err(_) => "invalid schedule".into(),
        }
    }

    pub fn open_create(&mut self) {
        self.mode = Mode::Editor;
        self.editing_id = None;
        self.name.clear();
        self.name_cursor = 0;
        self.path.reset_to_default();
        self.agent_idx = 0;
        self.model_idx = 0;
        self.schedule_idx = 2;
        self.prompt.clear();
        self.prompt_cursor = 0;
        self.prompt_scroll = 0;
        self.editor_focus = EditorFocus::Name;
        self.dropdown_open = false;
        self.status.clear();
    }

    pub fn open_edit(&mut self, automation: &Automation) {
        self.mode = Mode::Editor;
        self.editing_id = Some(automation.id.clone());
        self.name = automation.name.clone();
        self.name_cursor = self.name.len();
        let cwd = automation.primary_cwd().unwrap_or("~/");
        self.path.set_path(cwd);
        let choices = self.agent_choices();
        self.agent_idx = choices
            .iter()
            .position(|a| a.id == automation.agent)
            .unwrap_or(0);
        self.sync_model_idx(&automation.model);
        self.schedule_idx = SchedulePreset::from_rrule(&automation.rrule)
            .and_then(|preset| Self::schedule_presets().iter().position(|p| *p == preset))
            .unwrap_or(2);
        self.prompt = automation.prompt.clone();
        self.prompt_cursor = self.prompt.len();
        self.prompt_scroll = 0;
        self.editor_focus = EditorFocus::Name;
        self.dropdown_open = false;
        self.status.clear();
    }

    fn sync_model_idx(&mut self, model_id: &str) {
        let agent = self.selected_agent();
        self.model_idx = agent
            .models
            .iter()
            .position(|m| m.id == model_id)
            .unwrap_or(0);
    }

    pub fn cancel_editor(&mut self) {
        self.mode = Mode::List;
        self.dropdown_open = false;
        self.path.close_menu();
        self.status.clear();
    }

    pub fn set_focus(&mut self, focus: EditorFocus) {
        if self.editor_focus == EditorFocus::Cwd && focus != EditorFocus::Cwd {
            self.path.confirm_on_blur();
            self.dropdown_open = false;
            self.path.close_menu();
        } else if self.editor_focus != focus {
            self.dropdown_open = false;
            self.path.close_menu();
        }
        self.editor_focus = focus;
    }

    pub fn save(&mut self, config: &Config, run_now: bool) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() {
            self.status = "name is required".into();
            self.editor_focus = EditorFocus::Name;
            return Ok(());
        }
        let prompt = self.prompt.trim();
        if prompt.is_empty() {
            self.status = "prompt is required".into();
            self.editor_focus = EditorFocus::Prompt;
            return Ok(());
        }
        let cwd = match self.path.resolved_cwd() {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("{e}");
                self.editor_focus = EditorFocus::Cwd;
                self.dropdown_open = true;
                self.path.open_menu();
                return Ok(());
            }
        };
        if !std::path::Path::new(&cwd).is_dir() {
            self.status = format!("directory not found: {cwd}");
            self.editor_focus = EditorFocus::Cwd;
            self.dropdown_open = true;
            self.path.open_menu();
            return Ok(());
        }
        self.path.record_usage(config);
        let agent = self.selected_agent().id.to_string();
        let model = self.selected_model_id().to_string();
        let rrule = self.selected_rrule();

        let id = self.editing_id.clone().unwrap_or_else(|| slugify_id(name));

        let mut a = if let Ok(existing) = automation::load_automation(config, &id) {
            let mut a = existing;
            a.name = name.to_string();
            a.prompt = prompt.to_string();
            a.agent = agent;
            a.model = model;
            a.rrule = rrule;
            a.cwds = vec![cwd];
            a.touch();
            a
        } else {
            Automation::new(
                id.clone(),
                name.to_string(),
                prompt.to_string(),
                agent,
                model,
                rrule,
                cwd,
            )
        };

        let salt = automation::load_or_create_jitter_salt(config)?;
        if let Err(e) = next_fire_after(&a, Utc::now(), &salt) {
            self.status = format!("invalid schedule: {e}");
            self.editor_focus = EditorFocus::Schedule;
            return Ok(());
        }

        if !a.prompt.contains("AUTOMATION_RESULT:") && self.editing_id.is_none() {
            a.prompt.push_str(prompt_result_hint());
        }

        automation::save_automation(config, &a)?;
        self.status = format!("saved {}", a.id);

        if run_now {
            match automation::fire_automation(config, &a, false) {
                Ok(run) => self.status = format!("saved · started {}", short_id(&run.id)),
                Err(e) => self.status = format!("saved · run failed: {e}"),
            }
        }

        self.mode = Mode::List;
        self.dropdown_open = false;
        self.reload(config)?;
        if let Some(idx) = self.filtered_items().iter().position(|x| x.id == a.id) {
            self.selected = idx;
        }
        Ok(())
    }

    pub fn toggle_pause(&mut self, config: &Config) -> Result<()> {
        let Some(a) = self
            .filtered_items()
            .get(self.selected)
            .map(|a| (*a).clone())
        else {
            return Ok(());
        };
        let mut a = a;
        a.status = match a.status {
            AutomationStatus::Active => AutomationStatus::Paused,
            AutomationStatus::Paused => AutomationStatus::Active,
        };
        a.touch();
        automation::save_automation(config, &a)?;
        self.status = match a.status {
            AutomationStatus::Active => format!("{} resumed", a.name),
            AutomationStatus::Paused => format!("{} paused", a.name),
        };
        self.reload(config)?;
        Ok(())
    }

    pub fn run_selected(&mut self, config: &Config) -> Result<()> {
        let Some(a) = self
            .filtered_items()
            .get(self.selected)
            .map(|a| (*a).clone())
        else {
            self.status = "no automation selected".into();
            return Ok(());
        };
        match automation::fire_automation(config, &a, false) {
            Ok(run) => {
                self.status = format!("started {}", short_id(&run.id));
                self.reload(config)?;
            }
            Err(e) => self.status = format!("{e}"),
        }
        Ok(())
    }

    pub fn delete_selected(&mut self, config: &Config) -> Result<()> {
        let Some((id, name)) = self
            .filtered_items()
            .get(self.selected)
            .map(|a| (a.id.clone(), a.name.clone()))
        else {
            return Ok(());
        };
        automation::delete_automation(config, &id)?;
        self.status = format!("deleted {name}");
        self.reload(config)?;
        Ok(())
    }

    pub fn mark_all_read(&mut self, config: &Config) -> Result<()> {
        let n = automation::mark_all_read(config)?;
        self.status = if n == 0 {
            "nothing unread".into()
        } else {
            format!("marked {n} read")
        };
        self.reload(config)?;
        Ok(())
    }

    pub fn apply_paste(&mut self, text: &str) {
        match self.editor_focus {
            EditorFocus::Name if self.mode == Mode::Editor => {
                self.name.insert_str(self.name_cursor, text);
                self.name_cursor += text.len();
            }
            EditorFocus::Cwd if self.mode == Mode::Editor => {
                let sanitized = crate::clipboard::sanitize_paste_text(text, false);
                self.path.apply_paste(&sanitized);
                self.dropdown_open = true;
            }
            EditorFocus::Prompt if self.mode == Mode::Editor => {
                self.prompt.insert_str(self.prompt_cursor, text);
                self.prompt_cursor += text.len();
            }
            _ => {}
        }
    }

    pub fn ensure_list_scroll(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        if self.selected < self.list_scroll {
            self.list_scroll = self.selected;
        } else if self.selected >= self.list_scroll + visible_rows {
            self.list_scroll = self.selected + 1 - visible_rows;
        }
    }
}

fn format_next(t: DateTime<Utc>) -> String {
    let local = t.with_timezone(&Local);
    let now = Local::now();
    if local.date_naive() == now.date_naive() {
        format!("today {}", local.format("%H:%M"))
    } else if local.date_naive() == now.date_naive() + chrono::Duration::days(1) {
        format!("tomorrow {}", local.format("%H:%M"))
    } else {
        local.format("%a %H:%M").to_string()
    }
}

fn short_id(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

pub fn human_status(automation: &Automation) -> &'static str {
    match automation.status {
        AutomationStatus::Active => "active",
        AutomationStatus::Paused => "paused",
    }
}

pub fn human_run_status(run: &AutomationRun) -> String {
    use crate::automation::RunStatus;
    let base = match run.status {
        RunStatus::Pending => "pending",
        RunStatus::Running => "running",
        RunStatus::Done => "done",
        RunStatus::Failed => "failed",
        RunStatus::Archived => "archived",
    };
    if run.unread {
        format!("{base} · unread")
    } else {
        base.into()
    }
}

pub fn schedule_summary(automation: &Automation) -> String {
    humanize_schedule(automation)
}
