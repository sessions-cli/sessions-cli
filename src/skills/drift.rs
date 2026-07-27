//! Drift detection: store skills vs agent skill directories.

use super::paths::SkillAgent;
use super::scan::{agent_has_skill, SkillsInventory};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DriftKind {
    /// In store but missing from agent primary scan paths.
    MissingOnAgent,
    /// On agent but not in skillshare store (local-only / imported elsewhere).
    OnlyOnAgent,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DriftItem {
    pub skill: String,
    pub agent: String,
    pub kind: DriftKind,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DriftReport {
    pub items: Vec<DriftItem>,
    /// Agents with zero MissingOnAgent for store skills.
    pub agents_in_sync: Vec<String>,
}

/// Compare store library to agent presence.
///
/// - For each store skill: agents that lack it → MissingOnAgent
/// - For each non-store skill on an agent → OnlyOnAgent (informational)
pub fn detect_drift(home: &Path, inventory: &SkillsInventory) -> DriftReport {
    let store_names: std::collections::BTreeSet<&str> = inventory
        .store_skills
        .iter()
        .map(|s| s.name.as_str())
        .collect();

    let mut items = Vec::new();
    let mut agents_in_sync = Vec::new();

    for agent in SkillAgent::ALL {
        let mut missing = 0usize;
        for skill in &inventory.store_skills {
            if !agent_has_skill(home, agent, &skill.name) {
                missing += 1;
                items.push(DriftItem {
                    skill: skill.name.clone(),
                    agent: agent.id().to_string(),
                    kind: DriftKind::MissingOnAgent,
                    detail: format!("{} missing from {}", skill.name, agent.label()),
                });
            }
        }
        if missing == 0 && !inventory.store_skills.is_empty() {
            agents_in_sync.push(agent.id().to_string());
        } else if inventory.store_skills.is_empty() {
            // No store → nothing to sync; still report agent-only skills below.
        }
    }

    // Skills only on agents (not in store)
    for name in &inventory.all_names {
        if store_names.contains(name.as_str()) {
            continue;
        }
        if let Some(agents) = inventory.presence.get(name) {
            for agent_id in agents {
                items.push(DriftItem {
                    skill: name.clone(),
                    agent: agent_id.clone(),
                    kind: DriftKind::OnlyOnAgent,
                    detail: format!("{name} on {agent_id} but not in skillshare store"),
                });
            }
        }
    }

    items.sort_by(|a, b| {
        a.kind
            .cmp_rank()
            .cmp(&b.kind.cmp_rank())
            .then(a.skill.cmp(&b.skill))
            .then(a.agent.cmp(&b.agent))
    });

    DriftReport {
        items,
        agents_in_sync,
    }
}

impl DriftKind {
    fn cmp_rank(self) -> u8 {
        match self {
            DriftKind::MissingOnAgent => 0,
            DriftKind::OnlyOnAgent => 1,
        }
    }
}

/// Compact presence flags for UI: one bool per SkillAgent::ALL order.
pub fn presence_matrix_row(
    inventory: &SkillsInventory,
    skill_name: &str,
) -> Vec<(SkillAgent, bool)> {
    let present = inventory.presence.get(skill_name);
    SkillAgent::ALL
        .iter()
        .map(|agent| {
            let ok = present.map(|set| set.contains(agent.id())).unwrap_or(false);
            (*agent, ok)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::paths::skillshare_store_dir;
    use crate::skills::scan::collect_inventory;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detects_missing_on_agent() {
        let tmp = tempdir().unwrap();
        let home = tmp.path();
        let store = skillshare_store_dir(home);
        fs::create_dir_all(store.join("help")).unwrap();
        fs::write(store.join("help/SKILL.md"), "---\ndescription: x\n---\n").unwrap();
        // no agent dirs
        let inv = collect_inventory(home);
        let report = detect_drift(home, &inv);
        assert!(report
            .items
            .iter()
            .any(|i| i.skill == "help" && i.kind == DriftKind::MissingOnAgent));
    }
}
