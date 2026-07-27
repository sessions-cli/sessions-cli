//! Grok MCP adapter: `~/.grok/config.toml` `[mcp_servers.name]` sections.

use super::{
    command_on_path, read_toml_mcp_servers, write_merge_toml_mcp_servers, AgentMcpAdapter,
};
use crate::mcp::types::AgentMcpEntry;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct GrokMcpAdapter;

impl AgentMcpAdapter for GrokMcpAdapter {
    fn agent_id(&self) -> &'static str {
        "grok"
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        grok_home(home).join("config.toml")
    }

    fn present(&self, home: &Path) -> bool {
        grok_home(home).is_dir() || command_on_path("grok")
    }

    fn read(&self, home: &Path) -> Result<Vec<AgentMcpEntry>> {
        read_toml_mcp_servers(&self.config_path(home))
    }

    fn write_merge(&self, home: &Path, desired: &[AgentMcpEntry]) -> Result<()> {
        write_merge_toml_mcp_servers(&self.config_path(home), desired)
    }
}

fn grok_home(home: &Path) -> PathBuf {
    std::env::var_os("GROK_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".grok"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn read_write_merge() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".grok")).unwrap();
        fs::write(
            home.join(".grok/config.toml"),
            r#"
[models]
default = "grok"

[mcp_servers.gsc]
command = "/bin/gsc"
enabled = true
"#,
        )
        .unwrap();

        let adapter = GrokMcpAdapter;
        assert!(adapter.present(home));
        let entries = adapter.read(home).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "gsc");

        adapter
            .write_merge(
                home,
                &[
                    AgentMcpEntry::http("stripe", "http://127.0.0.1:8080/mcp-connect/ms"),
                    AgentMcpEntry::disabled("missing"),
                ],
            )
            .unwrap();

        let text = fs::read_to_string(home.join(".grok/config.toml")).unwrap();
        assert!(text.contains("[models]"));
        assert!(text.contains("gsc"));
        assert!(text.contains("stripe"));
        assert!(text.contains("mcp-connect"));

        let entries = adapter.read(home).unwrap();
        assert!(entries.iter().any(|e| e.key == "stripe" && e.is_http()));
        assert!(entries.iter().any(|e| e.key == "gsc" && e.is_stdio()));
    }
}
