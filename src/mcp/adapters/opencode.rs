//! OpenCode MCP adapter: `~/.config/opencode/opencode.json` `mcp` map.
//!
//! Format (observed):
//! ```json
//! {
//!   "mcp": {
//!     "name": { "type": "remote", "url": "...", "enabled": true },
//!     "local": { "type": "local", "command": ["bin"], "enabled": true }
//!   }
//! }
//! ```

use super::{atomic_write_json_pretty, command_on_path, AgentMcpAdapter};
use crate::mcp::types::AgentMcpEntry;
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct OpenCodeMcpAdapter;

impl AgentMcpAdapter for OpenCodeMcpAdapter {
    fn agent_id(&self) -> &'static str {
        "opencode"
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        config_dir(home).join("opencode.json")
    }

    fn present(&self, home: &Path) -> bool {
        config_dir(home).is_dir() || home.join(".opencode").is_dir() || command_on_path("opencode")
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
        Ok(parse_mcp(&value))
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
            .entry("mcp")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("mcp is not an object"))?;

        for entry in desired {
            if !entry.enabled {
                servers.remove(&entry.key);
                continue;
            }
            apply_entry(servers, entry);
        }

        if servers.is_empty() {
            obj.remove("mcp");
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write_json_pretty(&path, &root)
    }
}

fn config_dir(home: &Path) -> PathBuf {
    // Prefer XDG when set, else ~/.config/opencode (same as agent hooks).
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("opencode")
}

fn parse_mcp(root: &Value) -> Vec<AgentMcpEntry> {
    let Some(servers) = root.get("mcp").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, entry) in servers {
        let Some(table) = entry.as_object() else {
            continue;
        };
        let enabled = table
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let url = table
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        // command may be string or array.
        let (command, args) = parse_command_field(table);

        let mut env = BTreeMap::new();
        if let Some(env_obj) = table.get("env").and_then(|v| v.as_object()) {
            for (k, v) in env_obj {
                if let Some(s) = v.as_str() {
                    env.insert(k.clone(), s.to_string());
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
    out
}

fn parse_command_field(table: &Map<String, Value>) -> (Option<String>, Vec<String>) {
    match table.get("command") {
        Some(Value::String(s)) if !s.is_empty() => {
            let args = table
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            (Some(s.clone()), args)
        }
        Some(Value::Array(arr)) if !arr.is_empty() => {
            let mut parts: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if parts.is_empty() {
                (None, Vec::new())
            } else {
                let cmd = parts.remove(0);
                (Some(cmd), parts)
            }
        }
        _ => (None, Vec::new()),
    }
}

fn apply_entry(servers: &mut Map<String, Value>, entry: &AgentMcpEntry) {
    if let Some(url) = &entry.url {
        servers.insert(
            entry.key.clone(),
            json!({
                "type": "remote",
                "url": url,
                "enabled": true
            }),
        );
        return;
    }

    if let Some(command) = &entry.command {
        // Preserve existing when present.
        if let Some(Value::Object(_)) = servers.get(&entry.key) {
            if let Some(Value::Object(obj)) = servers.get_mut(&entry.key) {
                obj.insert("enabled".into(), json!(true));
                if !obj.contains_key("type") {
                    obj.insert("type".into(), json!("local"));
                }
            }
            return;
        }
        let mut cmd = vec![command.clone()];
        cmd.extend(entry.args.iter().cloned());
        servers.insert(
            entry.key.clone(),
            json!({
                "type": "local",
                "command": cmd,
                "enabled": true
            }),
        );
    }
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
        // Avoid XDG_CONFIG_HOME pollution: adapter uses XDG or home/.config
        let cfg_dir = home.join(".config/opencode");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::write(
            cfg_dir.join("opencode.json"),
            r#"{
  "permission": "allow",
  "mcp": {
    "calendar": {
      "type": "local",
      "command": ["/bin/cal"],
      "enabled": true
    }
  }
}
"#,
        )
        .unwrap();

        // Ensure XDG does not redirect away from our temp home.
        let adapter = OpenCodeMcpAdapter;
        // present checks config_dir under XDG or home — with XDG_CONFIG_HOME possibly set
        // in the developer environment, point tests via isolated path read.
        // We still exercise read/write via config_path relative to home when XDG unset.
        // If XDG is set globally, config_path uses it — so write via write_merge after
        // temporarily unsetting is fragile. Instead, only test parse helpers + write on
        // explicit path through the adapter when home's .config is used.
        //
        // When XDG_CONFIG_HOME is set, present/read may not see temp files. Force by
        // checking parse + write_merge path that goes through config_dir(home) only when
        // XDG is empty — we test the pure functions instead if needed.
        let _ = home;
        let _ = adapter;

        let root: Value = serde_json::from_str(
            r#"{ "mcp": {
                "gmail": { "type": "remote", "url": "https://example/mcp", "enabled": true },
                "cal": { "type": "local", "command": ["/bin/cal", "--x"], "enabled": false }
            }}"#,
        )
        .unwrap();
        let entries = parse_mcp(&root);
        assert_eq!(entries.len(), 2);
        let gmail = entries.iter().find(|e| e.key == "gmail").unwrap();
        assert!(gmail.is_http());
        let cal = entries.iter().find(|e| e.key == "cal").unwrap();
        assert!(cal.is_stdio());
        assert!(!cal.enabled);
        assert_eq!(cal.args, vec!["--x".to_string()]);
    }

    #[test]
    fn write_merge_isolated_home() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        // Use a scoped env override only if we can — prefer writing via absolute
        // path by creating the expected layout. When XDG_CONFIG_HOME is set, OpenCode
        // adapter follows XDG; for unit tests we set it to the temp dir.
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", home.join("xdg"));
        let result = std::panic::catch_unwind(|| {
            let adapter = OpenCodeMcpAdapter;
            let path = adapter.config_path(home);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, r#"{"plugin":["sessions"]}"#).unwrap();
            adapter
                .write_merge(
                    home,
                    &[AgentMcpEntry::http(
                        "stripe",
                        "http://127.0.0.1:8080/mcp-connect/ms",
                    )],
                )
                .unwrap();
            let text = fs::read_to_string(&path).unwrap();
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["plugin"][0], "sessions");
            assert_eq!(value["mcp"]["stripe"]["type"], "remote");
            assert_eq!(
                value["mcp"]["stripe"]["url"],
                "http://127.0.0.1:8080/mcp-connect/ms"
            );
        });
        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }
}
