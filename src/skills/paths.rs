//! Skill directory paths for skillshare store and known agent harnesses.

use std::path::{Path, PathBuf};

/// Agent / harness that can host skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SkillAgent {
    Claude,
    Codex,
    Cursor,
    Grok,
    OpenCode,
    Agents,
}

impl SkillAgent {
    pub const ALL: [SkillAgent; 6] = [
        SkillAgent::Claude,
        SkillAgent::Codex,
        SkillAgent::Cursor,
        SkillAgent::Grok,
        SkillAgent::OpenCode,
        SkillAgent::Agents,
    ];

    pub fn id(self) -> &'static str {
        match self {
            SkillAgent::Claude => "claude",
            SkillAgent::Codex => "codex",
            SkillAgent::Cursor => "cursor",
            SkillAgent::Grok => "grok",
            SkillAgent::OpenCode => "opencode",
            SkillAgent::Agents => "agents",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SkillAgent::Claude => "Claude",
            SkillAgent::Codex => "Codex",
            SkillAgent::Cursor => "Cursor",
            SkillAgent::Grok => "Grok",
            SkillAgent::OpenCode => "OpenCode",
            SkillAgent::Agents => "Agents",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            SkillAgent::Claude => "Cl",
            SkillAgent::Codex => "Cx",
            SkillAgent::Cursor => "Cu",
            SkillAgent::Grok => "Gk",
            SkillAgent::OpenCode => "Oc",
            SkillAgent::Agents => "Ag",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(SkillAgent::Claude),
            "codex" => Some(SkillAgent::Codex),
            "cursor" => Some(SkillAgent::Cursor),
            "grok" => Some(SkillAgent::Grok),
            "opencode" | "open-code" => Some(SkillAgent::OpenCode),
            "agents" | "agent" => Some(SkillAgent::Agents),
            _ => None,
        }
    }
}

/// skillshare config root (`~/.config/skillshare` on macOS/Linux).
pub fn skillshare_config_dir(home: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("SKILLSHARE_CONFIG_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let xdg = xdg.trim();
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("skillshare");
        }
    }
    home.join(".config/skillshare")
}

/// Central skill library managed by skillshare.
pub fn skillshare_store_dir(home: &Path) -> PathBuf {
    skillshare_config_dir(home).join("skills")
}

pub fn skillshare_agents_dir(home: &Path) -> PathBuf {
    skillshare_config_dir(home).join("agents")
}

/// Primary global skill directory for an agent.
pub fn agent_skills_dir(home: &Path, agent: SkillAgent) -> PathBuf {
    match agent {
        SkillAgent::Claude => home.join(".claude/skills"),
        SkillAgent::Codex => home.join(".codex/skills"),
        SkillAgent::Cursor => home.join(".cursor/skills"),
        SkillAgent::Grok => home.join(".grok/skills"),
        SkillAgent::OpenCode => home.join(".config/opencode/skill"),
        SkillAgent::Agents => home.join(".agents/skills"),
    }
}

/// Extra read-only locations (e.g. bundled Grok skills). Not written by sync.
pub fn agent_extra_skills_dirs(home: &Path, agent: SkillAgent) -> Vec<PathBuf> {
    match agent {
        SkillAgent::Grok => vec![home.join(".grok/bundled/skills")],
        SkillAgent::OpenCode => vec![
            home.join(".config/opencode/skills"),
            home.join(".opencode/skill"),
            home.join(".opencode/skills"),
        ],
        SkillAgent::Cursor => vec![home.join(".cursor/skills-cursor")],
        _ => Vec::new(),
    }
}

/// All directories to scan for presence of a skill for an agent.
pub fn agent_scan_dirs(home: &Path, agent: SkillAgent) -> Vec<PathBuf> {
    let mut dirs = vec![agent_skills_dir(home, agent)];
    dirs.extend(agent_extra_skills_dirs(home, agent));
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn skillshare_store_under_config() {
        let home = PathBuf::from("/tmp/home-test");
        assert_eq!(
            skillshare_store_dir(&home),
            PathBuf::from("/tmp/home-test/.config/skillshare/skills")
        );
    }

    #[test]
    fn agent_ids_round_trip() {
        for agent in SkillAgent::ALL {
            assert_eq!(SkillAgent::from_id(agent.id()), Some(agent));
        }
    }
}
