use anyhow::Result;
use chrono::{DateTime, Utc};

use super::config::{SessionsConfig, TelemetryLevel};
use super::counters::PendingFlush;
use super::remote::{self, HeartbeatRequest, HeartbeatResponse};
use super::rollup::TelemetryRollup;
use crate::daemon::metrics::HookOutcomes;
use crate::paths;
use crate::version::VERSION;

/// CLI opportunistic update check (`sessions up` / `doctor`).
const STALE_CHECK_HOURS: i64 = 24;
/// Minimum gap between automatic remote heartbeats (daemon loop, bar flush).
pub const MIN_REMOTE_INTERVAL_HOURS: i64 = 12;

pub fn build_heartbeat_payload(
    cfg: &SessionsConfig,
    level: TelemetryLevel,
    rollup: Option<TelemetryRollup>,
) -> Option<HeartbeatRequest> {
    let telemetry_level = remote::level_for_request(level)?;
    if cfg.telemetry.install_id.is_empty() {
        return None;
    }
    Some(HeartbeatRequest {
        install_id: cfg.telemetry.install_id.clone(),
        version: VERSION.to_string(),
        os: remote::platform_os(),
        arch: remote::platform_arch(),
        channel: cfg.telemetry.channel.clone(),
        telemetry_level,
        period_hours: rollup.as_ref().map(|_| 12),
        rollup,
    })
}

pub fn run_heartbeat(
    rollup: Option<TelemetryRollup>,
) -> Result<Option<HeartbeatResponse>> {
    let home = paths::home();
    let level = super::effective_level();
    if !level.sends_heartbeat() {
        return Ok(None);
    }
    let mut cfg = SessionsConfig::load(&home)?;
    let Some(request) = build_heartbeat_payload(&cfg, level, rollup) else {
        return Ok(None);
    };

    if level == TelemetryLevel::Log {
        let payload = serde_json::to_string_pretty(&request)?;
        eprintln!("{payload}");
        return Ok(None);
    }

    match remote::send_heartbeat(&request) {
        Ok(response) => {
            if let Some(ref resp) = response {
                cfg.apply_update_response(resp);
                cfg.save(&home)?;
            } else {
                cfg.update.last_check_at = Utc::now().to_rfc3339();
                cfg.save(&home)?;
            }
            super::journal::record_event(
                "heartbeat_sent",
                serde_json::json!({ "ok": true }),
            );
            Ok(response)
        }
        Err(err) => {
            super::journal::record_event(
                "heartbeat_failed",
                serde_json::json!({ "error_code": "heartbeat_request_failed" }),
            );
            tracing::debug!("heartbeat failed: {err}");
            Ok(None)
        }
    }
}

pub fn maybe_heartbeat(force: bool) -> Result<()> {
    let level = super::effective_level();
    if !level.sends_heartbeat() {
        return Ok(());
    }
    if !force && !is_check_stale()? {
        return Ok(());
    }
    let _ = run_heartbeat(None)?;
    Ok(())
}

pub fn is_check_stale() -> Result<bool> {
    hours_since_last_remote_send().map(|hours| hours >= STALE_CHECK_HOURS)
}

/// Gate automatic remote sends (daemon periodic loop). Manual `telemetry check` bypasses.
pub fn is_remote_send_due(force: bool) -> Result<bool> {
    if force {
        return Ok(true);
    }
    hours_since_last_remote_send().map(|hours| hours >= MIN_REMOTE_INTERVAL_HOURS)
}

fn hours_since_last_remote_send() -> Result<i64> {
    let cfg = SessionsConfig::load(&paths::home())?;
    if cfg.update.last_check_at.is_empty() {
        return Ok(i64::MAX);
    }
    let parsed: DateTime<Utc> = cfg.update.last_check_at.parse()?;
    Ok((Utc::now() - parsed).num_hours())
}

pub fn run_full_heartbeat(
    pending: PendingFlush,
    hook_outcomes: HookOutcomes,
    session_count: u64,
    daemon_started_at: std::time::Instant,
) -> Result<()> {
    let level = super::effective_level();
    let rollup = if level == TelemetryLevel::Full {
        let metrics = crate::daemon::metrics::snapshot();
        Some(super::rollup::build_rollup(
            &metrics,
            &hook_outcomes,
            &pending,
            session_count,
            daemon_started_at,
        ))
    } else {
        None
    };
    let _ = run_heartbeat(rollup)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_stale_window_is_at_least_daemon_interval() {
        assert!(STALE_CHECK_HOURS >= MIN_REMOTE_INTERVAL_HOURS);
    }
}