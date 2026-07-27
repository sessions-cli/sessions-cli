use crate::agents::common::notify_binary::hook_binary;
use crate::config::Config;
use crate::hooks;
use anyhow::Result;

pub fn run_status(agent: Option<&str>, json: bool) -> Result<()> {
    let config = Config::default();
    let reports: Vec<_> = match agent {
        Some(id) => vec![hooks::agent_report(&config.home, id)],
        None => hooks::detect_agents(&config.home),
    };

    if json {
        let payload: Vec<_> = reports
            .iter()
            .map(|report| {
                serde_json::json!({
                    "agent": report.id,
                    "present": report.present,
                    "detail": report.detail,
                    "needs_setup": report.needs_setup,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    if reports.is_empty() {
        println!("no supported agents detected");
        println!("install grok, codex, claude, or opencode to enable sidebar hooks");
        return Ok(());
    }

    println!("sessions binary: {}", hook_binary(&config.home).display());
    for report in reports {
        println!("{} hooks: {}", report.id, report.detail);
    }
    Ok(())
}

fn print_setup_summary(summary: &hooks::SetupSummary) {
    if !summary.configured.is_empty() {
        println!("configured: {}", summary.configured.join(", "));
    }
    if !summary.skipped.is_empty() {
        println!("already ok: {}", summary.skipped.join(", "));
    }
    for (agent, err) in &summary.failed {
        eprintln!("failed {agent}: {err}");
    }
}

pub fn run_setup(agent: Option<&str>) -> Result<()> {
    let config = Config::default();
    match agent {
        Some(id) => {
            hooks::setup_agent(&config.home, id)?;
            let report = hooks::agent_report(&config.home, id);
            println!("{} hooks: {}", report.id, report.detail);
            Ok(())
        }
        None => {
            let detected = hooks::detect_agents(&config.home);
            if detected.is_empty() {
                println!("no supported agents detected — skipping hook setup");
                return Ok(());
            }
            let summary = hooks::setup_detected(&config.home);
            print_setup_summary(&summary);
            if summary.failed.is_empty() {
                Ok(())
            } else {
                anyhow::bail!("hook setup failed for one or more agents")
            }
        }
    }
}
