use crate::config::Config;
use crate::pty::DEFAULT_AGENT_APP;
use anyhow::Result;

use super::windows::{active_window_details, create_terminal_window_in_cwd};

pub fn create_agent_window(config: &Config, agent_id: &str) -> Result<u32> {
    if agent_id == "console" {
        let home = crate::paths::home();
        return create_terminal_window_in_cwd(config, &home.display().to_string(), true);
    }
    let (_, cwd) = active_window_details(&config.tmux_session)?;
    create_agent_window_in_cwd(config, &cwd, agent_id)
}
pub fn create_agent_window_in_cwd(
    config: &Config,
    cwd: &str,
    agent_id: &str,
) -> Result<u32> {
    if agent_id == "console" {
        return create_terminal_window_in_cwd(config, cwd, true);
    }
    crate::session::create_quick_agent(
        config,
        cwd,
        agent_id,
        crate::session::ManifestSource::Cli,
        true,
    )
    .map(|created| created.index)
}
pub fn create_grok_window(config: &Config) -> Result<u32> {
    create_agent_window(config, DEFAULT_AGENT_APP)
}
pub fn create_agent_window_with_launch(
    config: &Config,
    cwd: &str,
    launch_command: &str,
    focus: bool,
) -> Result<u32> {
    crate::session::create_with_launch_command(
        config,
        cwd,
        launch_command,
        crate::session::ManifestSource::NewChat,
        focus,
        None,
        None,
    )
    .map(|created| created.index)
}
