use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::config::TelemetryLevel;
use super::guards::reject_sensitive_strings;
use super::rollup::TelemetryRollup;

pub const DEFAULT_HEARTBEAT_URL: &str =
    "https://slcqgbgvuemwwstuzvpu.supabase.co/functions/v1/heartbeat";

const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub install_id: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub channel: String,
    pub telemetry_level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_hours: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollup: Option<TelemetryRollup>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateResponse {
    pub available: Option<String>,
    pub urgency: Option<String>,
    pub message: Option<String>,
    pub changelog_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpgradeResponse {
    pub command: Option<String>,
    pub install_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    pub update: Option<UpdateResponse>,
    pub upgrade: Option<UpgradeResponse>,
}

pub fn heartbeat_url() -> String {
    std::env::var("SESSIONS_HEARTBEAT_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_HEARTBEAT_URL.to_string())
}

pub fn platform_os() -> String {
    if cfg!(target_os = "macos") {
        "darwin".into()
    } else {
        "linux".into()
    }
}

pub fn platform_arch() -> String {
    if cfg!(target_arch = "aarch64") {
        "aarch64".into()
    } else {
        "x86_64".into()
    }
}

pub fn send_heartbeat(request: &HeartbeatRequest) -> Result<Option<HeartbeatResponse>> {
    let payload = serde_json::to_string(request)?;
    reject_sensitive_strings(&payload)?;

    let client = reqwest::blocking::Client::builder()
        .timeout(HEARTBEAT_TIMEOUT)
        .user_agent(format!("sessions-cli/{}", crate::version::VERSION))
        .build()?;

    let response = client
        .post(heartbeat_url())
        .header("Content-Type", "application/json")
        .body(payload)
        .send()?;

    let status = response.status();
    if status.as_u16() == 204 || status.as_u16() == 503 {
        return Ok(None);
    }
    if !status.is_success() {
        anyhow::bail!("heartbeat returned {}", status);
    }
    let body = response.text()?;
    if body.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&body)?))
}

pub fn level_for_request(level: TelemetryLevel) -> Option<String> {
    match level {
        TelemetryLevel::UpdatesOnly => Some("updates_only".into()),
        TelemetryLevel::Full => Some("full".into()),
        TelemetryLevel::Log => Some("full".into()),
        TelemetryLevel::Off => None,
    }
}
