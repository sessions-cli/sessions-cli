//! Claude MCP adapter: user-scope `~/.claude.json` `mcpServers` map.

use super::{atomic_write_json_pretty, command_on_path, AgentMcpAdapter};
use crate::mcp::types::AgentMcpEntry;
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct ClaudeMcpAdapter;

impl AgentMcpAdapter for ClaudeMcpAdapter {
    fn agent_id(&self) -> &'static str {
        "claude"
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".claude.json")
    }

    fn present(&self, home: &Path) -> bool {
        self.config_path(home).is_file()
            || home.join(".claude").is_dir()
            || command_on_path("claude")
    }

    fn read(&self, home: &Path) -> Result<Vec<AgentMcpEntry>> {
        let path = self.config_path(home);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let value: Value =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        Ok(parse_mcp_servers(&value))
    }

    fn write_merge(&self, home: &Path, desired: &[AgentMcpEntry]) -> Result<()> {
        let path = self.config_path(home);
        let mut root: Value = if path.is_file() {
            let text = std::fs::read_to_string(&path)?;
            serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
        } else {
            json!({})
        };

        if !root.is_object() {
            root = json!({});
        }
        let obj = root.as_object_mut().expect("object");
        let servers = obj
            .entry("mcpServers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("mcpServers is not an object"))?;

        for entry in desired {
            if !entry.enabled {
                servers.remove(&entry.key);
                continue;
            }
            apply_entry(servers, entry);
        }

        // Drop empty mcpServers.
        if servers.is_empty() {
            obj.remove("mcpServers");
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write_json_pretty(&path, &root)
    }
}

fn parse_mcp_servers(root: &Value) -> Vec<AgentMcpEntry> {
    let Some(servers) = root.get("mcpServers").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, entry) in servers {
        let Some(table) = entry.as_object() else {
            continue;
        };
        let url = table
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        let command = table
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        let args = table
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let mut env = BTreeMap::new();
        if let Some(env_obj) = table.get("env").and_then(|v| v.as_object()) {
            for (k, v) in env_obj {
                if let Some(s) = v.as_str() {
                    env.insert(k.clone(), s.to_string());
                }
            }
        }
        // Claude has no top-level enabled flag; presence means enabled.
        out.push(AgentMcpEntry {
            key: key.clone(),
            url,
            command,
            args,
            env,
            enabled: true,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn apply_entry(servers: &mut Map<String, Value>, entry: &AgentMcpEntry) {
    if let Some(url) = &entry.url {
        // HTTP / streamable-http style.
        let mut obj = Map::new();
        obj.insert("type".into(), json!("http"));
        obj.insert("url".into(), json!(url));
        // Preserve env if we are replacing a prior stdio entry? Prefer clean HTTP shape.
        servers.insert(entry.key.clone(), Value::Object(obj));
        return;
    }

    if let Some(command) = &entry.command {
        // Preserve existing entry fields when possible.
        let mut obj = if let Some(Value::Object(existing)) = servers.get(&entry.key) {
            existing.clone()
        } else {
            Map::new()
        };
        if !obj.contains_key("command") {
            obj.insert("command".into(), json!(command));
        }
        if !entry.args.is_empty() && !obj.contains_key("args") {
            obj.insert("args".into(), json!(entry.args));
        }
        if !obj.contains_key("type") {
            obj.insert("type".into(), json!("stdio"));
        }
        if !entry.env.is_empty() && !obj.contains_key("env") {
            let env_map: Map<String, Value> = entry
                .env
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect();
            obj.insert("env".into(), Value::Object(env_map));
        }
        servers.insert(entry.key.clone(), Value::Object(obj));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn read_write_merge_preserves_other_keys() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        fs::write(
            home.join(".claude.json"),
            r#"{
  "theme": "dark",
  "mcpServers": {
    "calendar": {
      "type": "stdio",
      "command": "/bin/cal",
      "args": [],
      "env": {}
    }
  }
}
"#,
        )
        .unwrap();

        let adapter = ClaudeMcpAdapter;
        assert!(adapter.present(home));
        let entries = adapter.read(home).unwrap();
        assert_eq!(entries.len(), 1);

        adapter
            .write_merge(
                home,
                &[
                    AgentMcpEntry::http("stripe", "http://127.0.0.1:8080/mcp-connect/ms"),
                    AgentMcpEntry::disabled("calendar"),
                ],
            )
            .unwrap();

        let text = fs::read_to_string(home.join(".claude.json")).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["theme"], "dark");
        assert!(value["mcpServers"].get("calendar").is_none());
        assert_eq!(
            value["mcpServers"]["stripe"]["url"],
            "http://127.0.0.1:8080/mcp-connect/ms"
        );
        assert_eq!(value["mcpServers"]["stripe"]["type"], "http");
    }
}
