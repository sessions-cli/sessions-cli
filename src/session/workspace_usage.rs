use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::paths;

/// How the new-session picker orders active sessions and directory suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRankMode {
    #[default]
    MostUsed,
    MostRecent,
}

impl WorkspaceRankMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::MostUsed => "Most used",
            Self::MostRecent => "Most recent",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::MostUsed => Self::MostRecent,
            Self::MostRecent => Self::MostUsed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceUsageEntry {
    #[serde(default)]
    pub focus_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_focused_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceUsageStore {
    #[serde(default)]
    entries: HashMap<String, WorkspaceUsageEntry>,
}

impl WorkspaceUsageStore {
    pub fn path(home: &Path) -> PathBuf {
        paths::state_dir(home).join("workspace-usage.json")
    }

    pub fn load(home: &Path) -> Self {
        let path = Self::path(home);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, home: &Path) -> Result<()> {
        let path = Self::path(home);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, raw).with_context(|| path.display().to_string())?;
        Ok(())
    }

    pub fn entry(&self, cwd: &str) -> Option<&WorkspaceUsageEntry> {
        self.entries.get(cwd)
    }

    pub fn record_focus(&mut self, cwd: &str, label: &str) {
        let cwd = cwd.trim();
        if cwd.is_empty() {
            return;
        }
        let entry = self.entries.entry(cwd.to_string()).or_default();
        entry.focus_count = entry.focus_count.saturating_add(1);
        entry.last_focused_at = Some(Utc::now());
        if !label.trim().is_empty() {
            entry.label = label.trim().to_string();
        }
    }

    /// Persist a single focus event without keeping an in-memory store.
    pub fn record_focus_at(home: &Path, cwd: &str, label: &str) -> Result<()> {
        let mut store = Self::load(home);
        store.record_focus(cwd, label);
        store.save(home)
    }

    pub fn rank_score(
        &self,
        cwd: &str,
        recent_hint: Option<DateTime<Utc>>,
        mode: WorkspaceRankMode,
    ) -> (i64, i64) {
        let entry = self.entries.get(cwd);
        let count = entry.map(|e| e.focus_count as i64).unwrap_or(0);
        let last = entry
            .and_then(|e| e.last_focused_at)
            .or(recent_hint)
            .map(|ts| ts.timestamp())
            .unwrap_or(0);
        match mode {
            WorkspaceRankMode::MostUsed => (count, last),
            WorkspaceRankMode::MostRecent => (last, count),
        }
    }

    /// Closed workspaces with usage history, still valid directories on disk.
    pub fn closed_suggestions(
        &self,
        active_cwds: &std::collections::HashSet<String>,
        mode: WorkspaceRankMode,
        limit: usize,
    ) -> Vec<(String, String)> {
        let mut ranked: Vec<(String, String, (i64, i64))> = self
            .entries
            .iter()
            .filter(|(cwd, entry)| {
                entry.focus_count > 0
                    && !active_cwds.contains(cwd.as_str())
                    && Path::new(cwd.as_str()).is_dir()
            })
            .map(|(cwd, entry)| {
                let label = if entry.label.is_empty() {
                    cwd.clone()
                } else {
                    entry.label.clone()
                };
                let score = self.rank_score(cwd, entry.last_focused_at, mode);
                (label, cwd.clone(), score)
            })
            .collect();
        ranked.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
        ranked
            .into_iter()
            .take(limit)
            .map(|(label, cwd, _)| (label, cwd))
            .collect()
    }
}

pub fn load_rank_mode(home: &Path) -> WorkspaceRankMode {
    crate::telemetry::config::SessionsConfig::load(home)
        .map(|cfg| cfg.sidebar.new_session_rank)
        .unwrap_or_default()
}

pub fn save_rank_mode(home: &Path, mode: WorkspaceRankMode) -> Result<()> {
    let mut cfg = crate::telemetry::config::SessionsConfig::load(home)?;
    cfg.sidebar.new_session_rank = mode;
    cfg.save(home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn record_focus_increments_and_updates_label() {
        let mut store = WorkspaceUsageStore::default();
        store.record_focus("/tmp/a", "~/a");
        store.record_focus("/tmp/a", "~/a");
        let entry = store.entry("/tmp/a").unwrap();
        assert_eq!(entry.focus_count, 2);
        assert_eq!(entry.label, "~/a");
        assert!(entry.last_focused_at.is_some());
    }

    #[test]
    fn rank_score_prefers_higher_count_in_most_used_mode() {
        let mut store = WorkspaceUsageStore::default();
        store.record_focus("/tmp/a", "~/a");
        store.record_focus("/tmp/b", "~/b");
        store.record_focus("/tmp/b", "~/b");
        let a = store.rank_score("/tmp/a", None, WorkspaceRankMode::MostUsed);
        let b = store.rank_score("/tmp/b", None, WorkspaceRankMode::MostUsed);
        assert!(b > a);
    }

    #[test]
    fn rank_score_prefers_newer_last_focus_in_most_recent_mode() {
        let mut store = WorkspaceUsageStore::default();
        store.entries.insert(
            "/tmp/old".into(),
            WorkspaceUsageEntry {
                focus_count: 10,
                last_focused_at: Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()),
                label: "~/old".into(),
            },
        );
        store.entries.insert(
            "/tmp/new".into(),
            WorkspaceUsageEntry {
                focus_count: 1,
                last_focused_at: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
                label: "~/new".into(),
            },
        );
        let old = store.rank_score("/tmp/old", None, WorkspaceRankMode::MostRecent);
        let new = store.rank_score("/tmp/new", None, WorkspaceRankMode::MostRecent);
        assert!(new > old);
    }

    #[test]
    fn closed_suggestions_skip_active_and_missing_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let active_dir = temp.path().join("active");
        let closed_dir = temp.path().join("closed");
        std::fs::create_dir_all(&active_dir).unwrap();
        std::fs::create_dir_all(&closed_dir).unwrap();
        let active = active_dir.display().to_string();
        let closed = closed_dir.display().to_string();

        let mut store = WorkspaceUsageStore::default();
        store.record_focus(&active, "~/active");
        store.record_focus(&closed, "~/closed");
        store.record_focus(&closed, "~/closed");

        let active_set = std::iter::once(active.clone()).collect();
        let suggestions = store.closed_suggestions(&active_set, WorkspaceRankMode::MostUsed, 8);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].1, closed);
    }
}