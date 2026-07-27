//! Drift detection and enablement → agent config sync.

use super::adapters::{all_adapters, AgentMcpAdapter};
use super::enablement;
use super::inventory::{self, desired_entry};
use super::types::{
    DriftItem, DriftKind, EnablementMatrix, McpServerView, ServerSource, SyncAction, SyncChange,
    SyncReport,
};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Detect config drift for all present adapters.
pub fn detect_drift(home: &Path) -> Result<Vec<DriftItem>> {
    let inventory = inventory::list_inventory(home)?;
    let matrix = enablement::load(home)?;
    detect_drift_with(home, &inventory, &matrix, &all_adapters())
}

pub fn detect_drift_with(
    home: &Path,
    inventory: &[McpServerView],
    matrix: &EnablementMatrix,
    adapters: &[&dyn AgentMcpAdapter],
) -> Result<Vec<DriftItem>> {
    let mut drift = Vec::new();

    for adapter in adapters {
        if !adapter.present(home) {
            continue;
        }
        let agent_id = adapter.agent_id();
        let entries = adapter.read(home).unwrap_or_default();
        let by_key: BTreeMap<&str, _> = entries.iter().map(|e| (e.key.as_str(), e)).collect();

        for server in inventory {
            let present_in_agent = by_key.contains_key(server.key.as_str());
            // Default: enabled if already present (import-friendly), else false.
            let enabled = matrix.enabled_or(&server.key, agent_id, present_in_agent);

            match (enabled, by_key.get(server.key.as_str())) {
                (true, None) => {
                    drift.push(DriftItem {
                        agent_id: agent_id.into(),
                        server_key: server.key.clone(),
                        kind: DriftKind::Missing,
                        detail: format!("enabled for {agent_id} but missing from config"),
                    });
                }
                (false, Some(_)) => {
                    // Only flag if this key is in our inventory (managed).
                    drift.push(DriftItem {
                        agent_id: agent_id.into(),
                        server_key: server.key.clone(),
                        kind: DriftKind::DisabledButPresent,
                        detail: format!(
                            "disabled in matrix but still present in {agent_id} config"
                        ),
                    });
                }
                (true, Some(entry)) => {
                    if let ServerSource::ObotGateway { connect_url, .. } = &server.source {
                        if let Some(url) = &entry.url {
                            if url.trim_end_matches('/') != connect_url.trim_end_matches('/') {
                                drift.push(DriftItem {
                                    agent_id: agent_id.into(),
                                    server_key: server.key.clone(),
                                    kind: DriftKind::UrlMismatch,
                                    detail: "url ≠ Obot connectURL (agent has different URL)"
                                        .into(),
                                });
                            }
                        } else if entry.is_stdio() {
                            drift.push(DriftItem {
                                agent_id: agent_id.into(),
                                server_key: server.key.clone(),
                                kind: DriftKind::ShapeMismatch,
                                detail: "expected gateway URL but agent has stdio entry".into(),
                            });
                        } else {
                            drift.push(DriftItem {
                                agent_id: agent_id.into(),
                                server_key: server.key.clone(),
                                kind: DriftKind::Missing,
                                detail: "enabled but no url/command in agent config".into(),
                            });
                        }
                    }
                    // LocalOnly: do not flag command differences.
                }
                (false, None) => {}
            }
        }
    }

    drift.sort_by(|a, b| {
        (&a.agent_id, &a.server_key, format!("{:?}", a.kind)).cmp(&(
            &b.agent_id,
            &b.server_key,
            format!("{:?}", b.kind),
        ))
    });
    Ok(drift)
}

/// Apply enablement matrix to all present agent configs.
pub fn apply_sync(home: &Path, dry_run: bool) -> Result<SyncReport> {
    let inventory = inventory::list_inventory(home)?;
    let matrix = enablement::load(home)?;
    apply_sync_with(home, &inventory, &matrix, &all_adapters(), dry_run)
}

pub fn apply_sync_with(
    home: &Path,
    inventory: &[McpServerView],
    matrix: &EnablementMatrix,
    adapters: &[&dyn AgentMcpAdapter],
    dry_run: bool,
) -> Result<SyncReport> {
    let mut report = SyncReport {
        dry_run,
        changes: Vec::new(),
        errors: Vec::new(),
    };

    for adapter in adapters {
        if !adapter.present(home) {
            continue;
        }
        let agent_id = adapter.agent_id();
        let current = match adapter.read(home) {
            Ok(e) => e,
            Err(err) => {
                report
                    .errors
                    .push(format!("{agent_id}: read failed: {err}"));
                continue;
            }
        };
        let by_key: BTreeMap<&str, _> = current.iter().map(|e| (e.key.as_str(), e)).collect();

        let mut desired: Vec<_> = Vec::new();
        for server in inventory {
            let present_in_agent = by_key.contains_key(server.key.as_str());
            let enabled = matrix.enabled_or(&server.key, agent_id, present_in_agent);
            let entry = desired_entry(server, enabled);

            let action = plan_action(server, enabled, by_key.get(server.key.as_str()).copied());
            match action {
                SyncAction::Skip => {
                    report.changes.push(SyncChange {
                        agent_id: agent_id.into(),
                        server_key: server.key.clone(),
                        action: SyncAction::Skip,
                        detail: "already in sync".into(),
                    });
                }
                SyncAction::Upsert => {
                    report.changes.push(SyncChange {
                        agent_id: agent_id.into(),
                        server_key: server.key.clone(),
                        action: SyncAction::Upsert,
                        detail: match &server.source {
                            ServerSource::ObotGateway { connect_url, .. } => {
                                format!("upsert url={connect_url}")
                            }
                            ServerSource::LocalOnly { command, .. } => {
                                format!("upsert stdio command={command}")
                            }
                        },
                    });
                    desired.push(entry);
                }
                SyncAction::Remove => {
                    report.changes.push(SyncChange {
                        agent_id: agent_id.into(),
                        server_key: server.key.clone(),
                        action: SyncAction::Remove,
                        detail: "remove managed entry".into(),
                    });
                    desired.push(entry);
                }
            }
        }

        // Only call write when there is something to upsert/remove.
        let needs_write = desired.iter().any(|e| {
            report.changes.iter().any(|c| {
                c.agent_id == agent_id
                    && c.server_key == e.key
                    && matches!(c.action, SyncAction::Upsert | SyncAction::Remove)
            })
        });

        if needs_write && !dry_run {
            if let Err(err) = adapter.write_merge(home, &desired) {
                report
                    .errors
                    .push(format!("{agent_id}: write_merge failed: {err}"));
            }
        }
    }

    // Drop pure skip noise from change_count perspective — keep them for dry-run detail.
    Ok(report)
}

fn plan_action(
    server: &McpServerView,
    enabled: bool,
    current: Option<&super::types::AgentMcpEntry>,
) -> SyncAction {
    match (enabled, current) {
        (false, None) => SyncAction::Skip,
        (false, Some(_)) => SyncAction::Remove,
        (true, None) => SyncAction::Upsert,
        (true, Some(entry)) => match &server.source {
            ServerSource::ObotGateway { connect_url, .. } => {
                let matches = entry
                    .url
                    .as_ref()
                    .map(|u| u.trim_end_matches('/') == connect_url.trim_end_matches('/'))
                    .unwrap_or(false)
                    && entry.enabled;
                if matches {
                    SyncAction::Skip
                } else {
                    SyncAction::Upsert
                }
            }
            ServerSource::LocalOnly { .. } => {
                // Present + enabled stdio is enough; otherwise upsert (enable flip or shape fix).
                if entry.is_stdio() && entry.enabled {
                    SyncAction::Skip
                } else {
                    SyncAction::Upsert
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::adapters::grok::GrokMcpAdapter;
    use crate::mcp::types::ServerSource;
    use std::fs;
    use tempfile::TempDir;

    fn fixture_home() -> TempDir {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        fs::create_dir_all(crate::paths::config_dir(home)).unwrap();
        fs::write(crate::paths::obot_config_path(home), "enabled = false\n").unwrap();
        fs::create_dir_all(home.join(".grok")).unwrap();
        fs::write(
            home.join(".grok/config.toml"),
            r#"
[mcp_servers.gsc]
command = "/bin/gsc"
enabled = true

[mcp_servers.stripe]
url = "https://old.example/stripe"
enabled = true
"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn drift_url_mismatch_and_missing() {
        let dir = fixture_home();
        let home = dir.path();
        let inventory = vec![
            McpServerView {
                key: "stripe".into(),
                display_name: "Stripe".into(),
                source: ServerSource::ObotGateway {
                    obot_id: "ms_1".into(),
                    connect_url: "http://127.0.0.1:8080/mcp-connect/ms_1".into(),
                },
                oauth_ok: None,
                running: None,
            },
            McpServerView {
                key: "gmail".into(),
                display_name: "Gmail".into(),
                source: ServerSource::ObotGateway {
                    obot_id: "ms_2".into(),
                    connect_url: "http://127.0.0.1:8080/mcp-connect/ms_2".into(),
                },
                oauth_ok: None,
                running: None,
            },
        ];
        let mut matrix = EnablementMatrix::new();
        matrix.set("stripe", "grok", true);
        matrix.set("gmail", "grok", true);

        let adapters: Vec<&dyn AgentMcpAdapter> = vec![&GrokMcpAdapter];
        let drift = detect_drift_with(home, &inventory, &matrix, &adapters).unwrap();
        assert!(drift.iter().any(|d| d.kind == DriftKind::UrlMismatch
            && d.server_key == "stripe"
            && d.agent_id == "grok"));
        assert!(drift.iter().any(|d| d.kind == DriftKind::Missing
            && d.server_key == "gmail"
            && d.agent_id == "grok"));
    }

    #[test]
    fn sync_updates_url_and_preserves_unmanaged() {
        let dir = fixture_home();
        let home = dir.path();
        let inventory = vec![McpServerView {
            key: "stripe".into(),
            display_name: "Stripe".into(),
            source: ServerSource::ObotGateway {
                obot_id: "ms_1".into(),
                connect_url: "http://127.0.0.1:8080/mcp-connect/ms_1".into(),
            },
            oauth_ok: None,
            running: None,
        }];
        let mut matrix = EnablementMatrix::new();
        matrix.set("stripe", "grok", true);

        let adapters: Vec<&dyn AgentMcpAdapter> = vec![&GrokMcpAdapter];
        let report = apply_sync_with(home, &inventory, &matrix, &adapters, false).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.change_count() >= 1);

        let text = fs::read_to_string(home.join(".grok/config.toml")).unwrap();
        assert!(text.contains("127.0.0.1:8080/mcp-connect/ms_1"));
        assert!(text.contains("gsc"), "unmanaged local entry must remain");
        assert!(!text.contains("old.example"));
    }

    #[test]
    fn dry_run_does_not_write() {
        let dir = fixture_home();
        let home = dir.path();
        let inventory = vec![McpServerView {
            key: "stripe".into(),
            display_name: "Stripe".into(),
            source: ServerSource::ObotGateway {
                obot_id: "ms_1".into(),
                connect_url: "http://127.0.0.1:8080/mcp-connect/ms_1".into(),
            },
            oauth_ok: None,
            running: None,
        }];
        let mut matrix = EnablementMatrix::new();
        matrix.set("stripe", "grok", true);
        let adapters: Vec<&dyn AgentMcpAdapter> = vec![&GrokMcpAdapter];
        let report = apply_sync_with(home, &inventory, &matrix, &adapters, true).unwrap();
        assert!(report.dry_run);
        assert!(report.change_count() >= 1);
        let text = fs::read_to_string(home.join(".grok/config.toml")).unwrap();
        assert!(text.contains("old.example"));
    }

    #[test]
    fn disable_removes_managed_key() {
        let dir = fixture_home();
        let home = dir.path();
        let inventory = vec![McpServerView {
            key: "stripe".into(),
            display_name: "Stripe".into(),
            source: ServerSource::ObotGateway {
                obot_id: "ms_1".into(),
                connect_url: "http://127.0.0.1:8080/mcp-connect/ms_1".into(),
            },
            oauth_ok: None,
            running: None,
        }];
        let mut matrix = EnablementMatrix::new();
        matrix.set("stripe", "grok", false);
        let adapters: Vec<&dyn AgentMcpAdapter> = vec![&GrokMcpAdapter];
        apply_sync_with(home, &inventory, &matrix, &adapters, false).unwrap();
        let entries = GrokMcpAdapter.read(home).unwrap();
        assert!(!entries.iter().any(|e| e.key == "stripe"));
        assert!(entries.iter().any(|e| e.key == "gsc"));
    }
}
