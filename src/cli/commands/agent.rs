use crate::agents::registry::PROVIDERS;
use crate::cli::commands::session::refresh_after_window_change;
use crate::config::Config;
use crate::daemon;
use crate::session;
use anyhow::Result;

/// Top-level CLI shortcuts for agent launch (subset of [`PROVIDERS`]; claude has no alias).
pub const CLI_AGENT_ALIASES: &[&str] = &["grok", "codex", "opencode"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_agent_aliases_exist_in_providers_registry() {
        for alias in CLI_AGENT_ALIASES {
            assert!(
                PROVIDERS.iter().any(|provider| provider.id == *alias),
                "CLI alias {alias} must exist in PROVIDERS registry"
            );
        }
    }
}

pub fn run(agent_id: &str) -> Result<()> {
    let config = Config::default();
    daemon::tmux::create_agent_window(&config, agent_id)?;
    refresh_after_window_change(&config);
    Ok(())
}

pub fn run_create_instant(agent_id: &str) -> Result<()> {
    let config = Config::default();
    session::create_instant_agent(&config, agent_id)?;
    refresh_after_window_change(&config);
    Ok(())
}