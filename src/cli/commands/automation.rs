//! CLI for CLI-agnostic scheduled automations.

use crate::automation::{
    self, humanize_schedule, slugify_id, Automation, AutomationStatus, SchedulePreset,
};
use crate::config::Config;
use anyhow::{bail, Context, Result};
use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum AutomationCommands {
    /// List automation definitions
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one automation
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Create an automation
    Create {
        /// Display name
        #[arg(long)]
        name: String,
        /// Agent id: grok, codex, claude, opencode
        #[arg(long, default_value = "grok")]
        agent: String,
        /// Model id (agent default if omitted)
        #[arg(long)]
        model: Option<String>,
        /// Working directory (project root)
        #[arg(long)]
        cwd: String,
        /// Prompt body (or use --prompt-file)
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long)]
        prompt_file: Option<String>,
        /// RRULE body, e.g. FREQ=DAILY;BYHOUR=9;BYMINUTE=0
        #[arg(long, default_value = "FREQ=DAILY;BYHOUR=9;BYMINUTE=0")]
        rrule: String,
        /// Stable id (default: slug of name)
        #[arg(long)]
        id: Option<String>,
        /// Start paused
        #[arg(long)]
        paused: bool,
    },
    /// Pause scheduling
    Pause { id: String },
    /// Resume scheduling
    Resume { id: String },
    /// Delete an automation and its run history
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Fire an automation immediately (ignore schedule)
    Run { id: String },
    /// List recent runs
    Runs {
        #[arg(long)]
        automation: Option<String>,
        #[arg(long)]
        unread: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Mark run(s) read
    MarkRead {
        /// Run id, or "all"
        target: String,
        #[arg(long)]
        automation: Option<String>,
    },
    /// Open the Automations panel in the workspace pane
    Ui,
}

pub fn dispatch(command: AutomationCommands) -> Result<()> {
    let config = Config::default();
    match command {
        AutomationCommands::List { json } => run_list(&config, json),
        AutomationCommands::Show { id, json } => run_show(&config, &id, json),
        AutomationCommands::Create {
            name,
            agent,
            model,
            cwd,
            prompt,
            prompt_file,
            rrule,
            id,
            paused,
        } => run_create(
            &config,
            name,
            agent,
            model,
            cwd,
            prompt,
            prompt_file,
            rrule,
            id,
            paused,
        ),
        AutomationCommands::Pause { id } => run_set_status(&config, &id, AutomationStatus::Paused),
        AutomationCommands::Resume { id } => run_set_status(&config, &id, AutomationStatus::Active),
        AutomationCommands::Delete { id, yes } => run_delete(&config, &id, yes),
        AutomationCommands::Run { id } => run_now(&config, &id),
        AutomationCommands::Runs {
            automation,
            unread,
            json,
            limit,
        } => run_list_runs(&config, automation.as_deref(), unread, json, limit),
        AutomationCommands::MarkRead { target, automation } => {
            run_mark_read(&config, &target, automation.as_deref())
        }
        AutomationCommands::Ui => run_ui(),
    }
}

fn run_list(config: &Config, json: bool) -> Result<()> {
    automation::ensure_root(config)?;
    let items = automation::list_automations(config)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if items.is_empty() {
        println!("no automations — create one with `sessions automation create` or ⌘A");
        return Ok(());
    }
    let salt = automation::load_or_create_jitter_salt(config).unwrap_or_default();
    for a in items {
        let status = match a.status {
            AutomationStatus::Active => "active",
            AutomationStatus::Paused => "paused",
        };
        let next = automation::next_fire_after(&a, chrono::Utc::now(), &salt)
            .ok()
            .flatten()
            .map(|t: chrono::DateTime<chrono::Utc>| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "—".into());
        println!(
            "{:<24} {:<8} {:<10} {:<16} next {}",
            a.id,
            status,
            a.agent,
            humanize_schedule(&a),
            next
        );
        println!("  {}", a.name);
    }
    let unread = automation::unread_count(config).unwrap_or(0);
    if unread > 0 {
        println!("\n{unread} unread run(s) — `sessions automation runs --unread`");
    }
    Ok(())
}

fn run_show(config: &Config, id: &str, json: bool) -> Result<()> {
    let a = automation::load_automation(config, id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&a)?);
        return Ok(());
    }
    let state = automation::load_state(config, id).unwrap_or_default();
    println!("id:          {}", a.id);
    println!("name:        {}", a.name);
    println!("status:      {:?}", a.status);
    println!("agent:       {} ({})", a.agent, a.model);
    println!("schedule:    {}", humanize_schedule(&a));
    println!("rrule:       {}", a.rrule);
    println!("cwd:         {}", a.primary_cwd().unwrap_or("—"));
    println!("last_fired:  {:?}", state.last_fired_at);
    println!("next_due:    {:?}", state.next_due_at);
    println!("failures:    {}", state.consecutive_failures);
    println!("prompt:\n{}", a.prompt);
    Ok(())
}

fn run_create(
    config: &Config,
    name: String,
    agent: String,
    model: Option<String>,
    cwd: String,
    prompt: Option<String>,
    prompt_file: Option<String>,
    rrule: String,
    id: Option<String>,
    paused: bool,
) -> Result<()> {
    let prompt = match (prompt, prompt_file) {
        (Some(p), None) => p,
        (None, Some(path)) => {
            std::fs::read_to_string(&path).with_context(|| format!("read prompt file {path}"))?
        }
        (Some(_), Some(_)) => bail!("pass only one of --prompt or --prompt-file"),
        (None, None) => bail!("pass --prompt or --prompt-file"),
    };
    if crate::agents::agent_by_id(&agent).is_none() && agent != "console" {
        bail!("unknown agent `{agent}` — use grok, codex, claude, or opencode");
    }
    if agent == "console" {
        bail!("console agent is not supported for automations");
    }
    let model = model.unwrap_or_else(|| crate::agents::default_model_id(&agent).to_string());
    let cwd = automation::store::expand_cwd(&cwd)?;
    let id = id.unwrap_or_else(|| slugify_id(&name));
    if automation::load_automation(config, &id).is_ok() {
        bail!("automation already exists: {id}");
    }
    // Validate rrule parses for cron
    let mut a = Automation::new(
        id.clone(),
        name,
        prompt,
        agent,
        model,
        rrule.trim().trim_start_matches("RRULE:").to_string(),
        cwd,
    );
    if paused {
        a.status = AutomationStatus::Paused;
    }
    // Validate schedule
    let salt = automation::load_or_create_jitter_salt(config)?;
    let _ = automation::next_fire_after(&a, chrono::Utc::now(), &salt)
        .with_context(|| "invalid schedule / rrule")?;
    automation::save_automation(config, &a)?;
    println!("created automation {}", a.id);
    let _ = SchedulePreset::from_rrule(&a.rrule);
    Ok(())
}

fn run_set_status(config: &Config, id: &str, status: AutomationStatus) -> Result<()> {
    let mut a = automation::load_automation(config, id)?;
    a.status = status;
    a.touch();
    automation::save_automation(config, &a)?;
    println!("{id}: {:?}", status);
    Ok(())
}

fn run_delete(config: &Config, id: &str, yes: bool) -> Result<()> {
    let _ = automation::load_automation(config, id)?;
    if !yes {
        bail!("refusing to delete without --yes (removes run history for {id})");
    }
    automation::delete_automation(config, id)?;
    println!("deleted {id}");
    Ok(())
}

fn run_now(config: &Config, id: &str) -> Result<()> {
    let a = automation::load_automation(config, id)?;
    let run = automation::fire_automation(config, &a, false)?;
    println!(
        "started run {} (window {:?}, session {:?})",
        run.id, run.window_index, run.sessions_session_id
    );
    Ok(())
}

fn run_list_runs(
    config: &Config,
    automation_id: Option<&str>,
    unread_only: bool,
    json: bool,
    limit: usize,
) -> Result<()> {
    let mut runs = match automation_id {
        Some(id) => automation::list_runs(config, id, limit)?,
        None => automation::list_all_runs(config, limit)?,
    };
    if unread_only {
        runs.retain(|r| r.unread);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
        return Ok(());
    }
    if runs.is_empty() {
        println!("no runs");
        return Ok(());
    }
    for r in runs {
        let unread = if r.unread { "*" } else { " " };
        println!(
            "{unread} {}  {}  {:?}  {}  {}",
            r.id,
            r.automation_id,
            r.status,
            r.agent,
            r.started_at
                .with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
        );
        if let Some(err) = &r.error {
            println!("    error: {err}");
        }
    }
    Ok(())
}

fn run_mark_read(config: &Config, target: &str, automation_id: Option<&str>) -> Result<()> {
    if target == "all" {
        let n = automation::mark_all_read(config)?;
        println!("marked {n} run(s) read");
        return Ok(());
    }
    let auto_id = automation_id.context("pass --automation <id> when marking a single run")?;
    automation::mark_run_read(config, auto_id, target)?;
    println!("marked {target} read");
    Ok(())
}

fn run_ui() -> Result<()> {
    crate::bar::run_automations()
}
