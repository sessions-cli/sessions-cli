//! Load/save `mcp-enablement.toml` under sessions config dir.

use super::types::EnablementMatrix;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// Load enablement matrix. Missing file → empty matrix.
pub fn load(home: &Path) -> Result<EnablementMatrix> {
    let path = crate::paths::mcp_enablement_path(home);
    if !path.is_file() {
        return Ok(EnablementMatrix::new());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read enablement {}", path.display()))?;
    parse_toml(&text).with_context(|| format!("parse enablement {}", path.display()))
}

/// Persist enablement matrix atomically.
pub fn save(home: &Path, matrix: &EnablementMatrix) -> Result<()> {
    let path = crate::paths::mcp_enablement_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }
    let text = to_toml(matrix);
    crate::mcp::atomic_write(&path, text.as_bytes())
}

#[derive(Debug, serde::Deserialize, serde::Serialize, Default)]
struct FileShape {
    #[serde(default)]
    servers: BTreeMap<String, BTreeMap<String, bool>>,
    /// Reserved for local overrides (ignored for matrix map; kept for forward compat).
    #[serde(default)]
    local: BTreeMap<String, toml::Value>,
}

fn parse_toml(text: &str) -> Result<EnablementMatrix> {
    let shape: FileShape = toml::from_str(text)?;
    Ok(EnablementMatrix { map: shape.servers })
}

fn to_toml(matrix: &EnablementMatrix) -> String {
    let shape = FileShape {
        servers: matrix.map.clone(),
        local: BTreeMap::new(),
    };
    // Prefer hand-written sections for stable, readable order.
    let mut out = String::from(
        "# sessions-cli MCP enablement matrix\n# [servers.<key>]\n# agent_id = true|false\n\n",
    );
    for (server, agents) in &shape.servers {
        out.push_str(&format!("[servers.{server}]\n"));
        for (agent, enabled) in agents {
            out.push_str(&format!("{agent} = {enabled}\n"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_missing_is_empty() {
        let dir = TempDir::new().unwrap();
        let m = load(dir.path()).unwrap();
        assert!(m.map.is_empty());
    }

    #[test]
    fn round_trip() {
        let dir = TempDir::new().unwrap();
        let mut m = EnablementMatrix::new();
        m.set("stripe", "grok", true);
        m.set("stripe", "claude", false);
        m.set("gsc", "grok", true);
        save(dir.path(), &m).unwrap();

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.is_enabled("stripe", "grok"), Some(true));
        assert_eq!(loaded.is_enabled("stripe", "claude"), Some(false));
        assert_eq!(loaded.is_enabled("gsc", "grok"), Some(true));
        assert_eq!(loaded.is_enabled("missing", "grok"), None);
    }

    #[test]
    fn parse_example_shape() {
        let text = r#"
[servers.stripe]
grok = true
codex = true
claude = false
opencode = false

[servers.gmail]
grok = true
"#;
        let m = parse_toml(text).unwrap();
        assert_eq!(m.is_enabled("stripe", "codex"), Some(true));
        assert_eq!(m.is_enabled("gmail", "grok"), Some(true));
    }
}
