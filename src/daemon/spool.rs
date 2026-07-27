use crate::config::Config;
use crate::model::NotifyMessage;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

/// A spooled notification still backed by its on-disk file.
///
/// The file is intentionally *not* removed when the message is read. The
/// caller must apply the message and then call [`SpooledMessage::ack`] to
/// delete the backing file. This gives at-least-once delivery: a crash
/// between reading and applying leaves the file in place for the next drain.
pub struct SpooledMessage {
    pub msg: NotifyMessage,
    path: PathBuf,
}

impl SpooledMessage {
    /// Remove the backing file after the message has been applied.
    ///
    /// A failure here is non-fatal: the file simply survives to be retried on
    /// the next drain, so [`crate::daemon::state::DaemonState::handle_notify`]
    /// must remain idempotent.
    pub fn ack(self) {
        if let Err(e) = fs::remove_file(&self.path) {
            warn!("failed to remove spool file {}: {e}", self.path.display());
        }
    }
}

/// Read all valid spooled messages without deleting them.
///
/// Invalid files are quarantined immediately (they can never be applied, so
/// retaining them would block the drain forever). Valid files are returned to
/// the caller, which is responsible for acking each one after application.
pub fn drain_spool(spool_dir: &Path) -> Result<Vec<SpooledMessage>> {
    if !spool_dir.exists() {
        return Ok(Vec::new());
    }
    let mut messages = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(spool_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(msg) = serde_json::from_str::<NotifyMessage>(data.trim()) {
            messages.push(SpooledMessage { msg, path });
            continue;
        }
        warn!("quarantining invalid spool file {}", path.display());
        let _ = quarantine_spool_file(&path);
    }
    Ok(messages)
}

pub fn drain_config_spool(config: &Config) -> Result<Vec<SpooledMessage>> {
    drain_spool(&config.spool_dir)
}

fn quarantine_spool_file(path: &Path) -> Result<()> {
    let invalid_path = path.with_extension("invalid");
    if invalid_path.exists() {
        fs::remove_file(path)?;
    } else {
        fs::rename(path, invalid_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn drain_retains_files_until_acked() {
        let dir = TempDir::new().unwrap();
        let msg = r#"{"t":"grok","event":"stop","ts":1,"payload":{}}"#;
        fs::write(dir.path().join("100-test.json"), msg).unwrap();

        let drained = drain_spool(dir.path()).unwrap();
        assert_eq!(drained.len(), 1);
        // File survives until the caller acks — a crash before ack retries it.
        assert!(dir.path().join("100-test.json").exists());

        // A second drain (simulating a restart before ack) re-delivers it.
        let redrained = drain_spool(dir.path()).unwrap();
        assert_eq!(redrained.len(), 1);

        for spooled in redrained {
            spooled.ack();
        }
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn drain_quarantines_invalid_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("100-test.json"), "{not-json").unwrap();
        let drained = drain_spool(dir.path()).unwrap();
        assert!(drained.is_empty());
        assert!(dir.path().join("100-test.invalid").exists());
    }
}
