pub mod cli;
pub mod config;
pub mod counters;
pub mod feature;
pub mod guards;
pub mod heartbeat;
pub mod journal;
pub mod remote;
pub mod rollup;

pub use config::{SessionsConfig, TelemetryLevel};
pub use feature::{FeatureId, Source};

use std::sync::OnceLock;

static SESSIONS_CONFIG: OnceLock<SessionsConfig> = OnceLock::new();

pub fn sessions_config() -> &'static SessionsConfig {
    SESSIONS_CONFIG.get_or_init(|| {
        let home = crate::paths::home();
        SessionsConfig::load(&home).unwrap_or_default()
    })
}

pub fn reload_sessions_config() -> SessionsConfig {
    let home = crate::paths::home();
    let cfg = SessionsConfig::load(&home).unwrap_or_default();
    let _ = SESSIONS_CONFIG.set(cfg.clone());
    cfg
}

pub fn effective_level() -> TelemetryLevel {
    if std::env::var("DO_NOT_TRACK")
        .ok()
        .is_some_and(|v| !v.trim().is_empty() && v != "0")
    {
        return TelemetryLevel::Off;
    }
    if let Ok(level) = std::env::var("SESSIONS_TELEMETRY") {
        if let Some(parsed) = TelemetryLevel::parse(&level) {
            return parsed;
        }
    }
    sessions_config().telemetry.level
}

pub fn record_feature(feature: FeatureId, source: Source) {
    let level = effective_level();
    if level == TelemetryLevel::Off {
        return;
    }
    counters::record_feature(feature, source);
    if level == TelemetryLevel::Full {
        journal::record_feature(feature, source);
    }
}

pub fn record_lifecycle(feature: FeatureId) {
    record_feature(feature, Source::Cli);
}

/// Create `~/.config/sessions/config.toml` with `install_id` on first use.
pub fn ensure_sessions_config(home: &std::path::Path) -> anyhow::Result<()> {
    let install_method = if crate::paths::install_dir(home).join("sessions").is_file() {
        "local"
    } else {
        "unknown"
    };
    config::ensure_config(home, install_method, None)?;
    let _ = reload_sessions_config();
    Ok(())
}
