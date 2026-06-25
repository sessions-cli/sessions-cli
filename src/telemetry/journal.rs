use super::feature::{FeatureId, Source};
use super::guards::reject_sensitive_strings;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::paths;

const MAX_JOURNAL_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Serialize)]
struct JournalEntry<'a> {
    kind: &'a str,
    #[serde(flatten)]
    body: serde_json::Value,
    at: String,
}

pub fn record_feature(feature: FeatureId, source: Source) {
    let _ = append_entry(JournalEntry {
        kind: "feature_used",
        body: serde_json::json!({
            "feature": feature.as_str(),
            "source": source.as_str(),
            "n": 1,
        }),
        at: chrono::Utc::now().to_rfc3339(),
    });
}

pub fn record_event(name: &str, props: serde_json::Value) {
    let _ = append_entry(JournalEntry {
        kind: "event",
        body: serde_json::json!({
            "name": name,
            "props": props,
        }),
        at: chrono::Utc::now().to_rfc3339(),
    });
}

fn append_entry(entry: JournalEntry<'_>) -> anyhow::Result<()> {
    let home = crate::paths::home();
    let dir = paths::telemetry_dir(&home);
    fs::create_dir_all(&dir)?;
    let path = dir.join("events.jsonl");
    rotate_if_needed(&path)?;
    let line = serde_json::to_string(&entry)?;
    reject_sensitive_strings(&line)?;
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn rotate_if_needed(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let size = fs::metadata(path)?.len();
    if size < MAX_JOURNAL_BYTES {
        return Ok(());
    }
    let rotated = path.with_extension("jsonl.1");
    let _ = fs::remove_file(&rotated);
    fs::rename(path, rotated)?;
    Ok(())
}

pub fn export_journal() -> anyhow::Result<String> {
    let home = crate::paths::home();
    let dir = paths::telemetry_dir(&home);
    let path = dir.join("events.jsonl");
    if !path.exists() {
        return Ok(String::new());
    }
    Ok(fs::read_to_string(path)?)
}