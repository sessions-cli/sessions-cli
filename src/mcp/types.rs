//! Shared types for Obot-backed MCP inventory, enablement, and agent sync.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where an MCP server definition comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerSource {
    /// Hosted / gateway URL owned by Obot.
    ObotGateway {
        obot_id: String,
        connect_url: String,
    },
    /// Host-local stdio process (not routed through Obot).
    LocalOnly { command: String, args: Vec<String> },
}

impl ServerSource {
    pub fn is_obot(&self) -> bool {
        matches!(self, Self::ObotGateway { .. })
    }

    pub fn connect_url(&self) -> Option<&str> {
        match self {
            Self::ObotGateway { connect_url, .. } => Some(connect_url.as_str()),
            Self::LocalOnly { .. } => None,
        }
    }

    pub fn command(&self) -> Option<&str> {
        match self {
            Self::LocalOnly { command, .. } => Some(command.as_str()),
            Self::ObotGateway { .. } => None,
        }
    }
}

/// Unified view of one MCP server for UI / CLI listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerView {
    /// Stable slug used as the agent config key (e.g. `stripe`).
    pub key: String,
    pub display_name: String,
    pub source: ServerSource,
    /// `None` = n/a or unknown.
    pub oauth_ok: Option<bool>,
    /// `None` = unknown / not applicable.
    pub running: Option<bool>,
}

/// Catalog entry available to deploy (Obot `/api/all-mcps/entries`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntryView {
    /// Catalog entry id (passed to create-server as `catalogEntryID`).
    pub id: String,
    /// Display / slug name from the entry manifest.
    pub name: String,
    pub short_description: String,
    pub description: String,
    /// `singleUser` / `multiUser` when known.
    pub user_type: String,
    pub catalog_name: String,
    /// Default connect URL if the entry exposes one before deploy.
    pub connect_url: String,
    /// Admin OAuth for this entry is configured (when required).
    pub oauth_configured: bool,
}

impl CatalogEntryView {
    /// Prefer short description, else truncated full description.
    pub fn summary(&self) -> &str {
        if !self.short_description.trim().is_empty() {
            self.short_description.as_str()
        } else {
            self.description.as_str()
        }
    }

    /// Suggested agent-config key / alias from the entry name.
    pub fn suggested_key(&self) -> String {
        let from_name = slugify_key(&self.name);
        if is_plausible_key(&from_name) {
            from_name
        } else {
            slugify_key(&self.id)
        }
    }
}

fn is_plausible_key(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn slugify_key(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if (c == '_' || c == '-' || c == '.' || c.is_whitespace())
            && !prev_dash
            && !out.is_empty()
        {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "server".into()
    } else {
        out
    }
}

/// Result of deploying a catalog entry as a personal MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateServerResult {
    pub id: String,
    pub key: String,
    pub display_name: String,
    pub connect_url: String,
    pub configured: bool,
    pub missing_oauth: bool,
}

/// sessions-owned enable matrix: server_key → agent_id → enabled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnablementMatrix {
    pub map: BTreeMap<String, BTreeMap<String, bool>>,
}

impl EnablementMatrix {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_enabled(&self, server_key: &str, agent_id: &str) -> Option<bool> {
        self.map
            .get(server_key)
            .and_then(|agents| agents.get(agent_id).copied())
    }

    pub fn set(&mut self, server_key: &str, agent_id: &str, enabled: bool) {
        self.map
            .entry(server_key.to_string())
            .or_default()
            .insert(agent_id.to_string(), enabled);
    }

    /// Resolve enablement: matrix → else `default_if_absent`.
    pub fn enabled_or(&self, server_key: &str, agent_id: &str, default_if_absent: bool) -> bool {
        self.is_enabled(server_key, agent_id)
            .unwrap_or(default_if_absent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    /// Matrix says enabled but agent config lacks the entry.
    Missing,
    /// Managed key present in agent config but matrix says disabled.
    DisabledButPresent,
    /// HTTP URL in agent config differs from Obot `connectURL`.
    UrlMismatch,
    /// Entry exists with unexpected shape (e.g. stdio where gateway URL expected).
    ShapeMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftItem {
    pub agent_id: String,
    pub server_key: String,
    pub kind: DriftKind,
    pub detail: String,
}

/// One MCP entry as stored in an agent’s native config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMcpEntry {
    pub key: String,
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// When `false` and passed to `write_merge`, the managed key is removed.
    pub enabled: bool,
}

impl AgentMcpEntry {
    pub fn http(key: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            url: Some(url.into()),
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: true,
        }
    }

    pub fn stdio(key: impl Into<String>, command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            key: key.into(),
            url: None,
            command: Some(command.into()),
            args,
            env: BTreeMap::new(),
            enabled: true,
        }
    }

    pub fn disabled(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            url: None,
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: false,
        }
    }

    pub fn is_http(&self) -> bool {
        self.url.as_ref().is_some_and(|u| !u.is_empty())
    }

    pub fn is_stdio(&self) -> bool {
        self.command.as_ref().is_some_and(|c| !c.is_empty())
    }
}

/// Obot client settings from `obot.toml` (+ env token override).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObotConfig {
    #[serde(default = "default_obot_enabled")]
    pub enabled: bool,
    #[serde(default = "default_obot_base_url")]
    pub base_url: String,
    /// Prefer env `SESSIONS_OBOT_TOKEN` over this file value at runtime.
    #[serde(default)]
    pub bootstrap_token: Option<String>,
    /// Path appended to `base_url` for “Open Obot” (e.g. `/mcp-catalog`).
    #[serde(default = "default_open_admin_path")]
    pub open_admin_path: String,
}

fn default_obot_enabled() -> bool {
    true
}

fn default_obot_base_url() -> String {
    "http://127.0.0.1:8080".into()
}

fn default_open_admin_path() -> String {
    "/mcp-catalog".into()
}

impl Default for ObotConfig {
    fn default() -> Self {
        Self {
            enabled: default_obot_enabled(),
            base_url: default_obot_base_url(),
            bootstrap_token: None,
            open_admin_path: default_open_admin_path(),
        }
    }
}

impl ObotConfig {
    pub fn admin_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = if self.open_admin_path.starts_with('/') {
            self.open_admin_path.clone()
        } else {
            format!("/{}", self.open_admin_path)
        };
        format!("{base}{path}")
    }

    /// Token from env `SESSIONS_OBOT_TOKEN`, else file `bootstrap_token`.
    pub fn resolved_token(&self) -> Option<String> {
        if let Ok(env) = std::env::var("SESSIONS_OBOT_TOKEN") {
            let t = env.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
        self.bootstrap_token
            .as_ref()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObotHealthStatus {
    Up,
    Down,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObotHealth {
    pub status: ObotHealthStatus,
    pub base_url: String,
    pub detail: String,
    pub token_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncChange {
    pub agent_id: String,
    pub server_key: String,
    pub action: SyncAction,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncAction {
    Upsert,
    Remove,
    Skip,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    pub dry_run: bool,
    pub changes: Vec<SyncChange>,
    pub errors: Vec<String>,
}

impl SyncReport {
    pub fn change_count(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| matches!(c.action, SyncAction::Upsert | SyncAction::Remove))
            .count()
    }
}
