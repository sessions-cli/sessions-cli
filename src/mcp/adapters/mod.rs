//! Agent MCP config adapters (read / merge-write native formats).

pub mod claude;
pub mod codex;
pub mod grok;
pub mod opencode;

use super::types::AgentMcpEntry;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Read and merge-write MCP entries in an agent’s native config.
pub trait AgentMcpAdapter: Send + Sync {
    fn agent_id(&self) -> &'static str;
    fn config_path(&self, home: &Path) -> PathBuf;
    /// True when the agent appears installed (config dir / binary).
    fn present(&self, home: &Path) -> bool;
    fn read(&self, home: &Path) -> Result<Vec<AgentMcpEntry>>;
    /// Upsert enabled entries and remove disabled managed keys.
    ///
    /// **Must not** delete MCP entries whose keys are not in `desired`.
    /// Atomic write (temp + rename). Preserves unrelated config sections.
    fn write_merge(&self, home: &Path, desired: &[AgentMcpEntry]) -> Result<()>;
}

/// All known adapters in stable order (matches `AGENT_IDS`).
pub fn all_adapters() -> Vec<&'static dyn AgentMcpAdapter> {
    vec![
        &grok::GrokMcpAdapter,
        &codex::CodexMcpAdapter,
        &claude::ClaudeMcpAdapter,
        &opencode::OpenCodeMcpAdapter,
    ]
}

pub fn adapter_by_id(id: &str) -> Option<&'static dyn AgentMcpAdapter> {
    let id = id.trim().to_ascii_lowercase();
    all_adapters().into_iter().find(|a| a.agent_id() == id)
}

// ── Shared TOML helpers (Grok / Codex) ──────────────────────────────────────

pub(crate) fn command_on_path(name: &str) -> bool {
    crate::hooks::command_on_path(name)
}

pub(crate) fn read_toml_mcp_servers(path: &Path) -> Result<Vec<AgentMcpEntry>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)?;
    parse_toml_mcp_servers(&text)
}

pub(crate) fn parse_toml_mcp_servers(text: &str) -> Result<Vec<AgentMcpEntry>> {
    let value: toml::Value = toml::from_str(text)?;
    let mut out = Vec::new();
    let Some(servers) = value.get("mcp_servers").and_then(|v| v.as_table()) else {
        return Ok(out);
    };
    for (key, entry_val) in servers {
        let Some(table) = entry_val.as_table() else {
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
        let enabled = table
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let mut env = std::collections::BTreeMap::new();
        if let Some(env_table) = table.get("env").and_then(|v| v.as_table()) {
            for (ek, ev) in env_table {
                if let Some(s) = ev.as_str() {
                    env.insert(ek.clone(), s.to_string());
                }
            }
        }
        out.push(AgentMcpEntry {
            key: key.clone(),
            url,
            command,
            args,
            env,
            enabled,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

pub(crate) fn write_merge_toml_mcp_servers(path: &Path, desired: &[AgentMcpEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = if path.is_file() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    let mut doc = if existing.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        existing
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| anyhow::anyhow!("parse TOML {}: {e}", path.display()))?
    };

    // Ensure mcp_servers table exists.
    if doc.get("mcp_servers").is_none() {
        doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let servers = doc["mcp_servers"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("mcp_servers is not a table in {}", path.display()))?;

    for entry in desired {
        if !entry.enabled {
            servers.remove(&entry.key);
            continue;
        }

        if servers.get(&entry.key).and_then(|i| i.as_table()).is_none() {
            let mut t = toml_edit::Table::new();
            t.set_implicit(false);
            servers.insert(&entry.key, toml_edit::Item::Table(t));
        }
        let table = servers
            .get_mut(&entry.key)
            .and_then(|i| i.as_table_mut())
            .ok_or_else(|| anyhow::anyhow!("mcp_servers.{} is not a table", entry.key))?;

        if let Some(url) = &entry.url {
            table.insert("url", toml_edit::value(url.as_str()));
            // Prefer URL transport: drop command/args if we are writing a gateway URL.
            table.remove("command");
            table.remove("args");
        } else if let Some(command) = &entry.command {
            // LocalOnly: set command/args only when missing (preserve hand-tuned values).
            if table.get("command").is_none() {
                table.insert("command", toml_edit::value(command.as_str()));
            }
            if !entry.args.is_empty() && table.get("args").is_none() {
                let mut arr = toml_edit::Array::new();
                for a in &entry.args {
                    arr.push(a.as_str());
                }
                table.insert("args", toml_edit::value(arr));
            }
            // Do not force-remove url if user had both; prefer leaving unknown keys.
        }

        table.insert("enabled", toml_edit::value(entry.enabled));

        if !entry.env.is_empty() {
            if table.get("env").and_then(|i| i.as_table()).is_none() {
                table.insert("env", toml_edit::Item::Table(toml_edit::Table::new()));
            }
            if let Some(env_table) = table.get_mut("env").and_then(|i| i.as_table_mut()) {
                for (k, v) in &entry.env {
                    if env_table.get(k).is_none() {
                        env_table.insert(k, toml_edit::value(v.as_str()));
                    }
                }
            }
        }
    }

    // Drop empty mcp_servers table.
    if let Some(t) = doc.get("mcp_servers").and_then(|i| i.as_table()) {
        if t.is_empty() {
            doc.as_table_mut().remove("mcp_servers");
        }
    }

    let rendered = doc.to_string();
    crate::mcp::atomic_write(path, rendered.as_bytes())
}

// ── Shared JSON helpers (Claude / OpenCode) ─────────────────────────────────

pub(crate) fn atomic_write_json_pretty(path: &Path, value: &serde_json::Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    let mut bytes = text.into_bytes();
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    crate::mcp::atomic_write(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn toml_round_trip_preserves_unrelated() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[ui]
theme = "dark"

[mcp_servers.keep_me]
url = "https://example.com/keep"
enabled = true

[mcp_servers.stripe]
url = "https://old.example/stripe"
enabled = true
"#,
        )
        .unwrap();

        let desired = vec![
            AgentMcpEntry::http("stripe", "http://127.0.0.1:8080/mcp-connect/ms_stripe"),
            AgentMcpEntry::disabled("gone"),
        ];
        write_merge_toml_mcp_servers(&path, &desired).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("theme"));
        assert!(text.contains("keep_me"));
        assert!(text.contains("127.0.0.1:8080"));
        assert!(!text.contains("old.example"));

        // "gone" was never present — still fine; keep_me untouched.
        let entries = read_toml_mcp_servers(&path).unwrap();
        let keys: Vec<_> = entries.iter().map(|e| e.key.as_str()).collect();
        assert!(keys.contains(&"keep_me"));
        assert!(keys.contains(&"stripe"));
        assert!(!keys.contains(&"gone"));
    }

    #[test]
    fn toml_remove_managed_key() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.stripe]
url = "http://x"
enabled = true

[mcp_servers.other]
command = "/bin/tool"
"#,
        )
        .unwrap();
        write_merge_toml_mcp_servers(&path, &[AgentMcpEntry::disabled("stripe")]).unwrap();
        let entries = read_toml_mcp_servers(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "other");
    }
}
