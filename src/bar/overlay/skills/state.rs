//! Skills panel state.

use crate::companions::{self, CompanionKind, SetupDialog, SetupMsg, SetupPhase};
use crate::config::Config;
use crate::skills::{
    self, detect_drift, presence_matrix_row, DriftItem, DriftKind, SkillAgent, SkillPackage,
    SkillsInventory, SkillshareStatus,
};
use anyhow::Result;
use std::sync::mpsc::Receiver;

pub const CLOSE_BUTTON_COLS: u16 = 5;
pub const CLOSE_BUTTON_LABEL: &str = "[esc]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsAction {
    Unchanged,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusSection {
    Actions,
    Library,
    Drift,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionId {
    Init,
    Sync,
    Ui,
    Audit,
    Reload,
}

impl ActionId {
    pub const ALL: [ActionId; 5] = [
        ActionId::Init,
        ActionId::Sync,
        ActionId::Ui,
        ActionId::Audit,
        ActionId::Reload,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ActionId::Init => "Init",
            ActionId::Sync => "Sync",
            ActionId::Ui => "UI",
            ActionId::Audit => "Audit",
            ActionId::Reload => "Reload",
        }
    }

    pub fn key(self) -> char {
        match self {
            ActionId::Init => 'i',
            ActionId::Sync => 's',
            ActionId::Ui => 'u',
            ActionId::Audit => 'a',
            ActionId::Reload => 'r',
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PanelHover {
    pub close: bool,
    pub action: Option<ActionId>,
    pub row: Option<usize>,
}

pub struct SkillsState {
    pub skillshare: SkillshareStatus,
    pub inventory: SkillsInventory,
    pub drift: Vec<DriftItem>,
    pub agents_in_sync: Vec<String>,
    pub selected: usize,
    pub list_scroll: usize,
    pub focus: FocusSection,
    pub action_idx: usize,
    pub status: String,
    pub busy: bool,
    pub setup: Option<SetupDialog>,
    pub setup_rx: Option<Receiver<SetupMsg>>,
}

impl SkillsState {
    pub fn load(config: &Config) -> Result<Self> {
        let mut state = Self {
            skillshare: skills::status(&config.home),
            inventory: SkillsInventory::default(),
            drift: Vec::new(),
            agents_in_sync: Vec::new(),
            selected: 0,
            list_scroll: 0,
            focus: FocusSection::Library,
            action_idx: 1, // Sync default
            status: String::new(),
            busy: false,
            setup: None,
            setup_rx: None,
        };
        state.reload(config);
        if companions::skillshare_needs_setup(&config.home) {
            state.setup = Some(SetupDialog::prompt(CompanionKind::Skillshare));
        }
        Ok(state)
    }

    pub fn reload(&mut self, config: &Config) {
        let snap = skills::snapshot(&config.home);
        self.skillshare = snap.skillshare;
        self.inventory = snap.inventory;
        self.agents_in_sync = snap.drift.agents_in_sync;
        self.drift = snap.drift.items;
        if self.selected >= self.library_len().max(1) {
            self.selected = self.library_len().saturating_sub(1);
        }
        if self.status.is_empty() {
            self.status = self.default_status();
        }
    }

    pub fn drain_setup(&mut self, config: &Config) {
        let Some(rx) = self.setup_rx.as_ref() else {
            return;
        };
        let mut msgs = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            msgs.push(msg);
        }
        let mut finished_ok = false;
        if let Some(setup) = self.setup.as_mut() {
            for msg in msgs {
                if matches!(msg, SetupMsg::Finished { success: true }) {
                    finished_ok = true;
                }
                setup.apply(msg);
            }
        }
        if finished_ok {
            self.reload(config);
            self.status = self.default_status();
        }
    }

    pub fn start_setup(&mut self, config: &Config) {
        self.setup = Some(SetupDialog {
            kind: CompanionKind::Skillshare,
            phase: SetupPhase::Running,
            lines: vec!["Starting Skills manager setup…".into()],
            scroll: 0,
        });
        self.setup_rx = Some(companions::spawn_ensure(
            &config.home,
            CompanionKind::Skillshare,
        ));
    }

    pub fn handle_setup_enter(&mut self, config: &Config) -> bool {
        let Some(setup) = self.setup.as_ref() else {
            return false;
        };
        match setup.phase {
            SetupPhase::Prompt | SetupPhase::DoneFail => {
                self.start_setup(config);
                true
            }
            SetupPhase::DoneOk => {
                self.setup = None;
                self.setup_rx = None;
                self.reload(config);
                self.status = self.default_status();
                true
            }
            SetupPhase::Running => true,
        }
    }

    pub fn handle_setup_esc(&mut self, config: &Config) -> bool {
        if self.setup.is_none() {
            return false;
        }
        let phase = self.setup.as_ref().map(|s| s.phase);
        if phase == Some(SetupPhase::DoneOk) {
            self.setup = None;
            self.setup_rx = None;
            self.reload(config);
            self.status = self.default_status();
            return true;
        }
        self.setup = None;
        self.setup_rx = None;
        self.status = if phase == Some(SetupPhase::Running) {
            "Setup continues in background — press r to reload.".into()
        } else {
            "Skills manager setup skipped — agent dirs still scanned.".into()
        };
        true
    }

    pub fn default_status(&self) -> String {
        if !self.skillshare.installed {
            return format!(
                "Skills manager not installed — press U to set up · {}",
                skills::install_hint()
            );
        }
        let n = self.inventory.store_skills.len();
        let missing = self
            .drift
            .iter()
            .filter(|d| d.kind == DriftKind::MissingOnAgent)
            .count();
        if missing > 0 {
            format!("{n} store skills · {missing} missing on agents · press s to sync")
        } else if n == 0 {
            "Manager ready · store empty — install skills then sync".into()
        } else {
            format!("{n} store skills · agents in sync")
        }
    }

    pub fn library_rows(&self) -> Vec<LibraryRow> {
        let mut rows = Vec::new();
        // Prefer store skills first
        let mut seen = std::collections::BTreeSet::new();
        for skill in &self.inventory.store_skills {
            seen.insert(skill.name.clone());
            rows.push(LibraryRow {
                name: skill.name.clone(),
                description: skill.description.clone(),
                in_store: true,
                presence: presence_matrix_row(&self.inventory, &skill.name),
            });
        }
        // Agent-only skills
        for name in &self.inventory.all_names {
            if seen.contains(name) {
                continue;
            }
            rows.push(LibraryRow {
                name: name.clone(),
                description: String::new(),
                in_store: false,
                presence: presence_matrix_row(&self.inventory, name),
            });
        }
        rows
    }

    pub fn library_len(&self) -> usize {
        self.library_rows().len()
    }

    pub fn run_action(&mut self, config: &Config, action: ActionId) {
        self.busy = true;
        let result = match action {
            ActionId::Init => skills::run_init(),
            ActionId::Sync => skills::run_sync(),
            ActionId::Ui => skills::run_ui(),
            ActionId::Audit => skills::run_audit(),
            ActionId::Reload => {
                self.reload(config);
                self.status = "Reloaded inventory".into();
                self.busy = false;
                return;
            }
        };
        match result {
            Ok(r) => {
                let msg = if r.ok {
                    let out = r.stdout.trim();
                    if out.is_empty() {
                        format!("{} ok", action.label())
                    } else {
                        truncate(out, 120)
                    }
                } else {
                    let err = r.stderr.trim();
                    if err.is_empty() {
                        format!("{} failed (exit {:?})", action.label(), r.code)
                    } else {
                        format!("{} failed: {}", action.label(), truncate(err, 100))
                    }
                };
                self.status = msg;
            }
            Err(e) => {
                self.status = e;
            }
        }
        self.reload(config);
        self.busy = false;
    }

    pub fn missing_drift(&self) -> Vec<&DriftItem> {
        self.drift
            .iter()
            .filter(|d| d.kind == DriftKind::MissingOnAgent)
            .collect()
    }

    pub fn store_skill_count(&self) -> usize {
        self.inventory.store_skills.len()
    }

    #[allow(dead_code)]
    pub fn store_skills(&self) -> &[SkillPackage] {
        &self.inventory.store_skills
    }
}

#[derive(Debug, Clone)]
pub struct LibraryRow {
    pub name: String,
    pub description: String,
    pub in_store: bool,
    pub presence: Vec<(SkillAgent, bool)>,
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Re-export for render if needed without re-importing skills.
pub fn refresh_drift(home: &std::path::Path, inventory: &SkillsInventory) -> skills::DriftReport {
    detect_drift(home, inventory)
}
