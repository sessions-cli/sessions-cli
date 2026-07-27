//! MCP management panel state — wired to `crate::mcp` domain facade.

use crate::companions::{self, CompanionKind, SetupDialog, SetupMsg, SetupPhase};
use crate::config::Config;
use crate::hooks;
use crate::mcp::{
    self, CatalogEntryView, DriftKind, EnablementMatrix, ObotHealthStatus, ServerSource,
};
use anyhow::Result;
use std::sync::mpsc::Receiver;

pub const CLOSE_BUTTON_COLS: u16 = 5;
pub const CLOSE_BUTTON_LABEL: &str = "[esc]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpsAction {
    Unchanged,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusZone {
    Table,
    Drift,
    Actions,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionButton {
    OpenObot,
    Search,
    Refresh,
    SyncAll,
    DryRun,
}

impl ActionButton {
    pub fn all() -> &'static [ActionButton] {
        &[
            Self::OpenObot,
            Self::Search,
            Self::Refresh,
            Self::SyncAll,
            Self::DryRun,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenObot => "Catalog",
            Self::Search => "Search",
            Self::Refresh => "Refresh",
            Self::SyncAll => "Sync all",
            Self::DryRun => "Dry-run",
        }
    }

    pub fn cycle(self, delta: i32) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|b| *b == self).unwrap_or(0) as i32;
        let n = all.len() as i32;
        let next = (idx + delta).rem_euclid(n) as usize;
        all[next]
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PanelHover {
    pub close: bool,
    pub open_obot: bool,
    pub search: bool,
    pub refresh: bool,
    pub sync_all: bool,
    pub dry_run: bool,
    pub row: Option<usize>,
    pub agent_col: Option<usize>,
    pub search_row: Option<usize>,
}

/// One row in catalog search results.
#[derive(Debug, Clone)]
pub struct SearchResultRow {
    pub entry: CatalogEntryView,
    /// Already present in installed inventory (matched by key/name).
    pub installed: bool,
}

#[derive(Debug, Clone)]
pub struct AgentColumn {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct ServerRow {
    pub key: String,
    pub display_name: String,
    pub source: String,
    pub auth: String,
    /// Per-agent enabled flags aligned with `McpsState::agents`.
    pub enabled: Vec<bool>,
}

#[derive(Debug, Clone)]
pub struct DriftRow {
    pub detail: String,
}

pub struct McpsState {
    pub agents: Vec<AgentColumn>,
    pub servers: Vec<ServerRow>,
    pub drift: Vec<DriftRow>,
    pub matrix: EnablementMatrix,
    pub selected_row: usize,
    pub selected_agent: usize,
    pub selected_drift: usize,
    pub list_scroll: usize,
    pub focus: FocusZone,
    pub action_focus: ActionButton,
    pub obot_status: String,
    pub obot_url: String,
    pub obot_up: bool,
    pub backend_ready: bool,
    pub status: String,
    pub last_sync: String,
    pub staged_changes: usize,
    /// Optional setup dialog when the MCP manager is not running.
    pub setup: Option<SetupDialog>,
    pub setup_rx: Option<Receiver<SetupMsg>>,
    /// Catalog search mode (`/` or Search button).
    pub search_open: bool,
    pub search_query: String,
    pub search_results: Vec<SearchResultRow>,
    pub search_selected: usize,
    pub search_scroll: usize,
    /// Full catalog cache (refreshed when search opens / on explicit reload).
    pub catalog_cache: Vec<CatalogEntryView>,
    pub catalog_loaded: bool,
    pub catalog_error: String,
    /// Busy flag while create-from-catalog is in flight (status messaging).
    pub search_busy: bool,
}

impl McpsState {
    pub fn load(config: &Config) -> Result<Self> {
        let agents = detected_agent_columns(config);
        let mut state = Self {
            agents,
            servers: Vec::new(),
            drift: Vec::new(),
            matrix: EnablementMatrix::new(),
            selected_row: 0,
            selected_agent: 0,
            selected_drift: 0,
            list_scroll: 0,
            focus: FocusZone::Table,
            action_focus: ActionButton::SyncAll,
            obot_status: "unknown".into(),
            obot_url: "http://127.0.0.1:8080".into(),
            obot_up: false,
            backend_ready: true,
            status: String::new(),
            last_sync: "never".into(),
            staged_changes: 0,
            setup: None,
            setup_rx: None,
            search_open: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_selected: 0,
            search_scroll: 0,
            catalog_cache: Vec::new(),
            catalog_loaded: false,
            catalog_error: String::new(),
            search_busy: false,
        };
        state.refresh(config);
        if companions::obot_needs_setup(&config.home) {
            state.setup = Some(SetupDialog::prompt(CompanionKind::Obot));
        }
        Ok(state)
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
            // Keep dialog until user presses Enter, but refresh data underneath.
            self.refresh(config);
        }
    }

    pub fn start_setup(&mut self, config: &Config) {
        self.setup = Some(SetupDialog {
            kind: CompanionKind::Obot,
            phase: SetupPhase::Running,
            lines: vec!["Starting MCP manager setup…".into()],
            scroll: 0,
        });
        self.setup_rx = Some(companions::spawn_ensure(&config.home, CompanionKind::Obot));
    }

    /// Handle Enter while setup dialog is open. Returns true if consumed.
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
                self.refresh(config);
                true
            }
            SetupPhase::Running => true, // ignore Enter while running
        }
    }

    /// Handle Esc while setup dialog is open. Returns true if dialog dismissed
    /// (caller should not close the whole panel).
    pub fn handle_setup_esc(&mut self, config: &Config) -> bool {
        if self.setup.is_none() {
            return false;
        }
        let phase = self.setup.as_ref().map(|s| s.phase);
        if phase == Some(SetupPhase::Running) {
            // Keep running in background; just hide dialog.
            self.setup = None;
            self.setup_rx = None;
            self.status = "Setup continues in background — press r to refresh.".into();
            return true;
        }
        if phase == Some(SetupPhase::DoneOk) {
            self.setup = None;
            self.setup_rx = None;
            self.refresh(config);
            return true;
        }
        // Prompt / fail → skip
        self.setup = None;
        self.setup_rx = None;
        self.status = "MCP manager setup skipped — local inventory only.".into();
        true
    }

    pub fn refresh(&mut self, config: &Config) {
        self.agents = detected_agent_columns(config);
        self.backend_ready = true;

        match mcp::load_obot_config(&config.home) {
            Ok(cfg) => {
                self.obot_url = cfg.base_url.clone();
            }
            Err(err) => {
                self.status = format!("obot config: {err}");
            }
        }

        match mcp::health(&config.home) {
            Ok(h) => {
                self.obot_url = h.base_url.clone();
                match h.status {
                    ObotHealthStatus::Up => {
                        self.obot_up = true;
                        self.obot_status = "running".into();
                    }
                    ObotHealthStatus::Down => {
                        self.obot_up = false;
                        self.obot_status = "down".into();
                    }
                    ObotHealthStatus::Disabled => {
                        self.obot_up = false;
                        self.obot_status = "disabled".into();
                    }
                }
                if !h.detail.is_empty() && !self.obot_up {
                    self.status = h.detail;
                }
            }
            Err(err) => {
                self.obot_up = false;
                self.obot_status = "error".into();
                self.status = format!("health: {err}");
            }
        }

        self.matrix = mcp::load_enablement(&config.home).unwrap_or_default();

        let inventory = match mcp::list_inventory(&config.home) {
            Ok(list) => list,
            Err(err) => {
                self.status = format!("inventory: {err}");
                Vec::new()
            }
        };

        // agent_id → set of MCP keys present in that agent's config (import defaults).
        let present_keys = present_keys_by_agent(&config.home, &self.agents);

        self.servers = inventory
            .iter()
            .map(|server| {
                let source = match &server.source {
                    ServerSource::ObotGateway { .. } => "gateway".into(),
                    ServerSource::LocalOnly { .. } => "local".into(),
                };
                let auth = match server.oauth_ok {
                    Some(true) => "✓".into(),
                    Some(false) => "! needs".into(),
                    None => "—".into(),
                };
                let enabled = self
                    .agents
                    .iter()
                    .map(|agent| {
                        let default_if_absent = present_keys
                            .get(&agent.id)
                            .is_some_and(|keys| keys.contains(&server.key));
                        self.matrix
                            .enabled_or(&server.key, &agent.id, default_if_absent)
                    })
                    .collect();
                ServerRow {
                    key: server.key.clone(),
                    display_name: server.display_name.clone(),
                    source,
                    auth,
                    enabled,
                }
            })
            .collect();

        self.drift = match mcp::detect_drift(&config.home) {
            Ok(items) => items
                .into_iter()
                .map(|d| DriftRow {
                    detail: format!(
                        "{}  {}  {}",
                        d.agent_id,
                        d.server_key,
                        drift_kind_label(d.kind, &d.detail)
                    ),
                })
                .collect(),
            Err(err) => {
                if self.status.is_empty() {
                    self.status = format!("drift: {err}");
                }
                Vec::new()
            }
        };

        if self.selected_row >= self.servers.len() {
            self.selected_row = self.servers.len().saturating_sub(1);
        }
        if self.selected_agent >= self.agents.len() {
            self.selected_agent = self.agents.len().saturating_sub(1);
        }
        if self.selected_drift >= self.drift.len() {
            self.selected_drift = self.drift.len().saturating_sub(1);
        }

        if self.servers.is_empty() && self.status.is_empty() {
            self.status = if self.obot_up {
                "No servers yet — add in Catalog, then Refresh.".into()
            } else {
                "MCP manager unreachable — local inventory only. Use Catalog or set up the manager."
                    .into()
            };
        } else if self.status.is_empty() {
            self.status = format!(
                "{} server(s) · {} drift · agents: {}",
                self.servers.len(),
                self.drift.len(),
                self.agents_summary()
            );
        }
        self.recount_staged();
    }

    fn recount_staged(&mut self) {
        self.staged_changes = self
            .servers
            .iter()
            .flat_map(|s| s.enabled.iter())
            .filter(|&&e| e)
            .count();
    }

    pub fn agents_summary(&self) -> String {
        if self.agents.is_empty() {
            "none detected".into()
        } else {
            self.agents
                .iter()
                .map(|a| a.label.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        }
    }

    pub fn move_row(&mut self, delta: i32) {
        if self.servers.is_empty() {
            return;
        }
        let n = self.servers.len() as i32;
        let next = (self.selected_row as i32 + delta).clamp(0, n - 1);
        self.selected_row = next as usize;
    }

    pub fn move_agent(&mut self, delta: i32) {
        if self.agents.is_empty() {
            return;
        }
        let n = self.agents.len() as i32;
        let next = (self.selected_agent as i32 + delta).clamp(0, n - 1);
        self.selected_agent = next as usize;
    }

    pub fn move_drift(&mut self, delta: i32) {
        if self.drift.is_empty() {
            return;
        }
        let n = self.drift.len() as i32;
        let next = (self.selected_drift as i32 + delta).clamp(0, n - 1);
        self.selected_drift = next as usize;
    }

    pub fn cycle_focus(&mut self) {
        if self.search_open {
            self.focus = FocusZone::Search;
            return;
        }
        self.focus = match self.focus {
            FocusZone::Table => FocusZone::Drift,
            FocusZone::Drift => FocusZone::Actions,
            FocusZone::Actions => FocusZone::Table,
            FocusZone::Search => FocusZone::Table,
        };
    }

    pub fn open_search(&mut self, config: &Config) {
        self.search_open = true;
        self.focus = FocusZone::Search;
        self.search_selected = 0;
        self.search_scroll = 0;
        if !self.catalog_loaded {
            self.load_catalog(config);
        } else {
            self.apply_search_filter();
        }
        self.status = "Search catalog — type to filter · Enter add · esc back".into();
    }

    pub fn close_search(&mut self) {
        self.search_open = false;
        self.focus = FocusZone::Table;
        self.search_busy = false;
        if self.status.starts_with("Search") || self.status.starts_with("Added ") {
            // keep "Added …" messages; clear pure search hints
            if self.status.starts_with("Search") {
                self.status.clear();
            }
        }
    }

    pub fn load_catalog(&mut self, config: &Config) {
        self.catalog_error.clear();
        if !self.obot_up {
            self.catalog_cache.clear();
            self.catalog_loaded = true;
            self.catalog_error =
                "MCP manager unreachable — set up the manager or open Catalog in browser.".into();
            self.apply_search_filter();
            return;
        }
        match mcp::list_catalog_entries(&config.home) {
            Ok(entries) => {
                self.catalog_cache = entries;
                self.catalog_loaded = true;
                if self.catalog_cache.is_empty() {
                    // Fallback: registry search API (may still return nothing offline).
                    if let Ok(reg) = mcp::search_registry(&config.home, "", 50) {
                        if !reg.is_empty() {
                            self.catalog_cache = reg;
                        }
                    }
                }
                self.apply_search_filter();
                self.status = format!(
                    "Catalog: {} entr{}. Type to filter.",
                    self.catalog_cache.len(),
                    if self.catalog_cache.len() == 1 {
                        "y"
                    } else {
                        "ies"
                    }
                );
            }
            Err(err) => {
                self.catalog_cache.clear();
                self.catalog_loaded = true;
                self.catalog_error = err.to_string();
                self.search_results.clear();
                self.status = format!("Catalog load failed: {err}");
            }
        }
    }

    pub fn apply_search_filter(&mut self) {
        let q = self.search_query.trim().to_ascii_lowercase();
        let installed_keys: std::collections::HashSet<String> = self
            .servers
            .iter()
            .flat_map(|s| {
                [
                    s.key.to_ascii_lowercase(),
                    s.display_name.to_ascii_lowercase(),
                ]
            })
            .collect();

        let mut results: Vec<SearchResultRow> = self
            .catalog_cache
            .iter()
            .filter(|e| {
                if q.is_empty() {
                    return true;
                }
                e.name.to_ascii_lowercase().contains(&q)
                    || e.short_description.to_ascii_lowercase().contains(&q)
                    || e.description.to_ascii_lowercase().contains(&q)
                    || e.id.to_ascii_lowercase().contains(&q)
            })
            .map(|e| {
                let key = e.suggested_key().to_ascii_lowercase();
                let name = e.name.to_ascii_lowercase();
                let installed = installed_keys.contains(&key) || installed_keys.contains(&name);
                SearchResultRow {
                    entry: e.clone(),
                    installed,
                }
            })
            .collect();

        // When catalog is empty/unreachable, still allow filtering installed inventory.
        if results.is_empty() && !self.servers.is_empty() {
            results = self
                .servers
                .iter()
                .filter(|s| {
                    if q.is_empty() {
                        return true;
                    }
                    s.display_name.to_ascii_lowercase().contains(&q)
                        || s.key.to_ascii_lowercase().contains(&q)
                })
                .map(|s| SearchResultRow {
                    entry: CatalogEntryView {
                        id: s.key.clone(),
                        name: s.display_name.clone(),
                        short_description: format!("installed · {}", s.source),
                        description: String::new(),
                        user_type: String::new(),
                        catalog_name: "installed".into(),
                        connect_url: String::new(),
                        oauth_configured: true,
                    },
                    installed: true,
                })
                .collect();
        }

        results.sort_by(|a, b| {
            // Not-yet-installed first, then name.
            a.installed.cmp(&b.installed).then_with(|| {
                a.entry
                    .name
                    .to_ascii_lowercase()
                    .cmp(&b.entry.name.to_ascii_lowercase())
            })
        });

        self.search_results = results;
        if self.search_selected >= self.search_results.len() {
            self.search_selected = self.search_results.len().saturating_sub(1);
        }
    }

    pub fn move_search(&mut self, delta: i32) {
        if self.search_results.is_empty() {
            return;
        }
        let n = self.search_results.len() as i32;
        let next = (self.search_selected as i32 + delta).clamp(0, n - 1);
        self.search_selected = next as usize;
    }

    pub fn push_search_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        self.search_query.push(c);
        self.search_selected = 0;
        self.apply_search_filter();
    }

    pub fn pop_search_char(&mut self) {
        self.search_query.pop();
        self.search_selected = 0;
        self.apply_search_filter();
    }

    /// Activate selected search result: add from catalog, or jump to installed row.
    pub fn activate_search_selection(&mut self, config: &Config) {
        let Some(row) = self.search_results.get(self.search_selected).cloned() else {
            self.status = "No search result selected.".into();
            return;
        };
        if row.installed {
            // Jump to installed server in main table.
            if let Some(idx) = self
                .servers
                .iter()
                .position(|s| s.key == row.entry.id || s.display_name == row.entry.name)
            {
                self.selected_row = idx;
            }
            self.close_search();
            self.status = format!("Selected installed server: {}", row.entry.name);
            return;
        }
        if row.entry.catalog_name == "installed" {
            self.close_search();
            return;
        }
        if !self.obot_up {
            self.status =
                "Cannot add — MCP manager is down. Press u to set up or o for Catalog.".into();
            return;
        }
        if !row.entry.oauth_configured {
            // Needs admin OAuth — open browser catalog.
            self.open_obot(config);
            self.status = format!(
                "{} needs configuration in Catalog (OAuth) — opened browser.",
                row.entry.name
            );
            return;
        }

        self.search_busy = true;
        let alias = row.entry.suggested_key();
        let result =
            mcp::create_server_from_entry(&config.home, &row.entry.id, Some(alias.as_str()));
        self.search_busy = false;
        match result {
            Ok(created) => {
                let name = created.display_name.clone();
                // Enable for all detected agents by default so Sync can wire it.
                for agent in &self.agents {
                    self.matrix.set(&created.key, &agent.id, true);
                }
                let _ = mcp::save_enablement(&config.home, &self.matrix);
                self.refresh(config);
                self.catalog_loaded = false; // refresh catalog next open
                if created.missing_oauth || !created.configured {
                    self.open_obot(config);
                    self.status = format!(
                        "Added {name} — needs config/OAuth in Catalog. Enablement staged; Sync after configure."
                    );
                } else {
                    self.status = format!(
                        "Added {name} ({}). Toggled on for agents — press s to Sync.",
                        created.key
                    );
                }
                self.apply_search_filter();
            }
            Err(err) => {
                let msg = err.to_string();
                // Common case: needs interactive config — open browser.
                if msg.to_ascii_lowercase().contains("oauth")
                    || msg.to_ascii_lowercase().contains("configur")
                    || msg.to_ascii_lowercase().contains("required")
                {
                    self.open_obot(config);
                    self.status = format!("Add needs Catalog: {msg}");
                } else {
                    self.status = format!("Add failed: {msg}");
                }
            }
        }
    }

    pub fn toggle_selected_cell(&mut self) {
        if self.servers.is_empty() || self.agents.is_empty() {
            return;
        }
        let row = self.selected_row;
        let col = self.selected_agent;
        self.toggle_cell(row, col);
    }

    pub fn toggle_cell(&mut self, row: usize, agent_col: usize) {
        let Some(server) = self.servers.get_mut(row) else {
            return;
        };
        let Some(flag) = server.enabled.get_mut(agent_col) else {
            return;
        };
        *flag = !*flag;
        let enabled = *flag;
        let key = server.key.clone();
        let name = server.display_name.clone();
        let agent_id = self
            .agents
            .get(agent_col)
            .map(|a| a.id.clone())
            .unwrap_or_default();
        let agent_label = self
            .agents
            .get(agent_col)
            .map(|a| a.label.as_str())
            .unwrap_or("?")
            .to_string();
        self.matrix.set(&key, &agent_id, enabled);
        self.selected_row = row;
        self.selected_agent = agent_col;
        self.focus = FocusZone::Table;
        self.recount_staged();
        self.status = format!(
            "Toggled {name} for {agent_label} → {} (not written until Sync)",
            if enabled { "on" } else { "off" }
        );
    }

    pub fn persist_matrix(&mut self, config: &Config) {
        if let Err(err) = mcp::save_enablement(&config.home, &self.matrix) {
            self.status = format!("save enablement: {err}");
        }
    }

    pub fn open_obot(&mut self, config: &Config) {
        let url = mcp::open_admin_url(&config.home).unwrap_or_else(|_| self.obot_url.clone());
        let _ = std::process::Command::new("open").arg(&url).status();
        self.status = format!("Opening catalog: {url}");
    }

    pub fn run_sync(&mut self, config: &Config, dry_run: bool) {
        // Persist current matrix first so sync reads the toggles we staged in UI.
        if let Err(err) = mcp::save_enablement(&config.home, &self.matrix) {
            self.status = format!("save enablement failed: {err}");
            return;
        }
        let result = if dry_run {
            mcp::dry_run(&config.home)
        } else {
            mcp::sync_all(&config.home)
        };
        match result {
            Ok(report) => {
                let n = report.change_count();
                let errs = report.errors.len();
                if dry_run {
                    self.status = format!("Dry-run: {n} change(s) planned · {errs} error(s)");
                } else {
                    self.status = format!("Sync complete: {n} change(s) · {errs} error(s)");
                    self.last_sync = "just now".into();
                }
                if let Some(first) = report.errors.first() {
                    self.status = format!("{} — {first}", self.status);
                }
                // Reload drift after real sync.
                if !dry_run {
                    self.refresh(config);
                    self.status = format!("Sync complete: {n} change(s). Restart agents to apply.");
                }
            }
            Err(err) => {
                self.status = format!("{} failed: {err}", if dry_run { "Dry-run" } else { "Sync" });
            }
        }
    }

    pub fn activate_action(&mut self, button: ActionButton, config: &Config) {
        match button {
            ActionButton::OpenObot => self.open_obot(config),
            ActionButton::Search => self.open_search(config),
            ActionButton::Refresh => {
                if self.search_open {
                    self.catalog_loaded = false;
                    self.load_catalog(config);
                } else {
                    self.refresh(config);
                    if !self.status.starts_with("inventory")
                        && !self.status.starts_with("health")
                        && !self.status.starts_with("drift")
                    {
                        self.status = "Refreshed.".into();
                    }
                }
            }
            ActionButton::SyncAll => self.run_sync(config, false),
            ActionButton::DryRun => self.run_sync(config, true),
        }
    }
}

fn detected_agent_columns(config: &Config) -> Vec<AgentColumn> {
    hooks::detect_agents(&config.home)
        .into_iter()
        .map(|r| AgentColumn {
            id: r.id.to_string(),
            label: r.id.to_string(),
        })
        .collect()
}

fn present_keys_by_agent(
    home: &std::path::Path,
    agents: &[AgentColumn],
) -> std::collections::HashMap<String, std::collections::HashSet<String>> {
    use std::collections::{HashMap, HashSet};
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    for agent in agents {
        let Some(adapter) = mcp::adapters::adapter_by_id(&agent.id) else {
            continue;
        };
        if !adapter.present(home) {
            continue;
        }
        let keys = adapter
            .read(home)
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.key)
            .collect();
        out.insert(agent.id.clone(), keys);
    }
    out
}

fn drift_kind_label(kind: DriftKind, detail: &str) -> String {
    match kind {
        DriftKind::Missing => detail.to_string(),
        DriftKind::DisabledButPresent => detail.to_string(),
        DriftKind::UrlMismatch => {
            if detail.is_empty() {
                "url ≠ gateway connect URL".into()
            } else {
                detail.to_string()
            }
        }
        DriftKind::ShapeMismatch => detail.to_string(),
    }
}
