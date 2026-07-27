use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Hard cap on cached parse results. Prevents unbounded growth when the daemon
/// walks many agent history files (Codex rollouts can be hundreds of MB each).
const MAX_CACHE_ENTRIES: usize = 256;

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

/// LRU-ordered map: `order` is oldest → newest (front is eviction candidate).
struct ParseCache {
    map: HashMap<FileVersion, serde_json::Value>,
    /// Access order; index 0 is least-recently-used.
    order: Vec<FileVersion>,
}

impl ParseCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    fn touch(&mut self, version: &FileVersion) {
        if let Some(pos) = self.order.iter().position(|v| v == version) {
            let key = self.order.remove(pos);
            self.order.push(key);
        }
    }

    fn get(&mut self, version: &FileVersion) -> Option<&serde_json::Value> {
        if self.map.contains_key(version) {
            self.touch(version);
            self.map.get(version)
        } else {
            None
        }
    }

    fn insert(&mut self, version: FileVersion, value: serde_json::Value) {
        if self.map.contains_key(&version) {
            self.map.insert(version.clone(), value);
            self.touch(&version);
            return;
        }
        while self.map.len() >= MAX_CACHE_ENTRIES {
            self.evict_one();
        }
        self.order.push(version.clone());
        self.map.insert(version, value);
    }

    fn evict_one(&mut self) {
        if let Some(oldest) = self.order.first().cloned() {
            self.order.remove(0);
            self.map.remove(&oldest);
        }
    }
}

static CACHE: Mutex<Option<ParseCache>> = Mutex::new(None);

/// Clear the parse cache — used in tests and after poll boundaries if needed.
pub fn clear_parse_cache() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(ParseCache::new());
    }
}

/// Current number of cached entries (test / diagnostics).
#[cfg(test)]
fn cache_len() -> usize {
    CACHE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|c| c.len()))
        .unwrap_or(0)
}

fn cache_store<T: serde::Serialize + Clone + serde::de::DeserializeOwned>(
    path: &Path,
    value: &T,
) -> Option<T> {
    let version = file_version(path)?;
    let encoded = serde_json::to_value(value).ok()?;
    let mut guard = CACHE.lock().ok()?;
    let cache = guard.get_or_insert_with(ParseCache::new);
    cache.insert(version, encoded);
    Some(value.clone())
}

fn cache_lookup<T: serde::de::DeserializeOwned + Clone>(path: &Path) -> Option<T> {
    let version = file_version(path)?;
    let mut guard = CACHE.lock().ok()?;
    let cache = guard.as_mut()?;
    let encoded = cache.get(&version)?;
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
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    /// Global cache is process-wide; serialize tests that assert hit/miss counts.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_cache_tests() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        value: u32,
    }

    fn write_sample(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Parse counter local to a single test via a shared atomic ref held only while locked.
    fn counting_parse_with(
        counter: &std::sync::atomic::AtomicU32,
    ) -> impl Fn(&Path) -> Option<Sample> + '_ {
        move |path: &Path| {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = std::fs::read_to_string(path).ok()?;
            Some(Sample { value: 7 })
        }
    }

    #[test]
    fn cached_jsonl_parse_hits_cache_for_unchanged_file() {
        let _guard = lock_cache_tests();
        clear_parse_cache();
        let parses = std::sync::atomic::AtomicU32::new(0);
        let dir = TempDir::new().unwrap();
        let path = write_sample(dir.path(), "rollout.jsonl", "{}\n");

        assert_eq!(
            cached_jsonl_parse(&path, counting_parse_with(&parses)),
            Some(Sample { value: 7 })
        );
        assert_eq!(
            cached_jsonl_parse(&path, counting_parse_with(&parses)),
            Some(Sample { value: 7 })
        );
        assert_eq!(parses.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn cache_evicts_lru_when_over_cap() {
        let _guard = lock_cache_tests();
        clear_parse_cache();
        let parses = std::sync::atomic::AtomicU32::new(0);
        let dir = TempDir::new().unwrap();

        let mut paths = Vec::new();
        for i in 0..(MAX_CACHE_ENTRIES + 8) {
            // Unique content length so FileVersion::len differs even if mtime collides.
            let body = format!("{{\"i\":{i}}}{}\n", "x".repeat(i));
            paths.push(write_sample(
                dir.path(),
                &format!("rollout-{i:04}.jsonl"),
                &body,
            ));
        }

        for path in &paths {
            assert!(cached_jsonl_parse(path, counting_parse_with(&parses)).is_some());
        }
        assert!(cache_len() <= MAX_CACHE_ENTRIES);
        assert_eq!(cache_len(), MAX_CACHE_ENTRIES);

        // Oldest entries (first inserted, never re-touched) should be gone.
        parses.store(0, std::sync::atomic::Ordering::Relaxed);
        assert!(cached_jsonl_parse(&paths[0], counting_parse_with(&parses)).is_some());
        assert_eq!(parses.load(std::sync::atomic::Ordering::Relaxed), 1);

        // paths[len-2] was second-to-last of the original insert wave; after re-inserting
        // paths[0] (which evicted one LRU), it should still be present.
        parses.store(0, std::sync::atomic::Ordering::Relaxed);
        let late = &paths[paths.len() - 2];
        assert!(cached_jsonl_parse(late, counting_parse_with(&parses)).is_some());
        assert_eq!(parses.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn cache_lookup_promotes_entry_to_mru() {
        let _guard = lock_cache_tests();
        clear_parse_cache();
        let parses = std::sync::atomic::AtomicU32::new(0);
        let dir = TempDir::new().unwrap();

        // Fill to capacity.
        let mut paths = Vec::new();
        for i in 0..MAX_CACHE_ENTRIES {
            let body = format!("{{\"i\":{i}}}{}\n", "y".repeat(i));
            paths.push(write_sample(
                dir.path(),
                &format!("file-{i:04}.jsonl"),
                &body,
            ));
            assert!(cached_jsonl_parse(&paths[i], counting_parse_with(&parses)).is_some());
        }
        assert_eq!(cache_len(), MAX_CACHE_ENTRIES);

        // Touch the oldest entry so it becomes MRU.
        parses.store(0, std::sync::atomic::Ordering::Relaxed);
        assert!(cached_jsonl_parse(&paths[0], counting_parse_with(&parses)).is_some());
        assert_eq!(parses.load(std::sync::atomic::Ordering::Relaxed), 0);

        // Insert one more → should evict the new LRU (paths[1]), not paths[0].
        let extra_body = format!("{{\"i\":{}}}\n", "z".repeat(MAX_CACHE_ENTRIES + 3));
        let extra = write_sample(dir.path(), "extra.jsonl", &extra_body);
        assert!(cached_jsonl_parse(&extra, counting_parse_with(&parses)).is_some());
        assert_eq!(cache_len(), MAX_CACHE_ENTRIES);

        parses.store(0, std::sync::atomic::Ordering::Relaxed);
        assert!(cached_jsonl_parse(&paths[0], counting_parse_with(&parses)).is_some());
        assert_eq!(
            parses.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "paths[0] was promoted and must survive eviction"
        );

        parses.store(0, std::sync::atomic::Ordering::Relaxed);
        assert!(cached_jsonl_parse(&paths[1], counting_parse_with(&parses)).is_some());
        assert_eq!(
            parses.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "paths[1] should have been LRU-evicted"
        );
    }

    #[test]
    fn clear_parse_cache_empties_map() {
        let _guard = lock_cache_tests();
        clear_parse_cache();
        let parses = std::sync::atomic::AtomicU32::new(0);
        let dir = TempDir::new().unwrap();
        let path = write_sample(dir.path(), "a.jsonl", "{}\n");
        assert!(cached_jsonl_parse(&path, counting_parse_with(&parses)).is_some());
        assert!(cache_len() >= 1);
        clear_parse_cache();
        assert_eq!(cache_len(), 0);
    }
}
