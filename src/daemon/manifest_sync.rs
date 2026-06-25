use crate::config::Config;
use crate::session::manifest::{
    apply_manifest_sync_patches, update_manifest_agent_session_ids, ManifestSyncPatch,
};
use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Pending manifest fields to back-sync from hook notify events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestPatch {
    pub agent_session_id: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub messaged_at: Option<DateTime<Utc>>,
}

impl ManifestPatch {
    pub fn merge(&mut self, other: ManifestPatch) {
        if let Some(id) = other.agent_session_id {
            self.agent_session_id = Some(id);
        }
        if let Some(agent) = other.agent {
            self.agent = Some(agent);
        }
        if let Some(title) = other.title {
            self.title = Some(title);
        }
        if let Some(at) = other.messaged_at {
            self.messaged_at = Some(at);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.agent_session_id.is_none()
            && self.agent.is_none()
            && self.title.is_none()
            && self.messaged_at.is_none()
    }
}

impl From<ManifestPatch> for ManifestSyncPatch {
    fn from(patch: ManifestPatch) -> Self {
        ManifestSyncPatch {
            agent_session_id: patch.agent_session_id,
            agent: patch.agent,
            title: patch.title,
            messaged_at: patch.messaged_at,
        }
    }
}

impl From<&ManifestPatch> for ManifestSyncPatch {
    fn from(patch: &ManifestPatch) -> Self {
        ManifestSyncPatch {
            agent_session_id: patch.agent_session_id.clone(),
            agent: patch.agent.clone(),
            title: patch.title.clone(),
            messaged_at: patch.messaged_at,
        }
    }
}

#[derive(Debug, Default)]
struct QueueInner {
    patches: HashMap<String, ManifestPatch>,
    dirty: bool,
}

/// In-memory debounced queue of manifest patches keyed by `sessions_session_id`.
///
/// The [`generation`](ManifestSyncQueue::generation) counter bumps on every `enqueue` and is
/// used by `manifest_persist_loop` (500ms debounce) to detect bursts: flush commits only when
/// generation has been stable for the debounce window, and only clears the queue after a
/// successful `atomic_write_manifest` when generation is still unchanged.
#[derive(Debug)]
pub struct ManifestSyncQueue {
    inner: Mutex<QueueInner>,
    generation: AtomicU64,
}

impl Default for ManifestSyncQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Debounce gate mirrored by `manifest_persist_loop` in `server.rs`.
pub(crate) fn flush_after_debounce(
    stable_generation: u64,
    current_generation: u64,
    is_dirty: bool,
) -> bool {
    is_dirty && stable_generation == current_generation
}

impl ManifestSyncQueue {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(QueueInner::default()),
            generation: AtomicU64::new(0),
        }
    }

    pub fn enqueue(&self, ssn: String, patch: ManifestPatch) {
        if patch.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().expect("manifest sync queue lock");
        inner
            .patches
            .entry(ssn)
            .and_modify(|existing| existing.merge(patch.clone()))
            .or_insert(patch);
        inner.dirty = true;
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn is_dirty(&self) -> bool {
        self.inner.lock().expect("manifest sync queue lock").dirty
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn snapshot_pending(&self) -> HashMap<String, ManifestPatch> {
        self.inner
            .lock()
            .expect("manifest sync queue lock")
            .patches
            .clone()
    }

    fn clear_pending(&self) {
        let mut inner = self.inner.lock().expect("manifest sync queue lock");
        inner.patches.clear();
        inner.dirty = false;
    }

    fn persist_patches(config: &Config, patches: &HashMap<String, ManifestPatch>) -> Result<()> {
        if patches.is_empty() {
            return Ok(());
        }
        if patches.values().all(|patch| {
            patch.title.is_none() && patch.messaged_at.is_none() && patch.agent.is_none()
        }) {
            let updates: Vec<(String, String)> = patches
                .iter()
                .filter_map(|(ssn, patch)| {
                    patch
                        .agent_session_id
                        .as_ref()
                        .map(|id| (ssn.clone(), id.clone()))
                })
                .collect();
            return update_manifest_agent_session_ids(config, &updates);
        }
        let sync_patches: HashMap<String, ManifestSyncPatch> = patches
            .iter()
            .map(|(ssn, patch)| (ssn.clone(), ManifestSyncPatch::from(patch)))
            .collect();
        apply_manifest_sync_patches(config, &sync_patches)
    }

    /// Persist a snapshot of pending patches. The queue is cleared only after a successful
    /// write and only when no new `enqueue` calls occurred during the persist (generation stable).
    pub fn flush(&self, config: &Config) -> Result<()> {
        let stable_generation = self.generation();
        let patches = self.snapshot_pending();
        if patches.is_empty() {
            return Ok(());
        }

        Self::persist_patches(config, &patches)?;

        if self.generation() == stable_generation {
            self.clear_pending();
        }
        Ok(())
    }

    /// Explicit drain for `FlushManifest` / shutdown — keeps flushing until the queue is empty.
    pub fn flush_all(&self, config: &Config) -> Result<()> {
        const MAX_ROUNDS: usize = 64;
        for _ in 0..MAX_ROUNDS {
            if !self.is_dirty() {
                return Ok(());
            }
            self.flush(config)?;
        }
        if self.is_dirty() {
            bail!("manifest queue still dirty after {MAX_ROUNDS} flush rounds");
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn pending_snapshot(&self) -> HashMap<String, ManifestPatch> {
        self.snapshot_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::manifest::{
        append_entry, load_manifest, manifest_path, ManifestEntry, ManifestSource, MANIFEST_VERSION,
    };
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_config(home: &std::path::Path) -> Config {
        let mut config = Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.workspaces_path = home.join("workspaces.toml");
        config.tmux_session = "agents-nonexistent".into();
        config
    }

    fn sample_entry(ssn: &str) -> ManifestEntry {
        ManifestEntry {
            sessions_session_id: ssn.into(),
            source: ManifestSource::NewChat,
            workspace_index: None,
            cwd: "/tmp".into(),
            cwd_label: "/tmp".into(),
            agent: "grok".into(),
            launch_command: "grok".into(),
            agent_session_id: Some("agent-old".into()),
            title: Some("grok · old".into()),
            messaged_at: None,
            closed: false,
        }
    }

    #[test]
    fn queue_coalesces_duplicate_ssn_patches() {
        let queue = ManifestSyncQueue::new();
        queue.enqueue(
            "ssn_a".into(),
            ManifestPatch {
                agent_session_id: Some("agent-1".into()),
                agent: None,
                title: None,
                messaged_at: None,
            },
        );
        queue.enqueue(
            "ssn_a".into(),
            ManifestPatch {
                agent_session_id: Some("agent-2".into()),
                agent: None,
                title: Some("grok · merged".into()),
                messaged_at: None,
            },
        );

        let snapshot = queue.pending_snapshot();
        assert_eq!(snapshot.len(), 1);
        let patch = snapshot.get("ssn_a").expect("coalesced patch");
        assert_eq!(patch.agent_session_id.as_deref(), Some("agent-2"));
        assert_eq!(patch.title.as_deref(), Some("grok · merged"));
        assert!(queue.is_dirty());
    }

    #[test]
    fn unchanged_agent_session_id_is_no_op() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_noop")).unwrap();

        let queue = ManifestSyncQueue::new();
        queue.enqueue(
            "ssn_noop".into(),
            ManifestPatch {
                agent_session_id: Some("agent-old".into()),
                agent: None,
                title: None,
                messaged_at: None,
            },
        );

        let before = fs::metadata(manifest_path(dir.path()))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        queue.flush(&config).unwrap();
        let after = fs::metadata(manifest_path(dir.path()))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);

        let loaded = load_manifest(&config).unwrap();
        assert_eq!(
            loaded.entries[0].agent_session_id.as_deref(),
            Some("agent-old")
        );
    }

    #[test]
    fn rapid_enqueue_drains_to_single_manifest_write() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_a")).unwrap();
        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_b".into(),
                ..sample_entry("ssn_b")
            },
        )
        .unwrap();

        let queue = ManifestSyncQueue::new();
        for (ssn, agent_id) in [("ssn_a", "agent-a"), ("ssn_b", "agent-b")] {
            for _ in 0..5 {
                queue.enqueue(
                    ssn.into(),
                    ManifestPatch {
                        agent_session_id: Some(agent_id.into()),
                        agent: None,
                        title: None,
                        messaged_at: None,
                    },
                );
            }
        }

        let before = fs::metadata(manifest_path(dir.path()))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        queue.flush(&config).unwrap();

        let after = fs::metadata(manifest_path(dir.path()))
            .unwrap()
            .modified()
            .unwrap();
        assert!(after > before);

        let loaded = load_manifest(&config).unwrap();
        assert_eq!(loaded.version, MANIFEST_VERSION);
        let a = loaded
            .entries
            .iter()
            .find(|entry| entry.sessions_session_id == "ssn_a")
            .unwrap();
        let b = loaded
            .entries
            .iter()
            .find(|entry| entry.sessions_session_id == "ssn_b")
            .unwrap();
        assert_eq!(a.agent_session_id.as_deref(), Some("agent-a"));
        assert_eq!(b.agent_session_id.as_deref(), Some("agent-b"));

        assert!(!queue.is_dirty());
        assert!(queue.pending_snapshot().is_empty());
    }

    #[test]
    fn generation_bumps_on_enqueue() {
        let queue = ManifestSyncQueue::new();
        assert_eq!(queue.generation(), 0);
        queue.enqueue(
            "ssn".into(),
            ManifestPatch {
                agent_session_id: Some("agent".into()),
                agent: None,
                title: None,
                messaged_at: None,
            },
        );
        assert_eq!(queue.generation(), 1);
        queue.enqueue(
            "ssn".into(),
            ManifestPatch {
                agent_session_id: Some("agent-2".into()),
                agent: None,
                title: None,
                messaged_at: None,
            },
        );
        assert_eq!(queue.generation(), 2);
    }

    #[test]
    fn flush_failure_retains_pending_patches() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_fail")).unwrap();

        let path = manifest_path(dir.path());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();

        let queue = ManifestSyncQueue::new();
        queue.enqueue(
            "ssn_fail".into(),
            ManifestPatch {
                agent_session_id: Some("agent-new".into()),
                agent: None,
                title: None,
                messaged_at: None,
            },
        );

        assert!(queue.flush(&config).is_err());
        assert!(queue.is_dirty());
        let pending = queue.pending_snapshot();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending
                .get("ssn_fail")
                .and_then(|patch| patch.agent_session_id.as_deref()),
            Some("agent-new")
        );
    }

    #[test]
    fn flush_skips_clear_when_generation_changes_during_window() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_gen")).unwrap();

        let queue = ManifestSyncQueue::new();
        queue.enqueue(
            "ssn_gen".into(),
            ManifestPatch {
                agent_session_id: Some("agent-first".into()),
                agent: None,
                title: None,
                messaged_at: None,
            },
        );
        let stable_generation = queue.generation();

        queue.enqueue(
            "ssn_gen".into(),
            ManifestPatch {
                agent_session_id: Some("agent-second".into()),
                agent: None,
                title: None,
                messaged_at: None,
            },
        );
        assert_ne!(queue.generation(), stable_generation);

        let patches = queue.pending_snapshot();
        ManifestSyncQueue::persist_patches(&config, &patches).unwrap();

        if queue.generation() == stable_generation {
            queue.clear_pending();
        }

        assert!(queue.is_dirty());
        assert_eq!(
            queue
                .pending_snapshot()
                .get("ssn_gen")
                .and_then(|patch| patch.agent_session_id.as_deref()),
            Some("agent-second")
        );
    }

    #[test]
    fn debounce_ready_requires_stable_generation() {
        assert!(!flush_after_debounce(1, 2, true));
        assert!(flush_after_debounce(2, 2, true));
        assert!(!flush_after_debounce(2, 2, false));
    }

    #[test]
    fn debounce_gate_matches_manifest_persist_loop_contract() {
        // manifest_persist_loop in server.rs waits MANIFEST_PERSIST_DEBOUNCE_MS (500ms)
        // until generation stops changing before calling flush.
        assert!(!flush_after_debounce(0, 1, true));
        assert!(flush_after_debounce(3, 3, true));
        assert!(!flush_after_debounce(3, 3, false));
    }
}
