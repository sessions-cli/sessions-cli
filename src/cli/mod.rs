mod commands;
mod tmux;

use clap::{Parser, Subcommand};
use commands::{agent, bar, daemon, hooks, notify, session};
use crate::config::Config;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "sessions",
    version = crate::version::VERSION,
    about = "Agent session manager — tmux workspaces + Ratatui sidebar"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run the session daemon
    Daemon {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value_t = 1500)]
        poll_interval: u64,
        #[arg(long)]
        foreground: bool,
    },
    /// Ratatui session sidebar (left pane)
    Bar {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Sessions settings panel (tmux popup over the workspace pane)
    Settings,
    /// New session launcher in the workspace pane (right panel)
    #[command(name = "new-session", alias = "new-chat")]
    NewSession,
    /// Fire-and-forget hook bridge
    Notify {
        #[arg(long)]
        event: String,
        #[arg(long)]
        payload: Option<String>,
        /// Read prompt/session_start JSON from stdin (agent hook mode)
        #[arg(long)]
        stdin: bool,
    },
    /// Print sessions JSON
    List {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Focus workspace by ordered session number (1-based; 0 key maps to 10)
    Focus {
        index: u32,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Tell the daemon to resync tmux state (used by tmux instant-key hooks)
    Refresh {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Create a new tmux workspace window and focus it
    New,
    /// Create a new tmux workspace window with grok and focus it
    Grok,
    /// Create a new tmux workspace window with codex and focus it
    Codex,
    /// Create a new tmux workspace window with opencode and focus it
    Opencode,
    /// Create a managed agent window from tmux instant keys (M-g / M-c / M-o / M-t)
    #[command(hide = true)]
    CreateInstant {
        agent: String,
    },
    /// Close the currently focused tmux workspace window
    Close,
    /// Prompt before closing the currently focused tmux workspace window
    ConfirmClose,
    /// Detach the current tmux client from the sessions UI
    Leave,
    /// Validate install health (binary, PATH, hooks, signature)
    Doctor {
        #[arg(long)]
        json: bool,
        /// Suppress output when healthy (for install scripts)
        #[arg(long)]
        quiet: bool,
        /// Opt-in manifest repair (tombstone stale orphans, fix corrupted launch_command)
        #[arg(long)]
        repair: bool,
    },
    /// Daemon health check
    Status {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
        #[arg(long)]
        json: bool,
    },
    /// Kill the tmux sessions used by sessions UI
    Down,
    /// Bootstrap daemon + workspaces, then attach tmux UI
    Up,
    /// Finish daemon reconcile after reload (internal; used by bin/reload.sh)
    #[command(hide = true)]
    Reconcile {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Agent hook integration
    Hooks {
        #[command(subcommand)]
        command: HooksCommands,
    },
    /// Tmux integration
    Tmux {
        #[command(subcommand)]
        command: TmuxCommands,
    },
    /// Toggle workspace-pane overlays (new session form, settings)
    Panel {
        #[command(subcommand)]
        command: PanelCommands,
    },
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommands,
    },
    Upgrade {
        #[arg(long)]
        check: bool,
        #[arg(long)]
        channel: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum TelemetryCommands {
    Status,
    Enable {
        level: Option<String>,
    },
    Disable,
    Log,
    Export {
        output: Option<String>,
    },
    Check,
}

#[derive(Subcommand)]
pub enum PanelCommands {
    /// Toggle the new session form in the workspace pane
    #[command(name = "new-session", alias = "new-chat")]
    NewSession,
    /// Open the new session form without toggling it closed
    #[command(name = "open-new-session", alias = "open-new-chat")]
    OpenNewSession,
    /// Toggle settings in the workspace pane
    Settings,
    /// Close an open panel overlay without toggling it back on
    Dismiss,
}

#[derive(Subcommand)]
pub enum HooksCommands {
    /// Show hook status for detected agents (or one agent)
    Status {
        #[arg(long)]
        json: bool,
        /// Agent id: grok, codex, claude, or opencode
        agent: Option<String>,
    },
    /// Configure hooks for detected agents (or one agent)
    Setup {
        /// Agent id: grok, codex, claude, or opencode
        agent: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum TmuxCommands {
    /// Create tmux session from workspaces.toml if missing
    Bootstrap,
    /// Attach to the sessions tmux workspace session (replaces current process)
    Attach,
    /// Sidebar + attach split (no Kitty)
    Ui {
        #[command(subcommand)]
        command: UiCommands,
    },
}

#[derive(Subcommand)]
pub enum UiCommands {
    /// Create sessions-ui tmux session (sidebar left, workspaces right)
    Bootstrap,
    /// Attach to sessions-ui (replaces current process)
    Attach,
}

pub(crate) fn config_with_socket(socket: Option<PathBuf>) -> Config {
    let mut config = Config::default();
    if let Some(p) = socket {
        config.socket_path = p;
    }
    config
}

pub fn dispatch(command: Option<Commands>) -> anyhow::Result<()> {
    let invoked_as_daemon = std::env::args()
        .next()
        .is_some_and(|a| a.ends_with("sessionsd"));

    if invoked_as_daemon {
        return daemon::run(None, 1500, false);
    }

    match command {
        Some(Commands::Daemon {
            socket,
            poll_interval,
            foreground,
        }) => daemon::run(socket, poll_interval, foreground),
        Some(Commands::Bar { socket }) => bar::run(socket),
        Some(Commands::Settings) => bar::run_settings(),
        Some(Commands::NewSession) => bar::run_new_session(),
        Some(Commands::Notify {
            event,
            payload,
            stdin,
        }) => notify::run(&event, payload.as_deref(), stdin),
        Some(Commands::List { socket }) => session::run_list(socket),
        Some(Commands::Focus { index, socket }) => session::run_focus(socket, index),
        Some(Commands::Refresh { socket }) => session::run_refresh(socket),
        Some(Commands::New) => session::run_new(),
        Some(Commands::Grok) => agent::run(agent::CLI_AGENT_ALIASES[0]),
        Some(Commands::Codex) => agent::run(agent::CLI_AGENT_ALIASES[1]),
        Some(Commands::Opencode) => agent::run(agent::CLI_AGENT_ALIASES[2]),
        Some(Commands::CreateInstant { agent }) => agent::run_create_instant(&agent),
        Some(Commands::Close) => session::run_close(),
        Some(Commands::ConfirmClose) => session::run_confirm_close(),
        Some(Commands::Leave) => session::run_leave(),
        Some(Commands::Doctor { json, quiet, repair }) => {
            session::run_doctor(json, quiet, repair)
        }
        Some(Commands::Status {
            socket,
            verbose,
            json,
        }) => session::run_status(socket, verbose, json),
        Some(Commands::Down) => session::run_down(),
        Some(Commands::Up) => session::run_up(),
        Some(Commands::Reconcile { socket }) => session::run_reconcile(socket),
        Some(Commands::Hooks { command }) => match command {
            HooksCommands::Status { json, agent } => hooks::run_status(agent.as_deref(), json),
            HooksCommands::Setup { agent } => hooks::run_setup(agent.as_deref()),
        },
        Some(Commands::Tmux { command }) => match command {
            TmuxCommands::Bootstrap => tmux::run_bootstrap(),
            TmuxCommands::Attach => tmux::run_attach(),
            TmuxCommands::Ui { command } => match command {
                UiCommands::Bootstrap => tmux::run_ui_bootstrap(),
                UiCommands::Attach => tmux::run_ui_attach(),
            },
        },
        Some(Commands::Panel { command }) => match command {
            PanelCommands::NewSession => session::run_panel_new_session(),
            PanelCommands::OpenNewSession => session::run_panel_open_new_session(),
            PanelCommands::Settings => session::run_panel_settings(),
            PanelCommands::Dismiss => session::run_panel_dismiss(),
        },
        Some(Commands::Telemetry { command }) => match command {
            TelemetryCommands::Status => crate::telemetry::cli::run_status(),
            TelemetryCommands::Enable { level } => {
                crate::telemetry::cli::run_enable(level.as_deref())
            }
            TelemetryCommands::Disable => crate::telemetry::cli::run_disable(),
            TelemetryCommands::Log => crate::telemetry::cli::run_log(),
            TelemetryCommands::Export { output } => {
                crate::telemetry::cli::run_export(output.as_deref())
            }
            TelemetryCommands::Check => crate::telemetry::cli::run_check_now(),
        },
        Some(Commands::Upgrade { check, channel }) => {
            crate::upgrade::run_upgrade(check, channel.as_deref())
        }
        None => session::run_up(),
    }
}