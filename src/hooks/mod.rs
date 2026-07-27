pub mod providers;

use anyhow::Result;
use std::path::Path;

pub use crate::agents::registry::AGENT_IDS;
pub use crate::agents::AgentHookReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupSummary {
    pub configured: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<(String, String)>,
}

pub fn command_on_path(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    for dir in path.split(':').filter(|part| !part.is_empty()) {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return true;
        }
    }
    false
}

pub fn agent_report(home: &Path, agent: &str) -> AgentHookReport {
    if let Some(hooks) = crate::agents::registry::hook_provider_by_id(agent) {
        return hooks.hook_report(home);
    }
    AgentHookReport {
        id: "unknown",
        present: false,
        detail: "unsupported agent".into(),
        needs_setup: false,
    }
}

pub fn detect_agents(home: &Path) -> Vec<AgentHookReport> {
    AGENT_IDS
        .iter()
        .filter_map(|id| crate::agents::registry::hook_provider_by_id(id))
        .map(|hooks| hooks.hook_report(home))
        .filter(|report| report.present)
        .collect()
}

pub fn integrations_summary(home: &Path) -> String {
    let detected = detect_agents(home);
    if detected.is_empty() {
        return "no agents detected".into();
    }
    detected
        .iter()
        .map(|report| format!("{}: {}", report.id, report.detail))
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn setup_agent(home: &Path, agent: &str) -> Result<()> {
    if let Some(hooks) = crate::agents::registry::hook_provider_by_id(agent) {
        return hooks.setup(home);
    }
    anyhow::bail!("unsupported agent: {agent}");
}

/// Configure hooks for every detected agent (idempotent).
pub fn setup_detected(home: &Path) -> SetupSummary {
    let mut summary = SetupSummary {
        configured: Vec::new(),
        skipped: Vec::new(),
        failed: Vec::new(),
    };

    for id in AGENT_IDS {
        let id = *id;
        let before = agent_report(home, id);
        if !before.present {
            continue;
        }
        match setup_agent(home, id) {
            Ok(()) => {
                let after = agent_report(home, id);
                if before.needs_setup && !after.needs_setup {
                    summary.configured.push(id.to_string());
                } else if !before.needs_setup && !after.needs_setup {
                    summary.skipped.push(id.to_string());
                } else {
                    summary.configured.push(id.to_string());
                }
            }
            Err(err) => summary.failed.push((id.to_string(), err.to_string())),
        }
    }

    summary
}
