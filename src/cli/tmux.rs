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
    let sessions_bin = daemon::tmux::sessions_binary();
    daemon::tmux::bootstrap_ui_session(
        &config.tmux_ui_session,
        &config.tmux_session,
        &sessions_bin,
    )?;
    daemon::tmux::respawn_sidebar_bar(&config.tmux_ui_session, &sessions_bin)?;
    // Reload/up path: kill bare host attaches on agents without touching the
    // nested workspace client inside sessions-ui.
    if let Ok(detached) =
        daemon::tmux::detach_stray_agents_clients(&config.tmux_ui_session, &config.tmux_session)
    {
        for tty in &detached {
            eprintln!("detached stray agents client: {tty}");
        }
    }
    daemon::tmux::verify_ui_runtime(&config.tmux_ui_session, &config.tmux_session)
}

pub fn run_ui_attach() -> Result<()> {
    let config = Config::default();
    if !daemon::tmux::session_exists(&config.tmux_ui_session) {
        run_ui_bootstrap()?;
    }
    daemon::tmux::attach_ui_session(&config.tmux_ui_session)
}
