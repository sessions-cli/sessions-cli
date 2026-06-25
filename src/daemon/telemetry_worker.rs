use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::daemon::metrics;
use crate::daemon::state::DaemonState;
use crate::telemetry::counters::{self, PendingFlush};
use crate::telemetry::heartbeat;
use crate::telemetry::TelemetryLevel;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
const HEARTBEAT_START_DELAY: Duration = Duration::from_secs(60);

static PENDING_ACCUM: LazyLock<Mutex<PendingFlush>> =
    LazyLock::new(|| Mutex::new(PendingFlush::default()));

pub fn spawn_heartbeat_loop(state: Arc<DaemonState>, config: Config) {
    let started_at = Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(HEARTBEAT_START_DELAY).await;
        loop {
            if crate::telemetry::effective_level().sends_heartbeat() {
                let _ = maybe_send_remote_heartbeat(&state, &config, started_at, false).await;
            }
            tokio::time::sleep(HEARTBEAT_INTERVAL).await;
        }
    });
}

/// Merge bar/CLI pending counters locally. Never hits the network.
pub async fn handle_telemetry_flush(state: &DaemonState, config: &Config) {
    if !crate::telemetry::effective_level().sends_heartbeat() {
        return;
    }
    merge_pending_from_disk(config);
    let session_count = state.session_count().await as u64;
    tracing::debug!(
        session_count,
        "telemetry flush merged locally (no remote send)"
    );
}

async fn maybe_send_remote_heartbeat(
    state: &DaemonState,
    config: &Config,
    started_at: Instant,
    force: bool,
) -> anyhow::Result<()> {
    if !crate::telemetry::effective_level().sends_heartbeat() {
        return Ok(());
    }
    if !heartbeat::is_remote_send_due(force)? {
        tracing::debug!("telemetry remote send skipped — within minimum interval");
        return Ok(());
    }
    merge_pending_from_disk(config);
    let pending = take_accumulated_pending();
    let hook_outcomes = metrics::take_hook_outcomes();
    let session_count = state.session_count().await as u64;
    let level = crate::telemetry::effective_level();
    if level == TelemetryLevel::Full {
        heartbeat::run_full_heartbeat(pending, hook_outcomes, session_count, started_at)?;
    } else {
        heartbeat::run_heartbeat(None)?;
    }
    Ok(())
}

fn merge_pending_from_disk(config: &Config) {
    let incoming = counters::flush_pending(&config.home);
    if incoming.feature_counts.is_empty()
        && incoming.sidebar_attach_count == 0
        && incoming.launcher_opens == 0
        && incoming.launcher_submits == 0
    {
        return;
    }
    let mut guard = PENDING_ACCUM.lock().unwrap_or_else(|e| e.into_inner());
    counters::merge_pending(&mut *guard, incoming);
}

fn take_accumulated_pending() -> PendingFlush {
    let mut guard = PENDING_ACCUM.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *guard)
}