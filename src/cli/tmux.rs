use crate::config::Config;
use crate::daemon;
use anyhow::Result;

pub fn run_bootstrap() -> Result<()> {
    let config = Config::default();
    daemon::tmux::bootstrap_session(&config)
}

pub fn run_attach() -> Result<()> {
    let config = Config::default();
    if !daemon::tmux::session_exists(&config.tmux_session) {
        daemon::tmux::bootstrap_session(&config)?;
    }
    daemon::tmux::attach_session(&config.tmux_session)
}

pub fn run_ui_bootstrap() -> Result<()> {
    let config = Config::default();
    if !daemon::tmux::session_exists(&config.tmux_session) {
        daemon::tmux::bootstrap_session(&config)?;
    }
    daemon::tmux::bootstrap_ui_session(
        &config.tmux_ui_session,
        &config.tmux_session,
        &daemon::tmux::sessions_binary(),
    )
}

pub fn run_ui_attach() -> Result<()> {
    let config = Config::default();
    if !daemon::tmux::session_exists(&config.tmux_ui_session) {
        run_ui_bootstrap()?;
    }
    daemon::tmux::attach_ui_session(&config.tmux_ui_session)
}