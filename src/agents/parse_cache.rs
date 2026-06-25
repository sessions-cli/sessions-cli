use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Clone, PartialEq, Eq)]
struct FileVersion {
    path: PathBuf,
    len: u64,
    mtime_ms: u128,
}

impl Hash for FileVersion {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.len.hash(state);
        self.mtime_ms.hash(state);
    }
}

fn file_version(path: &Path) -> Option<FileVersion> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Some(FileVersion {
        path: path.to_path_buf(),
        len: meta.len(),
        mtime_ms,
    })
}

static CACHE: Mutex<Option<HashMap<FileVersion, serde_json::Value>>> = Mutex::new(None);

/// Clear the parse cache — used in tests and after poll boundaries if needed.
pub fn clear_parse_cache() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(HashMap::new());
    }
}

fn cache_store<T: serde::Serialize + Clone + serde::de::DeserializeOwned>(
    path: &Path,
    value: &T,
) -> Option<T> {
    let version = file_version(path)?;
    let encoded = serde_json::to_value(value).ok()?;
    let mut guard = CACHE.lock().ok()?;
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(version, encoded);
    Some(value.clone())
}

fn cache_lookup<T: serde::de::DeserializeOwned + Clone>(path: &Path) -> Option<T> {
    let version = file_version(path)?;
    let guard = CACHE.lock().ok()?;
    let map = guard.as_ref()?;
    let encoded = map.get(&version)?;
    serde_json::from_value(encoded.clone()).ok()
}

/// Parse a JSONL agent log once per `(path, len, mtime)` and reuse the result.
pub fn cached_jsonl_parse<T, F>(path: &Path, parse: F) -> Option<T>
where
    T: Clone + serde::Serialize + serde::de::DeserializeOwned,
    F: FnOnce(&Path) -> Option<T>,
{
    if let Some(hit) = cache_lookup::<T>(path) {
        return Some(hit);
    }
    let parsed = parse(path)?;
    crate::daemon::metrics::record_log_parse();
    cache_store(path, &parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        value: u32,
    }

    static PARSE_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn counting_parse(path: &Path) -> Option<Sample> {
        PARSE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::read_to_string(path).ok()?;
        Some(Sample { value: 7 })
    }

    #[test]
    fn cached_jsonl_parse_hits_cache_for_unchanged_file() {
        clear_parse_cache();
        PARSE_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rollout.jsonl");
        std::fs::write(&path, "{}\n").unwrap();

        assert_eq!(
            cached_jsonl_parse(&path, counting_parse),
            Some(Sample { value: 7 })
        );
        assert_eq!(
            cached_jsonl_parse(&path, counting_parse),
            Some(Sample { value: 7 })
        );
        assert_eq!(PARSE_COUNT.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
