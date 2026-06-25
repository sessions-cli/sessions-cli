use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};

use crate::agents::adapter::SessionSummary;
use crate::pty::naming::{is_confident_thread_title, is_weak_thread_name, shorten_prompt};

static POLL_CACHE: RwLock<Option<OpenCodeSessionIndex>> = RwLock::new(None);

#[derive(Debug, Clone)]
pub struct OpenCodeSessionEntry {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub time_updated: i64,
}

#[derive(Debug, Default, Clone)]
pub struct OpenCodeSessionIndex {
    by_cwd: HashMap<String, Vec<OpenCodeSessionEntry>>,
    by_id: HashMap<String, OpenCodeSessionEntry>,
}

pub fn is_opencode_session_id(session_id: &str) -> bool {
    session_id.starts_with("ses_")
}

pub fn opencode_data_dir(home: &Path) -> PathBuf {
    home.join(".local/share/opencode")
}

pub fn opencode_db_path(home: &Path) -> PathBuf {
    opencode_data_dir(home).join("opencode.db")
}

/// Build the index once per poll (inside `spawn_blocking`). Also seeds the in-memory
/// lookup cache so per-session merge work stays O(1) without repeat SQLite opens.
pub fn opencode_session_index(home: &Path) -> OpenCodeSessionIndex {
    let index = load_session_index(home);
    if let Ok(mut cache) = POLL_CACHE.write() {
        *cache = Some(index.clone());
    }
    index
}

pub fn assign_session_for_cwd(
    index: &OpenCodeSessionIndex,
    cwd: &str,
    assigned: &mut HashSet<String>,
) -> Option<String> {
    let entries = index.by_cwd.get(normalize_cwd(cwd).as_str())?;
    entries
        .iter()
        .find(|entry| !assigned.contains(&entry.id))
        .map(|entry| {
            assigned.insert(entry.id.clone());
            entry.id.clone()
        })
}

pub fn session_exists(home: &Path, session_id: &str) -> bool {
    if !is_opencode_session_id(session_id) {
        return false;
    }
    if lookup_cached(session_id).is_some() {
        return true;
    }
    lookup_session_row(home, session_id).is_some()
}

pub fn load_session_summary(home: &Path, _cwd: &str, session_id: &str) -> Option<SessionSummary> {
    let entry = lookup_cached(session_id).or_else(|| lookup_session_row(home, session_id))?;
    let title = session_title_for_summary(&entry.title, home, &entry.id)?;
    Some(SessionSummary {
        generated_title: Some(title),
        session_summary: None,
        agent_name: Some("opencode".into()),
    })
}

pub fn session_cwd_for_id(home: &Path, session_id: &str) -> Option<String> {
    lookup_cached(session_id)
        .or_else(|| lookup_session_row(home, session_id))
        .map(|entry| normalize_cwd(&entry.directory))
}

pub fn session_messaged_at(home: &Path, session_id: &str) -> Option<DateTime<Utc>> {
    let conn = open_readonly(&opencode_db_path(home)).ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT MAX(p.time_created)
             FROM part p
             JOIN message m ON p.message_id = m.id
             WHERE m.session_id = ?1
               AND json_extract(m.data, '$.role') = 'user'
               AND json_extract(p.data, '$.type') = 'text'",
        )
        .ok()?;
    let ms: i64 = stmt.query_row([session_id], |row| row.get(0)).ok()?;
    millis_to_utc(ms).or_else(|| session_activity_at(home, session_id))
}

pub fn session_activity_at(home: &Path, session_id: &str) -> Option<DateTime<Utc>> {
    let entry = lookup_cached(session_id).or_else(|| lookup_session_row(home, session_id))?;
    millis_to_utc(entry.time_updated)
}

pub fn thread_title_from_summary(summary: &SessionSummary) -> Option<String> {
    summary
        .generated_title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(|title| {
            let thread = shorten_prompt(title);
            if thread.is_empty() || is_weak_thread_name(&thread) {
                title.to_string()
            } else {
                thread
            }
        })
        .filter(|thread| is_confident_thread_title(thread))
}

fn load_session_index(home: &Path) -> OpenCodeSessionIndex {
    let path = opencode_db_path(home);
    let Ok(conn) = open_readonly(&path) else {
        return OpenCodeSessionIndex::default();
    };
    let mut stmt = match conn.prepare(
        "SELECT id, title, directory, time_updated
         FROM session
         WHERE directory IS NOT NULL AND directory != ''",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return OpenCodeSessionIndex::default(),
    };
    let rows = stmt.query_map([], |row| {
        Ok(OpenCodeSessionEntry {
            id: row.get(0)?,
            title: row.get(1)?,
            directory: row.get(2)?,
            time_updated: row.get(3)?,
        })
    });
    let Ok(rows) = rows else {
        return OpenCodeSessionIndex::default();
    };

    let mut by_cwd: HashMap<String, Vec<OpenCodeSessionEntry>> = HashMap::new();
    let mut by_id = HashMap::new();
    for row in rows.flatten() {
        by_id.insert(row.id.clone(), row.clone());
        by_cwd
            .entry(normalize_cwd(&row.directory))
            .or_default()
            .push(row);
    }
    for entries in by_cwd.values_mut() {
        entries.sort_by(|a, b| b.time_updated.cmp(&a.time_updated));
    }
    OpenCodeSessionIndex { by_cwd, by_id }
}

fn lookup_cached(session_id: &str) -> Option<OpenCodeSessionEntry> {
    let cache = POLL_CACHE.read().ok()?;
    cache.as_ref()?.by_id.get(session_id).cloned()
}

fn lookup_session_row(home: &Path, session_id: &str) -> Option<OpenCodeSessionEntry> {
    let conn = open_readonly(&opencode_db_path(home)).ok()?;
    conn.query_row(
        "SELECT id, title, directory, time_updated FROM session WHERE id = ?1",
        [session_id],
        |row| {
            Ok(OpenCodeSessionEntry {
                id: row.get(0)?,
                title: row.get(1)?,
                directory: row.get(2)?,
                time_updated: row.get(3)?,
            })
        },
    )
    .ok()
}

fn session_title_for_summary(title: &str, home: &Path, session_id: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() {
        return first_user_prompt(home, session_id);
    }
    if is_weak_opencode_title(title) {
        return first_user_prompt(home, session_id);
    }
    Some(title.to_string())
}

fn is_weak_opencode_title(title: &str) -> bool {
    title.starts_with("New session -") || is_weak_thread_name(title)
}

fn first_user_prompt(home: &Path, session_id: &str) -> Option<String> {
    let conn = open_readonly(&opencode_db_path(home)).ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT p.data
             FROM part p
             JOIN message m ON p.message_id = m.id
             WHERE m.session_id = ?1
               AND json_extract(m.data, '$.role') = 'user'
               AND json_extract(p.data, '$.type') = 'text'
             ORDER BY p.time_created
             LIMIT 1",
        )
        .ok()?;
    let data: String = stmt.query_row([session_id], |row| row.get(0)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&data).ok()?;
    let prompt = value.get("text").and_then(|v| v.as_str())?.trim();
    if prompt.is_empty() {
        return None;
    }
    let shortened = shorten_prompt(prompt);
    is_confident_thread_title(&shortened).then_some(shortened)
}

pub(super) fn open_readonly(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

fn normalize_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.to_string()
    }
}

pub(super) fn millis_to_utc(ms: i64) -> Option<DateTime<Utc>> {
    let secs = ms / 1000;
    let nanos = ((ms % 1000).max(0) as u32) * 1_000_000;
    Utc.timestamp_opt(secs, nanos).single()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::adapter::AgentAdapter;
    use crate::agents::opencode::OpenCode;
    use crate::model::AgentState;
    use std::fs;
    use tempfile::TempDir;

    fn seed_db(home: &Path) -> rusqlite::Result<()> {
        let path = opencode_db_path(home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                directory TEXT NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )?;
        conn.execute(
            "INSERT INTO session (id, title, directory, time_updated) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                "ses_test001",
                "Fix opencode sidebar titles",
                env!("CARGO_MANIFEST_DIR"),
                1_781_134_800_000_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO session (id, title, directory, time_updated) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                "ses_test002",
                "New session - 2026-06-11T00:00:00.000Z",
                env!("CARGO_MANIFEST_DIR"),
                1_781_134_700_000_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('msg1', 'ses_test002', 1781134700000, 1781134700000, '{\"role\":\"user\"}')",
            [],
        )?;
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data)
             VALUES ('prt1', 'msg1', 'ses_test002', 1, '{\"type\":\"text\",\"text\":\"drop readme hash title\"}')",
            [],
        )?;
        Ok(())
    }

    #[test]
    fn opencode_session_id_prefix_is_fast_path() {
        assert!(is_opencode_session_id("ses_14c367547ffe7g1N1inGGONKuZ"));
        assert!(!is_opencode_session_id(
            "019eb088-9748-7ef2-86ba-4d7e20f5a576"
        ));
    }

    #[test]
    fn session_index_groups_by_cwd_and_assigns_uniques() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        seed_db(home).unwrap();
        let index = opencode_session_index(home);
        let cwd = env!("CARGO_MANIFEST_DIR");
        let mut assigned = HashSet::new();
        let first = assign_session_for_cwd(&index, cwd, &mut assigned).unwrap();
        let second = assign_session_for_cwd(&index, cwd, &mut assigned).unwrap();
        assert_ne!(first, second);
        assert_eq!(assigned.len(), 2);
    }

    #[test]
    fn load_summary_uses_title_or_first_user_prompt() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        seed_db(home).unwrap();
        opencode_session_index(home);

        let strong = load_session_summary(home, cwd(), "ses_test001").unwrap();
        assert_eq!(
            strong.generated_title.as_deref(),
            Some("Fix opencode sidebar titles")
        );

        let weak = load_session_summary(home, cwd(), "ses_test002").unwrap();
        assert_eq!(
            weak.generated_title.as_deref(),
            Some("drop readme hash title")
        );
    }

    #[test]
    fn poll_cache_avoids_repeat_db_reads() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        seed_db(home).unwrap();
        opencode_session_index(home);
        fs::remove_file(opencode_db_path(home)).unwrap();

        assert_eq!(
            session_cwd_for_id(home, "ses_test001").as_deref(),
            Some(env!("CARGO_MANIFEST_DIR"))
        );
    }

    #[test]
    fn turn_boundary_reads_user_and_stop_messages() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let path = opencode_db_path(home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY, title TEXT, directory TEXT, time_updated INTEGER
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY, message_id TEXT NOT NULL,
                session_id TEXT NOT NULL, time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute("INSERT INTO session (id, title, directory, time_updated) VALUES ('ses_test', '', ?, 100)", rusqlite::params![env!("CARGO_MANIFEST_DIR")]).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_u1', 'ses_test', 100, 100, '{\"role\":\"user\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_a1', 'ses_test', 200, 500, '{\"role\":\"assistant\",\"finish\":\"tool-calls\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_a2', 'ses_test', 600, 900, '{\"role\":\"assistant\",\"finish\":\"stop\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data)
             VALUES ('p1', 'msg_u1', 'ses_test', 100, '{\"type\":\"text\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data)
             VALUES ('p2', 'msg_a1', 'ses_test', 500, '{\"type\":\"text\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data)
             VALUES ('p3', 'msg_a2', 'ses_test', 900, '{\"type\":\"text\"}')",
            [],
        )
        .unwrap();

        // ses_test: user msg at 100, assistant finished with stop at 900
        let boundary = OpenCode.turn_boundary(home, env!("CARGO_MANIFEST_DIR"), "ses_test").unwrap();
        assert_eq!(boundary.last_started.map(|t| t.timestamp_millis()), Some(100));
        assert_eq!(boundary.last_completed.map(|t| t.timestamp_millis()), Some(900));
        assert!(crate::agents::turn_is_complete(&boundary));
    }

    #[test]
    fn live_activity_returns_working_when_turn_not_complete() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let path = opencode_db_path(home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY, title TEXT, directory TEXT, time_updated INTEGER
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY, message_id TEXT NOT NULL,
                session_id TEXT NOT NULL, time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute("INSERT INTO session (id, title, directory, time_updated) VALUES ('ses_active', '', ?, 100)", rusqlite::params![env!("CARGO_MANIFEST_DIR")]).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_u1', 'ses_active', 100, 100, '{\"role\":\"user\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data)
             VALUES ('p1', 'msg_u1', 'ses_active', 100, '{\"type\":\"text\"}')",
            [],
        )
        .unwrap();

        // No assistant response yet → turn not complete → working
        let activity = OpenCode
            .live_activity(home, env!("CARGO_MANIFEST_DIR"), "ses_active")
            .unwrap();
        assert_eq!(activity.state, AgentState::Working);
        assert_eq!(activity.at.timestamp_millis(), 100);
    }

    #[test]
    fn live_activity_returns_none_for_completed_turn() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let path = opencode_db_path(home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY, title TEXT, directory TEXT, time_updated INTEGER
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY, message_id TEXT NOT NULL,
                session_id TEXT NOT NULL, time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute("INSERT INTO session (id, title, directory, time_updated) VALUES ('ses_done', '', ?, 500)", rusqlite::params![env!("CARGO_MANIFEST_DIR")]).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_u1', 'ses_done', 100, 100, '{\"role\":\"user\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_a1', 'ses_done', 200, 500, '{\"role\":\"assistant\",\"finish\":\"stop\"}')",
            [],
        )
        .unwrap();

        assert!(OpenCode
            .live_activity(home, env!("CARGO_MANIFEST_DIR"), "ses_done")
            .is_none());
    }

    fn cwd() -> &'static str {
        env!("CARGO_MANIFEST_DIR")
    }
}
