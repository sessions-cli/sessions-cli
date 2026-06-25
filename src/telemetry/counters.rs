use super::feature::{FeatureId, Source};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use crate::paths;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeatureCount {
    pub feature: String,
    pub source: String,
    pub count: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingFlush {
    pub feature_counts: Vec<FeatureCount>,
    pub sidebar_attach_count: u64,
    pub launcher_opens: u64,
    pub launcher_submits: u64,
}

static COUNTERS: Mutex<Option<PendingFlush>> = Mutex::new(None);

fn counters_mut() -> std::sync::MutexGuard<'static, Option<PendingFlush>> {
    let mut guard = COUNTERS.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(PendingFlush::default());
    }
    guard
}

pub fn record_feature(feature: FeatureId, source: Source) {
    let mut guard = counters_mut();
    let pending = guard.as_mut().unwrap();
    let feature_str = feature.as_str().to_string();
    let source_str = source.as_str().to_string();
    if let Some(entry) = pending
        .feature_counts
        .iter_mut()
        .find(|e| e.feature == feature_str && e.source == source_str)
    {
        entry.count += 1;
    } else {
        pending.feature_counts.push(FeatureCount {
            feature: feature_str,
            source: source_str,
            count: 1,
        });
    }
    match feature {
        FeatureId::SidebarAttach => pending.sidebar_attach_count += 1,
        FeatureId::LauncherOpen => pending.launcher_opens += 1,
        FeatureId::LauncherSubmit => pending.launcher_submits += 1,
        _ => {}
    }
}

pub fn take_pending() -> PendingFlush {
    let mut guard = counters_mut();
    let taken = guard.take().unwrap_or_default();
    *guard = Some(PendingFlush::default());
    taken
}

pub fn merge_pending(into: &mut PendingFlush, from: PendingFlush) {
    for entry in from.feature_counts {
        if let Some(existing) = into
            .feature_counts
            .iter_mut()
            .find(|e| e.feature == entry.feature && e.source == entry.source)
        {
            existing.count += entry.count;
        } else {
            into.feature_counts.push(entry);
        }
    }
    into.sidebar_attach_count += from.sidebar_attach_count;
    into.launcher_opens += from.launcher_opens;
    into.launcher_submits += from.launcher_submits;
}

pub fn write_pending_file(home: &Path, pending: &PendingFlush) -> anyhow::Result<()> {
    let path = paths::telemetry_pending_path(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string(pending)?;
    fs::write(path, raw)?;
    Ok(())
}

pub fn read_pending_file(home: &Path) -> PendingFlush {
    let path = paths::telemetry_pending_path(home);
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn flush_pending(home: &Path) -> PendingFlush {
    let mut pending = take_pending();
    let file_pending = read_pending_file(home);
    merge_pending(&mut pending, file_pending);
    let _ = fs::remove_file(paths::telemetry_pending_path(home));
    pending
}

pub fn save_pending_to_file(home: &Path) -> anyhow::Result<bool> {
    let pending = take_pending();
    if pending.feature_counts.is_empty()
        && pending.sidebar_attach_count == 0
        && pending.launcher_opens == 0
        && pending.launcher_submits == 0
    {
        return Ok(false);
    }
    let mut existing = read_pending_file(home);
    merge_pending(&mut existing, pending);
    write_pending_file(home, &existing)?;
    Ok(true)
}