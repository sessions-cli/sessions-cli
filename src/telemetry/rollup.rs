use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::counters::PendingFlush;
use crate::daemon::metrics::{HookOutcomes, RuntimeMetrics};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngagementRollup {
    pub sidebar_attach_count: u64,
    pub launcher_opens: u64,
    pub launcher_submits: u64,
    pub sessions_active_max: u64,
    pub features_distinct: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelemetryRollup {
    pub hook_outcomes: HookOutcomes,
    pub session_count: u64,
    pub sessions_created: u64,
    pub refresh_p95_us: u64,
    pub hook_apply_p95_us: u64,
    pub daemon_uptime_s: u64,
    pub engagement: EngagementRollup,
    pub feature_counts: Vec<super::counters::FeatureCount>,
    #[serde(default)]
    pub agents_installed: Vec<String>,
}

pub fn build_rollup(
    metrics: &RuntimeMetrics,
    hook_outcomes: &HookOutcomes,
    pending: &PendingFlush,
    session_count: u64,
    daemon_started_at: std::time::Instant,
) -> TelemetryRollup {
    let distinct: HashSet<_> = pending
        .feature_counts
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    TelemetryRollup {
        hook_outcomes: hook_outcomes.clone(),
        session_count,
        sessions_created: 0,
        refresh_p95_us: metrics.refresh_p95_us(),
        hook_apply_p95_us: metrics.hook_apply_p95_us(),
        daemon_uptime_s: daemon_started_at.elapsed().as_secs(),
        engagement: EngagementRollup {
            sidebar_attach_count: pending.sidebar_attach_count,
            launcher_opens: pending.launcher_opens,
            launcher_submits: pending.launcher_submits,
            sessions_active_max: session_count,
            features_distinct: distinct.len() as u64,
        },
        feature_counts: pending.feature_counts.clone(),
        agents_installed: Vec::new(),
    }
}
