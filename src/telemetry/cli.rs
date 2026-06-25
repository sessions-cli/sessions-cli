use anyhow::Result;

use super::config::{SessionsConfig, TelemetryLevel};
use super::heartbeat::{build_heartbeat_payload, run_heartbeat};
use super::{effective_level, ensure_sessions_config, record_feature, FeatureId, Source};
use crate::paths;
use crate::version::VERSION;

pub fn run_status() -> Result<()> {
    let home = paths::home();
    ensure_sessions_config(&home)?;
    let cfg = SessionsConfig::load(&home)?;
    let level = effective_level();
    let install_id = if cfg.telemetry.install_id.len() > 8 {
        format!("{}…", &cfg.telemetry.install_id[..8])
    } else {
        cfg.telemetry.install_id.clone()
    };
    println!("telemetry level: {}", level.as_str());
    println!("install_id: {install_id}");
    println!("channel: {}", cfg.telemetry.channel);
    println!("version: {VERSION}");
    if !cfg.update.last_check_at.is_empty() {
        println!("last update check: {}", cfg.update.last_check_at);
    }
    if let Some(info) = cfg.update_info() {
        if let Some(v) = info.available_version {
            println!("cached update: {v} ({})", info.urgency.as_str());
        }
    }
    Ok(())
}

pub fn run_enable(level: Option<&str>) -> Result<()> {
    ensure_sessions_config(&paths::home())?;
    record_feature(FeatureId::CliTelemetryEnable, Source::Cli);
    let parsed = match level {
        Some("full") => TelemetryLevel::Full,
        Some("updates_only") | Some("updates-only") | None => TelemetryLevel::UpdatesOnly,
        other => anyhow::bail!("unknown telemetry level: {:?}", other),
    };
    let home = paths::home();
    let mut cfg = SessionsConfig::load(&home)?;
    cfg.telemetry.level = parsed;
    cfg.save(&home)?;
    println!("telemetry enabled: {}", parsed.as_str());
    Ok(())
}

pub fn run_disable() -> Result<()> {
    record_feature(FeatureId::CliTelemetryDisable, Source::Cli);
    let home = paths::home();
    let mut cfg = SessionsConfig::load(&home)?;
    cfg.telemetry.level = TelemetryLevel::Off;
    cfg.save(&home)?;
    println!("telemetry disabled");
    Ok(())
}

pub fn run_log() -> Result<()> {
    record_feature(FeatureId::CliTelemetryLog, Source::Cli);
    let home = paths::home();
    let cfg = SessionsConfig::load(&home)?;
    let level = TelemetryLevel::Log;
    let request = build_heartbeat_payload(&cfg, level, None)
        .ok_or_else(|| anyhow::anyhow!("cannot build heartbeat payload"))?;
    let payload = serde_json::to_string_pretty(&request)?;
    eprintln!("{payload}");
    Ok(())
}

pub fn run_export(output: Option<&str>) -> Result<()> {
    record_feature(FeatureId::CliTelemetryExport, Source::Cli);
    let journal = super::journal::export_journal()?;
    match output {
        Some(path) => {
            std::fs::write(path, journal)?;
            println!("exported to {path}");
        }
        None => print!("{journal}"),
    }
    Ok(())
}

pub fn run_check_now() -> Result<()> {
    ensure_sessions_config(&paths::home())?;
    let _ = run_heartbeat(None)?;
    run_status()
}