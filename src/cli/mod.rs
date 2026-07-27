mod commands;
mod tmux;

use crate::config::Config;
use clap::{Parser, Subcommand};
use commands::{agent, automation, bar, daemon, hooks, notify, opencode_question, session, skill};
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
    /// Automations manager in the workspace pane
    #[command(name = "automations", alias = "automation-ui")]
    Automations,
    /// MCPs management portal (workspace pane) and CLI helpers
    #[command(name = "mcps", alias = "mcp")]
    Mcps {
        #[command(subcommand)]
        command: Option<McpsCommands>,
    },
    /// Skills management portal in the workspace pane
    #[command(name = "skills", alias = "skills-ui")]
    Skills,
    /// Manage skills via skillshare (list, status, sync, doctor)
    #[command(name = "skill", alias = "skills-cli")]
    Skill {
        #[command(subcommand)]
        command: skill::SkillCommands,
    },
    /// Manage scheduled automations
    #[command(name = "automation", alias = "automations-cli")]
    Automation {
        #[command(subcommand)]
        command: automation::AutomationCommands,
    },
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
    /// Grow or shrink the sessions sidebar pane (columns)
    #[command(name = "resize-sidebar", alias = "sidebar-resize")]
    ResizeSidebar {
        /// `wider` or `narrower` (aliases: grow/shrink, +/-)
        direction: String,
        /// Columns per step (default: 4)
        #[arg(long, short = 's')]
        step: Option<u16>,
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
    CreateInstant { agent: String },
    /// Paste OS clipboard into a tmux pane (internal; used by C-v / M-v binds)
    #[command(hide = true, name = "paste-tmux")]
    PasteTmux {
        /// Target pane id (e.g. %42). Prefer this when multiple clients are attached.
        #[arg(short = 't', long = "target")]
        target: Option<String>,
        /// Only load OS clipboard into the tmux buffer (key binding runs paste-buffer)
        #[arg(long = "load-only")]
        load_only: bool,
    },
    /// Answer an OpenCode question request in a popup (bypasses broken TUI Enter)
    #[command(hide = true, name = "opencode-question")]
    OpencodeQuestion {
        /// Path to question request JSON written by the OpenCode sessions plugin
        #[arg(long)]
        request: PathBuf,
        /// Path to write answers JSON (`string[][]`) for the plugin to submit
        #[arg(long)]
        output: PathBuf,
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
    Up {
        /// Bootstrap only (no tmux attach) — for scripts after install/reload
        #[arg(long)]
        no_attach: bool,
    },
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
    Enable { level: Option<String> },
    Disable,
    Log,
    Export { output: Option<String> },
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
    /// Open automations manager
    #[command(name = "automations", alias = "open-automations")]
    Automations,
    /// Open MCPs management portal
    #[command(name = "mcps")]
    Mcps,
    /// Open MCPs management portal (open-only)
    #[command(name = "open-mcps")]
    OpenMcps,
    /// Open Skills management portal
    #[command(name = "skills")]
    Skills,
    /// Open Skills management portal (open-only)
    #[command(name = "open-skills")]
    OpenSkills,
    /// Close an open panel overlay without toggling it back on
    Dismiss,
}

#[derive(Subcommand)]
pub enum McpsCommands {
    /// Show MCP manager backend status
    Status,
    /// List known MCP servers (requires domain backend)
    List,
    /// Search the MCP catalog for servers to add
    Search {
        /// Free-text query (name / description). Empty lists the catalog.
        query: Vec<String>,
        /// Max results (default 50)
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Add (deploy) a catalog entry as a personal MCP server
    Add {
        /// Catalog entry id (from `sessions mcps search`)
        entry_id: String,
        /// Optional alias / agent config key
        #[arg(long)]
        alias: Option<String>,
    },
    /// Sync enablement matrix into agent configs
    Sync {
        /// Print planned changes without writing
        #[arg(long)]
        dry_run: bool,
    },
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
        Some(Commands::Automations) => bar::run_automations(),
        Some(Commands::Mcps { command }) => match command {
            None => bar::run_mcps(),
            Some(McpsCommands::Status) => mcps_cli_status(),
            Some(McpsCommands::List) => mcps_cli_list(),
            Some(McpsCommands::Search { query, limit, json }) => {
                mcps_cli_search(&query, limit, json)
            }
            Some(McpsCommands::Add { entry_id, alias }) => {
                mcps_cli_add(&entry_id, alias.as_deref())
            }
            Some(McpsCommands::Sync { dry_run }) => mcps_cli_sync(dry_run),
        },
        Some(Commands::Skills) => bar::run_skills(),
        Some(Commands::Skill { command }) => skill::dispatch(command),
        Some(Commands::Automation { command }) => automation::dispatch(command),
        Some(Commands::Notify {
            event,
            payload,
            stdin,
        }) => notify::run(&event, payload.as_deref(), stdin),
        Some(Commands::List { socket }) => session::run_list(socket),
        Some(Commands::Focus { index, socket }) => session::run_focus(socket, index),
        Some(Commands::ResizeSidebar { direction, step }) => {
            session::run_resize_sidebar(&direction, step)
        }
        Some(Commands::Refresh { socket }) => session::run_refresh(socket),
        Some(Commands::New) => session::run_new(),
        Some(Commands::Grok) => agent::run(agent::CLI_AGENT_ALIASES[0]),
        Some(Commands::Codex) => agent::run(agent::CLI_AGENT_ALIASES[1]),
        Some(Commands::Opencode) => agent::run(agent::CLI_AGENT_ALIASES[2]),
        Some(Commands::CreateInstant { agent }) => agent::run_create_instant(&agent),
        Some(Commands::PasteTmux { target, load_only }) => {
            if load_only {
                crate::clipboard::load_os_clipboard_into_tmux_buffer()
            } else {
                crate::clipboard::paste_into_tmux_pane(target.as_deref())
            }
        }
        Some(Commands::OpencodeQuestion { request, output }) => {
            opencode_question::run(request, output)
        }
        Some(Commands::Close) => session::run_close(),
        Some(Commands::ConfirmClose) => session::run_confirm_close(),
        Some(Commands::Leave) => session::run_leave(),
        Some(Commands::Doctor {
            json,
            quiet,
            repair,
        }) => session::run_doctor(json, quiet, repair),
        Some(Commands::Status {
            socket,
            verbose,
            json,
        }) => session::run_status(socket, verbose, json),
        Some(Commands::Down) => session::run_down(),
        Some(Commands::Up { no_attach }) => session::run_up(no_attach),
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
            PanelCommands::Automations => session::run_panel_automations(),
            PanelCommands::Mcps | PanelCommands::OpenMcps => session::run_panel_open_mcps(),
            PanelCommands::Skills | PanelCommands::OpenSkills => session::run_panel_open_skills(),
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
        None => session::run_up(false),
    }
}

fn mcps_cli_status() -> anyhow::Result<()> {
    let config = Config::default();
    let health = crate::mcp::health(&config.home)?;
    let status = match health.status {
        crate::mcp::ObotHealthStatus::Up => "up",
        crate::mcp::ObotHealthStatus::Down => "down",
        crate::mcp::ObotHealthStatus::Disabled => "disabled",
    };
    println!("obot: {status}");
    println!("url: {}", health.base_url);
    println!("detail: {}", health.detail);
    println!(
        "token: {}",
        if health.token_configured {
            "configured"
        } else {
            "missing"
        }
    );
    let inv = crate::mcp::list_inventory(&config.home).unwrap_or_default();
    println!("servers: {}", inv.len());
    let drift = crate::mcp::detect_drift(&config.home).unwrap_or_default();
    println!("drift: {}", drift.len());
    Ok(())
}

fn mcps_cli_list() -> anyhow::Result<()> {
    let config = Config::default();
    let inv = crate::mcp::list_inventory(&config.home)?;
    if inv.is_empty() {
        println!("(no servers)");
        return Ok(());
    }
    let matrix = crate::mcp::load_enablement(&config.home).unwrap_or_default();
    for server in inv {
        let source = match &server.source {
            crate::mcp::ServerSource::ObotGateway { connect_url, .. } => {
                format!("obot {connect_url}")
            }
            crate::mcp::ServerSource::LocalOnly { command, args } => {
                format!("local {command} {}", args.join(" "))
            }
        };
        let enabled: Vec<String> = ["grok", "codex", "claude", "opencode"]
            .iter()
            .filter_map(|agent| {
                matrix
                    .is_enabled(&server.key, agent)
                    .filter(|&on| on)
                    .map(|_| (*agent).to_string())
            })
            .collect();
        let agents = if enabled.is_empty() {
            "—".into()
        } else {
            enabled.join(",")
        };
        println!(
            "{:<16} {:<8} agents=[{agents}]  {source}",
            server.key,
            match server.source {
                crate::mcp::ServerSource::ObotGateway { .. } => "obot",
                crate::mcp::ServerSource::LocalOnly { .. } => "local",
            }
        );
    }
    Ok(())
}

fn mcps_cli_sync(dry_run: bool) -> anyhow::Result<()> {
    let config = Config::default();
    let report = if dry_run {
        crate::mcp::dry_run(&config.home)?
    } else {
        crate::mcp::sync_all(&config.home)?
    };
    let kind = if dry_run { "dry-run" } else { "sync" };
    println!("{kind}: {} change(s)", report.change_count());
    for change in &report.changes {
        println!(
            "  {}  {}  {:?}  {}",
            change.agent_id, change.server_key, change.action, change.detail
        );
    }
    for err in &report.errors {
        println!("error: {err}");
    }
    Ok(())
}

fn mcps_cli_search(query_parts: &[String], limit: usize, json: bool) -> anyhow::Result<()> {
    let config = Config::default();
    let query = query_parts.join(" ");
    let mut entries = match crate::mcp::search_catalog(&config.home, &query) {
        Ok(e) => e,
        Err(err) => {
            // Soft-fallback to registry API.
            eprintln!("catalog entries: {err}; trying registry search…");
            crate::mcp::search_registry(&config.home, &query, limit)?
        }
    };
    if limit > 0 && entries.len() > limit {
        entries.truncate(limit);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!("(no catalog matches for {query:?})");
        println!("hint: open Catalog with `sessions mcps` then press o, or set up Obot");
        return Ok(());
    }
    println!("{:<28} {:<22} DESCRIPTION", "NAME", "ENTRY_ID");
    for e in entries {
        let desc = e.summary().chars().take(48).collect::<String>();
        println!(
            "{:<28} {:<22} {}",
            truncate_cli(&e.name, 28),
            truncate_cli(&e.id, 22),
            desc
        );
    }
    println!();
    println!("add with: sessions mcps add <ENTRY_ID> [--alias name]");
    Ok(())
}

fn mcps_cli_add(entry_id: &str, alias: Option<&str>) -> anyhow::Result<()> {
    let config = Config::default();
    let created = crate::mcp::create_server_from_entry(&config.home, entry_id, alias)?;
    println!(
        "added: {}  key={}  id={}  connect={}",
        created.display_name, created.key, created.id, created.connect_url
    );
    if created.missing_oauth || !created.configured {
        println!("note: needs OAuth/config in Catalog before tools work");
    }
    // Stage enablement for detected agents.
    let mut matrix = crate::mcp::load_enablement(&config.home).unwrap_or_default();
    for agent in crate::hooks::detect_agents(&config.home) {
        matrix.set(&created.key, agent.id, true);
    }
    crate::mcp::save_enablement(&config.home, &matrix)?;
    println!("enablement staged for detected agents — run: sessions mcps sync");
    Ok(())
}

fn truncate_cli(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".into();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}
