//! Scan skillshare store and agent skill directories for SKILL.md packages.

use super::paths::{agent_scan_dirs, skillshare_store_dir, SkillAgent};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// One skill package (directory containing SKILL.md or SKILL.md.disabled).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillPackage {
    pub name: String,
    pub path: PathBuf,
    pub description: String,
    pub enabled: bool,
    /// True when this skill lives only under a read-only/bundled path for some agent.
    pub bundled: bool,
}

/// Inventory across store + all agents.
#[derive(Debug, Clone, Serialize, Default)]
pub struct SkillsInventory {
    pub store_dir: PathBuf,
    pub store_skills: Vec<SkillPackage>,
    /// skill name → agents that currently have it (enabled or disabled file present)
    pub presence: BTreeMap<String, BTreeSet<String>>,
    /// All skill names seen (store ∪ agents)
    pub all_names: BTreeSet<String>,
}

/// Read YAML frontmatter `description` from a SKILL.md body (best-effort).
pub fn parse_skill_description(content: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return first_heading_or_empty(trimmed);
    }
    let rest = &trimmed[3..];
    let end = match rest.find("\n---") {
        Some(i) => i,
        None => return first_heading_or_empty(trimmed),
    };
    let front = &rest[..end];
    for line in front.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("description:") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    first_heading_or_empty(&rest[end.saturating_add(4)..])
}

fn first_heading_or_empty(content: &str) -> String {
    for line in content.lines() {
        let line = line.trim();
        if let Some(h) = line.strip_prefix("# ") {
            return h.trim().to_string();
        }
    }
    String::new()
}

/// Scan a directory of skill packages (each child dir with SKILL.md).
pub fn scan_skills_dir(dir: &Path, bundled: bool) -> Vec<SkillPackage> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) if !n.starts_with('.') => n.to_string(),
            _ => continue,
        };
        let skill_md = path.join("SKILL.md");
        let skill_disabled = path.join("SKILL.md.disabled");
        let (enabled, md_path) = if skill_md.is_file() {
            (true, skill_md)
        } else if skill_disabled.is_file() {
            (false, skill_disabled)
        } else {
            continue;
        };
        let description = fs::read_to_string(&md_path)
            .map(|c| parse_skill_description(&c))
            .unwrap_or_default();
        out.push(SkillPackage {
            name,
            path,
            description,
            enabled,
            bundled,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Build full inventory for `home`.
pub fn collect_inventory(home: &Path) -> SkillsInventory {
    let store_dir = skillshare_store_dir(home);
    let store_skills = scan_skills_dir(&store_dir, false);

    let mut presence: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut all_names: BTreeSet<String> = BTreeSet::new();

    for skill in &store_skills {
        all_names.insert(skill.name.clone());
    }

    for agent in SkillAgent::ALL {
        let dirs = agent_scan_dirs(home, agent);
        let mut seen_for_agent: BTreeSet<String> = BTreeSet::new();
        for (idx, dir) in dirs.iter().enumerate() {
            let bundled = idx > 0;
            for skill in scan_skills_dir(dir, bundled) {
                if seen_for_agent.insert(skill.name.clone()) {
                    all_names.insert(skill.name.clone());
                    presence
                        .entry(skill.name.clone())
                        .or_default()
                        .insert(agent.id().to_string());
                }
            }
        }
    }

    SkillsInventory {
        store_dir,
        store_skills,
        presence,
        all_names,
    }
}

/// Whether `agent` has skill `name` on disk.
pub fn agent_has_skill(home: &Path, agent: SkillAgent, name: &str) -> bool {
    for dir in agent_scan_dirs(home, agent) {
        let base = dir.join(name);
        if base.join("SKILL.md").is_file() || base.join("SKILL.md.disabled").is_file() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parse_description_from_frontmatter() {
        let md = "---\nname: help\ndescription: Show help docs\n---\n\n# Help\n";
        assert_eq!(parse_skill_description(md), "Show help docs");
    }

    #[test]
    fn scan_and_inventory() {
        let tmp = tempdir().unwrap();
        let home = tmp.path();
        let store = skillshare_store_dir(home);
        let claude = agent_scan_dirs(home, SkillAgent::Claude)[0].clone();
        fs::create_dir_all(store.join("help")).unwrap();
        fs::write(
            store.join("help/SKILL.md"),
            "---\ndescription: Help skill\n---\n",
        )
        .unwrap();
        fs::create_dir_all(claude.join("help")).unwrap();
        fs::write(
            claude.join("help/SKILL.md"),
            "---\ndescription: Help skill\n---\n",
        )
        .unwrap();
        fs::create_dir_all(claude.join("only-claude")).unwrap();
        fs::write(
            claude.join("only-claude/SKILL.md"),
            "---\ndescription: Local\n---\n",
        )
        .unwrap();

        let inv = collect_inventory(home);
        assert_eq!(inv.store_skills.len(), 1);
        assert_eq!(inv.store_skills[0].name, "help");
        assert!(inv.presence.get("help").unwrap().contains("claude"));
        assert!(inv.all_names.contains("only-claude"));
    }
}
