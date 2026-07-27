//! Automation definition and run record schemas (Codex-compatible base + multi-agent).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// How often the automation fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutomationKind {
    #[default]
    Cron,
    Heartbeat,
}

/// Whether the scheduler will fire this definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum AutomationStatus {
    #[default]
    Active,
    Paused,
}

/// Where the agent runs relative to the project tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionEnvironment {
    #[default]
    Local,
    /// Phase 2 — accepted in schema, rejected at run time until implemented.
    Worktree,
}

/// Standalone = new session each fire. Thread attach is phase 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AttachMode {
    #[default]
    Standalone,
    Thread,
}

/// Durable automation definition (`automation.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Automation {
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: AutomationKind,
    #[serde(default)]
    pub status: AutomationStatus,
    pub prompt: String,
    /// RFC 5545 RRULE body without the `RRULE:` prefix (cron kind).
    #[serde(default)]
    pub rrule: String,
    /// Heartbeat interval in minutes (heartbeat kind).
    #[serde(default)]
    pub interval_minutes: u32,
    /// IANA timezone name; empty means system local.
    #[serde(default)]
    pub timezone: String,
    /// Agent id: grok | codex | claude | opencode
    pub agent: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub reasoning_effort: String,
    #[serde(default)]
    pub execution_environment: ExecutionEnvironment,
    /// Absolute project roots. Phase 1 uses the first cwd for fires.
    pub cwds: Vec<String>,
    #[serde(default)]
    pub attach_mode: AttachMode,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Automation {
    pub fn new(
        id: String,
        name: String,
        prompt: String,
        agent: String,
        model: String,
        rrule: String,
        cwd: String,
    ) -> Self {
        let now = Utc::now().timestamp_millis();
        Self {
            version: SCHEMA_VERSION,
            id,
            name,
            kind: AutomationKind::Cron,
            status: AutomationStatus::Active,
            prompt,
            rrule,
            interval_minutes: 0,
            timezone: String::new(),
            agent,
            model,
            reasoning_effort: String::new(),
            execution_environment: ExecutionEnvironment::Local,
            cwds: vec![cwd],
            attach_mode: AttachMode::Standalone,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status, AutomationStatus::Active)
    }

    pub fn primary_cwd(&self) -> Option<&str> {
        self.cwds
            .first()
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now().timestamp_millis();
    }
}

/// Lifecycle of a single automation fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    #[default]
    Pending,
    Running,
    Done,
    Failed,
    Archived,
}

/// Findings classification for inbox policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Findings,
    Empty,
    Unknown,
}

/// One execution of an automation (`runs/<run-id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutomationRun {
    pub id: String,
    pub automation_id: String,
    pub cwd: String,
    pub agent: String,
    pub model: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub status: RunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub unread: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// True when this fire was a single catch-up after daemon downtime.
    #[serde(default)]
    pub caught_up: bool,
}

impl AutomationRun {
    pub fn new(automation: &Automation, cwd: &str) -> Self {
        Self {
            id: new_run_id(),
            automation_id: automation.id.clone(),
            cwd: cwd.to_string(),
            agent: automation.agent.clone(),
            model: automation.model.clone(),
            started_at: Utc::now(),
            finished_at: None,
            status: RunStatus::Pending,
            outcome: None,
            sessions_session_id: None,
            window_index: None,
            summary: None,
            unread: true,
            error: None,
            caught_up: false,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            RunStatus::Done | RunStatus::Failed | RunStatus::Archived
        )
    }

    pub fn is_open_inbox(&self) -> bool {
        self.unread
            && matches!(
                self.status,
                RunStatus::Done | RunStatus::Failed | RunStatus::Running
            )
    }
}

/// Per-definition runtime bookkeeping (`state.json`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AutomationState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub fn new_run_id() -> String {
    format!("run_{}", uuid::Uuid::new_v4().simple())
}

/// Slug id from a display name (filesystem-safe).
pub fn slugify_id(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        let c = if ch.is_ascii_alphanumeric() {
            prev_dash = false;
            ch.to_ascii_lowercase()
        } else if matches!(ch, ' ' | '-' | '_' | '/') {
            if prev_dash || out.is_empty() {
                continue;
            }
            prev_dash = true;
            '-'
        } else {
            continue;
        };
        out.push(c);
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        format!("auto-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify_id("Daily CI Triage"), "daily-ci-triage");
        assert_eq!(slugify_id("  foo__bar  "), "foo-bar");
    }

    #[test]
    fn automation_round_trip_toml() {
        let a = Automation::new(
            "daily-ci".into(),
            "Daily CI".into(),
            "check CI".into(),
            "grok".into(),
            "grok-build".into(),
            "FREQ=DAILY;BYHOUR=9;BYMINUTE=0".into(),
            "/tmp/project".into(),
        );
        let raw = toml::to_string_pretty(&a).unwrap();
        let back: Automation = toml::from_str(&raw).unwrap();
        assert_eq!(back.id, "daily-ci");
        assert_eq!(back.status, AutomationStatus::Active);
        assert_eq!(back.kind, AutomationKind::Cron);
    }

    #[test]
    fn run_round_trip_json() {
        let a = Automation::new(
            "x".into(),
            "X".into(),
            "p".into(),
            "codex".into(),
            "gpt-5.4".into(),
            "FREQ=DAILY;BYHOUR=9;BYMINUTE=0".into(),
            "/tmp".into(),
        );
        let run = AutomationRun::new(&a, "/tmp");
        let raw = serde_json::to_string_pretty(&run).unwrap();
        let back: AutomationRun = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.automation_id, "x");
        assert_eq!(back.status, RunStatus::Pending);
    }
}
