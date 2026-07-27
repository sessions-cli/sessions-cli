//! LocalOnly (stdio) inventory helpers.

use super::adapters::{all_adapters, AgentMcpAdapter};
use super::types::{McpServerView, ServerSource};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Discover LocalOnly servers from agent configs (stdio entries without a URL).
///
/// Merges by key across agents; first-seen command/args wins.
pub fn discover_local_only(home: &Path) -> Result<Vec<McpServerView>> {
    discover_local_only_with(home, &all_adapters())
}

pub fn discover_local_only_with(
    home: &Path,
    adapters: &[&dyn AgentMcpAdapter],
) -> Result<Vec<McpServerView>> {
    let mut by_key: BTreeMap<String, McpServerView> = BTreeMap::new();

    for adapter in adapters {
        if !adapter.present(home) {
            continue;
        }
        let entries = match adapter.read(home) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries {
            if !entry.is_stdio() || entry.is_http() {
                continue;
            }
            let Some(command) = entry.command.clone() else {
                continue;
            };
            by_key
                .entry(entry.key.clone())
                .or_insert_with(|| McpServerView {
                    key: entry.key.clone(),
                    display_name: entry.key.clone(),
                    source: ServerSource::LocalOnly {
                        command,
                        args: entry.args.clone(),
                    },
                    oauth_ok: None,
                    running: None,
                });
        }
    }

    Ok(by_key.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::adapters::grok::GrokMcpAdapter;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discovers_stdio_not_http() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let grok = home.join(".grok");
        fs::create_dir_all(&grok).unwrap();
        fs::write(
            grok.join("config.toml"),
            r#"
[mcp_servers.gsc]
command = "/usr/local/bin/mcp-gsc"
args = ["--verbose"]
enabled = true

[mcp_servers.stripe]
url = "https://mcp.stripe.com"
enabled = true
"#,
        )
        .unwrap();

        let adapters: Vec<&dyn AgentMcpAdapter> = vec![&GrokMcpAdapter];
        let local = discover_local_only_with(home, &adapters).unwrap();
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].key, "gsc");
        match &local[0].source {
            ServerSource::LocalOnly { command, args } => {
                assert!(command.contains("mcp-gsc"));
                assert_eq!(args, &vec!["--verbose".to_string()]);
            }
            _ => panic!("expected LocalOnly"),
        }
    }
}
