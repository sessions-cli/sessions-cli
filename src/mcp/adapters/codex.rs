//! Codex MCP adapter: `~/.codex/config.toml` `[mcp_servers.name]` sections.

use super::{
    command_on_path, read_toml_mcp_servers, write_merge_toml_mcp_servers, AgentMcpAdapter,
};
use crate::mcp::types::AgentMcpEntry;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct CodexMcpAdapter;

impl AgentMcpAdapter for CodexMcpAdapter {
    fn agent_id(&self) -> &'static str {
        "codex"
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        codex_home(home).join("config.toml")
    }

    fn present(&self, home: &Path) -> bool {
        codex_home(home).is_dir() || command_on_path("codex")
    }

    fn read(&self, home: &Path) -> Result<Vec<AgentMcpEntry>> {
        read_toml_mcp_servers(&self.config_path(home))
    }

    fn write_merge(&self, home: &Path, desired: &[AgentMcpEntry]) -> Result<()> {
        write_merge_toml_mcp_servers(&self.config_path(home), desired)
    }
}

fn codex_home(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
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
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex/config.toml"),
            r#"
[mcp]
remote_mcp_client_enabled = true

[mcp_servers.stripe]
url = "https://mcp.stripe.com"
"#,
        )
        .unwrap();

        let adapter = CodexMcpAdapter;
        assert!(adapter.present(home));
        adapter
            .write_merge(
                home,
                &[AgentMcpEntry::http(
                    "stripe",
                    "http://127.0.0.1:8080/mcp-connect/ms_stripe",
                )],
            )
            .unwrap();
        let text = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
        assert!(text.contains("remote_mcp_client_enabled"));
        assert!(text.contains("127.0.0.1:8080"));
    }
}
