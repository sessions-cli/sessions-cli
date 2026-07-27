//! MCP domain layer: Obot client, enablement matrix, agent adapters, sync.
//!
//! UI / CLI modules call the public facade below; they do not open agent
//! configs or Obot HTTP themselves.

pub mod adapters;
pub mod enablement;
pub mod inventory;
pub mod local;
pub mod obot;
pub mod sync;
pub mod types;

#[allow(unused_imports)] // public facade for UI/CLI consumers
pub use adapters::{adapter_by_id, all_adapters, AgentMcpAdapter};
pub use types::*;

use anyhow::Result;
use std::path::Path;

/// Atomic write: temp file next to target + rename.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use anyhow::Context;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent for {}", path.display()))?;
    }
    // Unique sibling temp (pid + nanos) so parallel tests never share a tmp name.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = path.with_extension(format!("sessions-tmp-{}-{}", std::process::id(), nanos));
    std::fs::write(&tmp, bytes).with_context(|| format!("write temp {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

// ── Public facade (used by UI / CLI later) ──────────────────────────────────

/// Load Obot settings from `~/.config/sessions/obot.toml` (defaults if missing).
pub fn load_obot_config(home: &Path) -> Result<ObotConfig> {
    obot::load_config(home)
}

/// Probe Obot reachability (401/404 count as up).
pub fn health(home: &Path) -> Result<ObotHealth> {
    obot::health(home)
}

/// Merged inventory: Obot gateway servers + LocalOnly stdio from agent configs.
pub fn list_inventory(home: &Path) -> Result<Vec<McpServerView>> {
    inventory::list_inventory(home)
}

/// Load per-agent enablement matrix.
pub fn load_enablement(home: &Path) -> Result<EnablementMatrix> {
    enablement::load(home)
}

/// Persist enablement matrix.
pub fn save_enablement(home: &Path, matrix: &EnablementMatrix) -> Result<()> {
    enablement::save(home, matrix)
}

/// Apply enablement → write agent configs.
pub fn sync_all(home: &Path) -> Result<SyncReport> {
    sync::apply_sync(home, false)
}

/// Plan enablement → agent config changes without writing.
pub fn dry_run(home: &Path) -> Result<SyncReport> {
    sync::apply_sync(home, true)
}

/// Detect drift between matrix / Obot URLs and agent configs.
pub fn detect_drift(home: &Path) -> Result<Vec<DriftItem>> {
    sync::detect_drift(home)
}

/// Browser URL for Obot admin (base_url + open_admin_path).
pub fn open_admin_url(home: &Path) -> Result<String> {
    let cfg = obot::load_config(home)?;
    Ok(cfg.admin_url())
}

/// List deployable catalog entries from Obot.
pub fn list_catalog_entries(home: &Path) -> Result<Vec<CatalogEntryView>> {
    obot::list_catalog_entries(home)
}

/// Search catalog entries by name / description (client-side filter of entries list).
pub fn search_catalog(home: &Path, query: &str) -> Result<Vec<CatalogEntryView>> {
    obot::search_catalog(home, query)
}

/// Optional MCP Registry API search (`/v0.1/servers?search=`).
pub fn search_registry(home: &Path, query: &str, limit: usize) -> Result<Vec<CatalogEntryView>> {
    obot::search_registry(home, query, limit)
}

/// Deploy a catalog entry as a personal MCP server in Obot.
pub fn create_server_from_entry(
    home: &Path,
    catalog_entry_id: &str,
    alias: Option<&str>,
) -> Result<CreateServerResult> {
    obot::create_server_from_entry(home, catalog_entry_id, alias)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/file.toml");
        atomic_write(&path, b"hello = true\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello = true\n");
    }

    #[test]
    fn facade_enablement_and_admin_url() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        fs::create_dir_all(crate::paths::config_dir(home)).unwrap();
        fs::write(
            crate::paths::obot_config_path(home),
            r#"
enabled = true
base_url = "http://127.0.0.1:8080"
open_admin_path = "/mcp-catalog"
"#,
        )
        .unwrap();
        assert_eq!(
            open_admin_url(home).unwrap(),
            "http://127.0.0.1:8080/mcp-catalog"
        );

        let mut m = EnablementMatrix::new();
        m.set("stripe", "grok", true);
        save_enablement(home, &m).unwrap();
        let loaded = load_enablement(home).unwrap();
        assert_eq!(loaded.is_enabled("stripe", "grok"), Some(true));
    }
}
