//! Merge Obot gateway servers with LocalOnly discoveries from agent configs.

use super::adapters::{all_adapters, AgentMcpAdapter};
use super::local;
use super::obot;
use super::types::{McpServerView, ServerSource};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Full inventory: Obot servers first (by key), then LocalOnly keys not claimed by Obot.
pub fn list_inventory(home: &Path) -> Result<Vec<McpServerView>> {
    list_inventory_with(home, &all_adapters(), true)
}

/// Like [`list_inventory`] but allows injecting adapters and skipping Obot HTTP.
pub fn list_inventory_with(
    home: &Path,
    adapters: &[&dyn AgentMcpAdapter],
    fetch_obot: bool,
) -> Result<Vec<McpServerView>> {
    let mut by_key: BTreeMap<String, McpServerView> = BTreeMap::new();

    if fetch_obot {
        match obot::list_servers(home) {
            Ok(servers) => {
                for s in servers {
                    by_key.insert(s.key.clone(), s);
                }
            }
            Err(err) => {
                // Soft-fail: still return local inventory; caller can check health separately.
                tracing::warn!("Obot list servers failed: {err}");
            }
        }
    }

    let local_only = local::discover_local_only_with(home, adapters)?;
    for s in local_only {
        // Obot gateway wins on key collision.
        by_key.entry(s.key.clone()).or_insert(s);
    }

    Ok(by_key.into_values().collect())
}

/// Keys sessions considers managed for a given inventory snapshot.
pub fn managed_keys(inventory: &[McpServerView]) -> Vec<String> {
    inventory.iter().map(|s| s.key.clone()).collect()
}

/// Build a desired `AgentMcpEntry` for syncing one inventory server onto an agent.
pub fn desired_entry(server: &McpServerView, enabled: bool) -> super::types::AgentMcpEntry {
    use super::types::AgentMcpEntry;
    if !enabled {
        return AgentMcpEntry::disabled(&server.key);
    }
    match &server.source {
        ServerSource::ObotGateway { connect_url, .. } => {
            AgentMcpEntry::http(&server.key, connect_url)
        }
        ServerSource::LocalOnly { command, args } => {
            AgentMcpEntry::stdio(&server.key, command, args.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::adapters::grok::GrokMcpAdapter;
    use crate::mcp::types::ServerSource;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn merges_local_only_without_obot() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".grok")).unwrap();
        // Disable Obot so we do not hit the network.
        fs::create_dir_all(crate::paths::config_dir(home)).unwrap();
        fs::write(
            crate::paths::obot_config_path(home),
            "enabled = false\nbase_url = \"http://127.0.0.1:1\"\n",
        )
        .unwrap();
        fs::write(
            home.join(".grok/config.toml"),
            r#"
[mcp_servers.gsc]
command = "/bin/gsc"
enabled = true
"#,
        )
        .unwrap();

        let adapters: Vec<&dyn AgentMcpAdapter> = vec![&GrokMcpAdapter];
        let inv = list_inventory_with(home, &adapters, true).unwrap();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].key, "gsc");
        assert!(matches!(inv[0].source, ServerSource::LocalOnly { .. }));
    }

    #[test]
    fn obot_key_wins_over_local() {
        let mut by_key: BTreeMap<String, McpServerView> = BTreeMap::new();
        by_key.insert(
            "stripe".into(),
            McpServerView {
                key: "stripe".into(),
                display_name: "Stripe".into(),
                source: ServerSource::ObotGateway {
                    obot_id: "ms_1".into(),
                    connect_url: "http://obot/mcp-connect/ms_1".into(),
                },
                oauth_ok: None,
                running: None,
            },
        );
        let local = McpServerView {
            key: "stripe".into(),
            display_name: "stripe".into(),
            source: ServerSource::LocalOnly {
                command: "/bin/stripe".into(),
                args: vec![],
            },
            oauth_ok: None,
            running: None,
        };
        by_key.entry(local.key.clone()).or_insert(local);
        assert!(matches!(
            by_key.get("stripe").unwrap().source,
            ServerSource::ObotGateway { .. }
        ));
    }
}
