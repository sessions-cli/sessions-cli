use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use uuid::Uuid;

use crate::paths;
use crate::session::workspace_usage::WorkspaceRankMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryLevel {
    Off,
    UpdatesOnly,
    Full,
    #[serde(skip)]
    Log,
}

impl TelemetryLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "off" | "" => Some(Self::Off),
            "updates_only" | "updates-only" => Some(Self::UpdatesOnly),
            "full" => Some(Self::Full),
            "log" => Some(Self::Log),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::UpdatesOnly => "updates_only",
            Self::Full => "full",
            Self::Log => "log",
        }
    }

    pub fn sends_heartbeat(self) -> bool {
        matches!(self, Self::UpdatesOnly | Self::Full)
    }
}

impl Default for TelemetryLevel {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateUrgency {
    #[default]
    None,
    Recommended,
    Critical,
}

impl UpdateUrgency {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "recommended" => Self::Recommended,
            "critical" => Self::Critical,
            _ => Self::None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Recommended => "recommended",
            Self::Critical => "critical",
        }
    }

    pub fn is_visible(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelemetrySection {
    #[serde(default)]
    pub level: TelemetryLevel,
    #[serde(default)]
    pub install_id: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    #[serde(default)]
    pub install_method: String,
}

fn default_channel() -> String {
    "stable".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstallSection {
    #[serde(default)]
    pub checkout_path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SidebarSection {
    #[serde(default)]
    pub new_session_rank: WorkspaceRankMode,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateCache {
    #[serde(default)]
    pub last_check_at: String,
    #[serde(default)]
    pub available_version: String,
    #[serde(default)]
    pub urgency: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub changelog_url: String,
    #[serde(default)]
    pub dismissed_version: String,
    #[serde(default)]
    pub install_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsConfig {
    #[serde(default)]
    pub telemetry: TelemetrySection,
    #[serde(default)]
    pub install: InstallSection,
    #[serde(default)]
    pub sidebar: SidebarSection,
    #[serde(default)]
    pub update: UpdateCache,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            telemetry: TelemetrySection {
                level: TelemetryLevel::Off,
                install_id: String::new(),
                channel: default_channel(),
                install_method: String::new(),
            },
            install: InstallSection::default(),
            sidebar: SidebarSection::default(),
            update: UpdateCache::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub available_version: Option<String>,
    pub urgency: UpdateUrgency,
    pub message: String,
    pub changelog_url: String,
    pub install_url: String,
    pub dismissed_version: Option<String>,
}

impl UpdateInfo {
    pub fn from_cache(cache: &UpdateCache) -> Option<Self> {
        Self::from_cache_with_options(cache, false)
    }

    /// Sidebar banner respects “remind me later” for recommended updates.
    pub fn from_cache_for_banner(cache: &UpdateCache) -> Option<Self> {
        Self::from_cache_with_options(cache, false)
    }

    /// Settings and status always show a cached update when one exists.
    pub fn from_cache_for_settings(cache: &UpdateCache) -> Option<Self> {
        Self::from_cache_with_options(cache, true)
    }

    fn from_cache_with_options(cache: &UpdateCache, ignore_dismiss: bool) -> Option<Self> {
        let urgency = UpdateUrgency::parse(&cache.urgency);
        if !urgency.is_visible() || cache.available_version.is_empty() {
            return None;
        }
        let dismissed = (!cache.dismissed_version.is_empty())
            .then(|| cache.dismissed_version.clone());
        if !ignore_dismiss
            && dismissed.as_deref() == Some(cache.available_version.as_str())
            && urgency == UpdateUrgency::Recommended
        {
            return None;
        }
        Some(Self {
            available_version: Some(cache.available_version.clone()),
            urgency,
            message: cache.message.clone(),
            changelog_url: cache.changelog_url.clone(),
            install_url: cache.install_url.clone(),
            dismissed_version: dismissed,
        })
    }

    pub fn should_show(&self) -> bool {
        self.urgency.is_visible()
    }
}

impl SessionsConfig {
    pub fn load(home: &Path) -> Result<Self> {
        let path = paths::config_path(home);
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        let mut cfg: Self = toml::from_str(&raw).with_context(|| path.display().to_string())?;
        if cfg.telemetry.install_id.is_empty() {
            cfg.telemetry.install_id = Uuid::new_v4().to_string();
            cfg.save(home)?;
        }
        Ok(cfg)
    }

    pub fn save(&self, home: &Path) -> Result<()> {
        let path = paths::config_path(home);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self)?;
        fs::write(&path, raw)?;
        Ok(())
    }

    pub fn apply_update_response(&mut self, response: &super::remote::HeartbeatResponse) {
        self.update.last_check_at = chrono::Utc::now().to_rfc3339();
        if let Some(update) = &response.update {
            if let Some(version) = &update.available {
                self.update.available_version = version.clone();
            }
            if let Some(urgency) = &update.urgency {
                self.update.urgency = urgency.clone();
            }
            if let Some(message) = &update.message {
                self.update.message = message.clone();
            }
            if let Some(url) = &update.changelog_url {
                self.update.changelog_url = url.clone();
            }
        }
        if let Some(upgrade) = &response.upgrade {
            if let Some(url) = &upgrade.install_url {
                self.update.install_url = url.clone();
            }
        }
    }

    pub fn update_info(&self) -> Option<UpdateInfo> {
        UpdateInfo::from_cache_for_banner(&self.update)
    }

    pub fn cached_update(&self) -> Option<UpdateInfo> {
        UpdateInfo::from_cache_for_settings(&self.update)
    }

    pub fn dismiss_update(&mut self, version: &str) -> Result<()> {
        self.update.dismissed_version = version.to_string();
        self.save(&crate::paths::home())
    }
}

pub fn ensure_config(home: &Path, install_method: &str, checkout_path: Option<&str>) -> Result<()> {
    let path = paths::config_path(home);
    if path.exists() {
        let mut cfg = SessionsConfig::load(home)?;
        if cfg.telemetry.install_method.is_empty() {
            cfg.telemetry.install_method = install_method.to_string();
        }
        if cfg.install.checkout_path.is_empty() {
            if let Some(p) = checkout_path {
                cfg.install.checkout_path = p.to_string();
            }
        }
        cfg.save(home)?;
        return Ok(());
    }
    let mut cfg = SessionsConfig::default();
    cfg.telemetry.install_id = Uuid::new_v4().to_string();
    cfg.telemetry.install_method = install_method.to_string();
    if let Ok(channel) = std::env::var("SESSIONS_CHANNEL") {
        if !channel.trim().is_empty() {
            cfg.telemetry.channel = channel.trim().to_string();
        }
    }
    if let Some(p) = checkout_path {
        cfg.install.checkout_path = p.to_string();
    }
    cfg.save(home)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cache() -> UpdateCache {
        UpdateCache {
            available_version: "0.2.0".into(),
            urgency: "recommended".into(),
            message: "Bug fixes".into(),
            ..Default::default()
        }
    }

    #[test]
    fn banner_hides_dismissed_recommended_update() {
        let mut cache = sample_cache();
        cache.dismissed_version = "0.2.0".into();
        assert!(UpdateInfo::from_cache_for_banner(&cache).is_none());
    }

    #[test]
    fn settings_shows_dismissed_recommended_update() {
        let mut cache = sample_cache();
        cache.dismissed_version = "0.2.0".into();
        let info = UpdateInfo::from_cache_for_settings(&cache).expect("settings update");
        assert_eq!(info.available_version.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn critical_update_ignores_dismiss_for_banner() {
        let mut cache = sample_cache();
        cache.urgency = "critical".into();
        cache.dismissed_version = "0.2.0".into();
        let info = UpdateInfo::from_cache_for_banner(&cache).expect("critical update");
        assert_eq!(info.urgency, UpdateUrgency::Critical);
    }
}