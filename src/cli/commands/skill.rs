//! CLI: `sessions skill list|status|doctor|sync|init`

use crate::config::Config;
use crate::skills::{self, DriftKind};
use anyhow::{bail, Result};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum SkillCommands {
    /// skillshare + store + drift summary
    Status {
        #[arg(long)]
        json: bool,
    },
    /// List skills in store and agent presence
    List {
        #[arg(long)]
        json: bool,
    },
    /// skillshare binary, store, drift
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Run `skillshare sync`
    Sync,
    /// Run `skillshare init`
    Init,
}

pub fn dispatch(command: SkillCommands) -> Result<()> {
    let config = Config::default();
    match command {
        SkillCommands::Status { json } => run_status(&config, json),
        SkillCommands::List { json } => run_list(&config, json),
        SkillCommands::Doctor { json } => run_doctor(&config, json),
        SkillCommands::Sync => run_sync(),
        SkillCommands::Init => run_init(),
    }
}

fn run_status(config: &Config, json: bool) -> Result<()> {
    let snap = skills::snapshot(&config.home);
    if json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
        return Ok(());
    }
    let ss = &snap.skillshare;
    if ss.installed {
        println!(
            "skillshare: installed{}",
            ss.version
                .as_ref()
                .map(|v| format!(" ({v})"))
                .unwrap_or_default()
        );
        if let Some(bin) = &ss.binary {
            println!("  binary: {}", bin.display());
        }
    } else {
        println!("skillshare: not found");
        println!("  {}", ss.install_hint);
    }
    println!(
        "store: {} ({})",
        ss.store_dir.display(),
        if ss.store_exists { "exists" } else { "missing" }
    );
    println!("  skills: {}", snap.inventory.store_skills.len());
    let missing = snap
        .drift
        .items
        .iter()
        .filter(|i| i.kind == DriftKind::MissingOnAgent)
        .count();
    println!("drift missing-on-agent: {missing}");
    if !snap.drift.agents_in_sync.is_empty() {
        println!("agents in sync: {}", snap.drift.agents_in_sync.join(", "));
    }
    Ok(())
}

fn run_list(config: &Config, json: bool) -> Result<()> {
    let snap = skills::snapshot(&config.home);
    if json {
        println!("{}", serde_json::to_string_pretty(&snap.inventory)?);
        return Ok(());
    }
    println!("Store ({})", snap.inventory.store_dir.display());
    if snap.inventory.store_skills.is_empty() {
        println!("  (empty)");
    }
    for skill in &snap.inventory.store_skills {
        let agents = snap
            .inventory
            .presence
            .get(&skill.name)
            .map(|s| {
                let mut v: Vec<_> = s.iter().cloned().collect();
                v.sort();
                v.join(",")
            })
            .unwrap_or_else(|| "-".into());
        println!(
            "  {}  [{}]  {}",
            skill.name,
            agents,
            if skill.description.is_empty() {
                ""
            } else {
                skill.description.as_str()
            }
        );
    }
    let only_agent: Vec<_> = snap
        .inventory
        .all_names
        .iter()
        .filter(|n| !snap.inventory.store_skills.iter().any(|s| s.name == **n))
        .collect();
    if !only_agent.is_empty() {
        println!("Agent-only (not in store)");
        for name in only_agent {
            let agents = snap
                .inventory
                .presence
                .get(name)
                .map(|s| {
                    let mut v: Vec<_> = s.iter().cloned().collect();
                    v.sort();
                    v.join(",")
                })
                .unwrap_or_default();
            println!("  {name}  [{agents}]");
        }
    }
    Ok(())
}

fn run_doctor(config: &Config, json: bool) -> Result<()> {
    let snap = skills::snapshot(&config.home);
    if json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
        return Ok(());
    }
    let mut issues = 0usize;
    if !snap.skillshare.installed {
        println!("FAIL  skillshare binary not found");
        println!("      {}", snap.skillshare.install_hint);
        issues += 1;
    } else {
        println!(
            "OK    skillshare{}",
            snap.skillshare
                .version
                .as_ref()
                .map(|v| format!(" {v}"))
                .unwrap_or_default()
        );
    }
    if !snap.skillshare.store_exists {
        println!(
            "WARN  store missing: {} (run skillshare init)",
            snap.skillshare.store_dir.display()
        );
        issues += 1;
    } else {
        println!(
            "OK    store {} ({} skills)",
            snap.skillshare.store_dir.display(),
            snap.inventory.store_skills.len()
        );
    }
    let missing = snap
        .drift
        .items
        .iter()
        .filter(|i| i.kind == DriftKind::MissingOnAgent)
        .count();
    if missing > 0 {
        println!("WARN  {missing} store skill placement(s) missing on agents (sync)");
        for item in snap
            .drift
            .items
            .iter()
            .filter(|i| i.kind == DriftKind::MissingOnAgent)
            .take(8)
        {
            println!("      - {}", item.detail);
        }
        issues += 1;
    } else if !snap.inventory.store_skills.is_empty() {
        println!("OK    no missing-on-agent drift for store skills");
    }
    if issues > 0 {
        bail!("skills doctor found {issues} issue(s)");
    }
    println!("skills doctor: healthy");
    Ok(())
}

fn run_sync() -> Result<()> {
    match skills::run_sync() {
        Ok(r) => {
            if !r.stdout.trim().is_empty() {
                print!("{}", r.stdout);
            }
            if !r.stderr.trim().is_empty() {
                eprint!("{}", r.stderr);
            }
            if !r.ok {
                bail!("skillshare sync failed");
            }
            Ok(())
        }
        Err(e) => bail!(e),
    }
}

fn run_init() -> Result<()> {
    match skills::run_init() {
        Ok(r) => {
            if !r.stdout.trim().is_empty() {
                print!("{}", r.stdout);
            }
            if !r.stderr.trim().is_empty() {
                eprint!("{}", r.stderr);
            }
            if !r.ok {
                bail!("skillshare init failed");
            }
            Ok(())
        }
        Err(e) => bail!(e),
    }
}
