use crate::config::Config;
use crate::paths;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

/// True when `config` resolves to the same state directory as the process home.
/// Used in tests to block accidental writes to a developer's live notepad.
#[cfg(test)]
pub(crate) fn is_live_notepad_config(config: &Config) -> bool {
    paths::state_dir(&config.home) == paths::state_dir(&paths::home())
}

#[cfg(test)]
fn assert_isolated_notepad_config(config: &Config) {
    if is_live_notepad_config(config) {
        panic!(
            "notepad persistence must use an isolated config in tests — \
             set config.home to a tempfile, never Config::default() alone. \
             Live path would be: {}",
            config.sidebar_notepad_dir().display()
        );
    }
}

#[cfg(test)]
pub fn isolated_test_config() -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.home = dir.path().to_path_buf();
    (dir, config)
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub text: String,
    #[serde(default)]
    pub expanded: bool,
}

impl Note {
    pub fn new(title: impl Into<String>, text: impl Into<String>, expanded: bool) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            text: text.into(),
            expanded,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidebarNotepad {
    pub expanded: bool,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub active_note_id: Option<String>,
    #[serde(default = "default_true")]
    pub sessions_expanded: bool,
    #[serde(default)]
    pub notes_list_expanded: bool,
    #[serde(default)]
    pub welcome_seeded: bool,
}

impl Default for SidebarNotepad {
    fn default() -> Self {
        Self {
            expanded: false,
            notes: Vec::new(),
            active_note_id: None,
            sessions_expanded: true,
            notes_list_expanded: false,
            welcome_seeded: false,
        }
    }
}

impl SidebarNotepad {
    pub fn active_note(&self) -> Option<&Note> {
        self.active_note_id
            .as_ref()
            .and_then(|id| self.notes.iter().find(|note| note.id == *id))
            .or_else(|| self.notes.first())
    }
}

pub fn default_note_title(notes: &[Note]) -> String {
    let mut n = 1usize;
    loop {
        let candidate = format!("Note {n}");
        if !notes.iter().any(|note| note.title == candidate) {
            return candidate;
        }
        n += 1;
    }
}

pub fn url_at(text: &str, cursor: usize) -> Option<String> {
    let cursor = cursor.min(text.chars().count());
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    let byte_idx = bytes
        .iter()
        .position(|(idx, _)| *idx >= cursor)
        .unwrap_or(bytes.len());
    let mut start = byte_idx;
    while start > 0 {
        let ch = bytes[start - 1].1;
        if ch.is_whitespace() || matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>') {
            break;
        }
        start -= 1;
    }
    let mut end = byte_idx;
    while end < bytes.len() {
        let ch = bytes[end].1;
        if ch.is_whitespace() || matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>') {
            break;
        }
        end += 1;
    }
    if start >= end {
        return None;
    }
    let start_byte = bytes.get(start).map(|(idx, _)| *idx).unwrap_or(0);
    let end_byte = bytes
        .get(end)
        .map(|(idx, _)| *idx)
        .unwrap_or(text.len());
    let token = &text[start_byte..end_byte];
    if token.starts_with("http://") || token.starts_with("https://") {
        Some(token.to_string())
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct NotepadPrefs {
    pub expanded: bool,
    #[serde(default)]
    pub active_note_id: Option<String>,
    #[serde(default = "default_true")]
    pub sessions_expanded: bool,
    #[serde(default)]
    pub notes_list_expanded: bool,
    #[serde(default)]
    pub welcome_seeded: bool,
    #[serde(default)]
    pub note_order: Vec<String>,
}

impl From<&SidebarNotepad> for NotepadPrefs {
    fn from(notepad: &SidebarNotepad) -> Self {
        Self {
            expanded: notepad.expanded,
            active_note_id: notepad.active_note_id.clone(),
            sessions_expanded: notepad.sessions_expanded,
            notes_list_expanded: notepad.notes_list_expanded,
            welcome_seeded: notepad.welcome_seeded,
            note_order: notepad.notes.iter().map(|note| note.id.clone()).collect(),
        }
    }
}

pub fn last_saved_at(config: &Config) -> Option<DateTime<Utc>> {
    let path = config.sidebar_notepad_prefs_path();
    let modified = fs::metadata(&path).ok()?.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    DateTime::from_timestamp(duration.as_secs() as i64, duration.subsec_nanos())
}

pub fn load(config: &Config) -> SidebarNotepad {
    load_from_dir(config).unwrap_or_default()
}

/// Write a single note body to disk immediately (used for typing autosave).
pub fn save_note_file(config: &Config, note: &Note) -> Result<()> {
    #[cfg(test)]
    assert_isolated_notepad_config(config);
    let notes_dir = config.sidebar_notepad_notes_dir();
    fs::create_dir_all(&notes_dir)?;
    atomic_write_json(&config.sidebar_note_path(&note.id), note)
}

/// Remove one note file. Only call from explicit user-confirmed delete — never from save/reorder.
pub fn delete_note_file(config: &Config, note_id: &str) -> Result<()> {
    #[cfg(test)]
    assert_isolated_notepad_config(config);
    let path = config.sidebar_note_path(note_id);
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("remove note file {}", path.display()))?;
    }
    Ok(())
}

pub fn save(config: &Config, notepad: &SidebarNotepad) -> Result<()> {
    #[cfg(test)]
    assert_isolated_notepad_config(config);
    let notes_dir = config.sidebar_notepad_notes_dir();
    fs::create_dir_all(&notes_dir)?;
    maybe_backup_notepad_dir(config, notepad)?;

    // Note bodies first so prefs never index files that have not been written yet.
    for note in &notepad.notes {
        atomic_write_json(&config.sidebar_note_path(&note.id), note)?;
    }

    let prefs = NotepadPrefs::from(notepad);
    atomic_write_json(&config.sidebar_notepad_prefs_path(), &prefs)?;

    // Never prune note files here. Orphans stay on disk for recovery; load() discovers them.
    // Only delete_note_file() (user-confirmed delete) removes a note file from disk.
    Ok(())
}

fn load_from_dir(config: &Config) -> Option<SidebarNotepad> {
    let prefs_path = config.sidebar_notepad_prefs_path();
    if !prefs_path.exists() {
        return None;
    }
    let data = fs::read_to_string(&prefs_path).ok()?;
    let prefs: NotepadPrefs = serde_json::from_str(&data).ok()?;
    let notes_dir = config.sidebar_notepad_notes_dir();
    let mut notes = Vec::new();
    let mut seen = HashSet::new();
    for id in &prefs.note_order {
        if let Some(note) = load_note_from_path(&config.sidebar_note_path(id)) {
            seen.insert(note.id.clone());
            notes.push(note);
        }
    }
    let entries = fs::read_dir(&notes_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(note) = load_note_from_path(&path) else {
            continue;
        };
        if seen.insert(note.id.clone()) {
            notes.push(note);
        }
    }
    Some(SidebarNotepad {
        expanded: prefs.expanded,
        notes,
        active_note_id: prefs.active_note_id,
        sessions_expanded: prefs.sessions_expanded,
        notes_list_expanded: prefs.notes_list_expanded,
        welcome_seeded: prefs.welcome_seeded,
    })
}

fn load_note_from_path(path: &Path) -> Option<Note> {
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn prefs_backup_path(path: &Path) -> std::path::PathBuf {
    path.with_extension("json.bak")
}

fn maybe_backup_notepad_dir(config: &Config, next: &SidebarNotepad) -> Result<()> {
    let prefs_path = config.sidebar_notepad_prefs_path();
    if !prefs_path.exists() {
        return Ok(());
    }
    let Some(current) = load_from_dir(config) else {
        return Ok(());
    };
    if current.notes.is_empty() {
        return Ok(());
    }
    if current == *next {
        return Ok(());
    }
    atomic_write_json(&prefs_backup_path(&prefs_path), &NotepadPrefs::from(&current))
}

pub fn cursor_line_col(text: &str, cursor: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut col = 0usize;
    for (idx, ch) in text.chars().enumerate() {
        if idx >= cursor {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

pub fn clamp_cursor(text: &str, cursor: usize) -> usize {
    cursor.min(text.chars().count())
}

pub fn line_len_at(text: &str, line: usize) -> usize {
    let mut current_line = 0usize;
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            if current_line == line {
                return col;
            }
            current_line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    if current_line == line {
        col
    } else {
        0
    }
}

pub fn line_count(text: &str) -> usize {
    logical_line_ranges(text).len().max(1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLine {
    pub start: usize,
    pub text: String,
}

pub fn logical_line_ranges(text: &str) -> Vec<(usize, usize)> {
    if text.is_empty() {
        return vec![(0, 0)];
    }
    let mut ranges = Vec::new();
    let mut line_start = 0usize;
    let mut idx = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            ranges.push((line_start, idx));
            line_start = idx + 1;
        }
        idx += 1;
    }
    ranges.push((line_start, idx));
    ranges
}

fn word_segments(line_start: usize, line: &str) -> Vec<(usize, &str)> {
    let mut segments = Vec::new();
    let mut byte_idx = 0;
    while byte_idx < line.len() {
        let ch = line[byte_idx..].chars().next().unwrap();
        if ch.is_whitespace() {
            byte_idx += ch.len_utf8();
            continue;
        }
        let word_start_byte = byte_idx;
        let char_start = line_start + line[..word_start_byte].chars().count();
        byte_idx += ch.len_utf8();
        while byte_idx < line.len() {
            let next = line[byte_idx..].chars().next().unwrap();
            if next.is_whitespace() {
                break;
            }
            byte_idx += next.len_utf8();
        }
        segments.push((char_start, &line[word_start_byte..byte_idx]));
    }
    segments
}

fn push_hard_broken_word(
    out: &mut Vec<DisplayLine>,
    word_start: usize,
    word: &str,
    width: usize,
) {
    let mut chunk = String::new();
    let mut chunk_start = word_start;
    let mut col = 0usize;
    for (offset, ch) in word.chars().enumerate() {
        if col >= width {
            out.push(DisplayLine {
                start: chunk_start,
                text: chunk,
            });
            chunk = String::new();
            chunk_start = word_start + offset;
            col = 0;
        }
        chunk.push(ch);
        col += 1;
    }
    if !chunk.is_empty() {
        out.push(DisplayLine {
            start: chunk_start,
            text: chunk,
        });
    }
}

fn wrap_logical_line(line_start: usize, line: &str, width: usize, out: &mut Vec<DisplayLine>) {
    let segments = word_segments(line_start, line);
    if segments.is_empty() {
        out.push(DisplayLine {
            start: line_start,
            text: String::new(),
        });
        return;
    }

    let mut current = String::new();
    let mut current_start = line_start;
    let mut col = 0usize;

    for (word_start, word) in segments {
        let word_len = word.chars().count();
        if word_len > width {
            if !current.is_empty() {
                out.push(DisplayLine {
                    start: current_start,
                    text: current,
                });
                current = String::new();
                col = 0;
            }
            push_hard_broken_word(out, word_start, word, width);
            continue;
        }

        let needed = if current.is_empty() {
            word_len
        } else {
            col.saturating_add(1).saturating_add(word_len)
        };

        if !current.is_empty() && needed > width {
            out.push(DisplayLine {
                start: current_start,
                text: current,
            });
            current = word.to_string();
            current_start = word_start;
            col = word_len;
        } else if current.is_empty() {
            current = word.to_string();
            current_start = word_start;
            col = word_len;
        } else {
            current.push(' ');
            current.push_str(word);
            col = needed;
        }
    }

    if !current.is_empty() {
        out.push(DisplayLine {
            start: current_start,
            text: current,
        });
    }
}

pub fn wrapped_display_lines(text: &str, width: usize) -> Vec<DisplayLine> {
    if width == 0 {
        return vec![DisplayLine {
            start: 0,
            text: String::new(),
        }];
    }

    let mut out = Vec::new();
    for (line_start, line_end) in logical_line_ranges(text) {
        let line: String = text
            .chars()
            .skip(line_start)
            .take(line_end.saturating_sub(line_start))
            .collect();
        wrap_logical_line(line_start, &line, width, &mut out);
    }

    if out.is_empty() {
        out.push(DisplayLine {
            start: 0,
            text: String::new(),
        });
    }
    out
}

pub fn display_line_index(text: &str, cursor: usize, width: usize) -> usize {
    let wrapped = wrapped_display_lines(text, width);
    wrapped
        .iter()
        .enumerate()
        .rfind(|(_, line)| line.start <= cursor)
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

pub fn display_line_range_at(text: &str, display_line: usize, width: usize) -> (usize, usize) {
    let wrapped = wrapped_display_lines(text, width);
    let Some(line) = wrapped.get(display_line) else {
        let end = text.chars().count();
        return (end, end);
    };
    let start = line.start;
    let end = start + line.text.chars().count();
    (start, end)
}

pub fn select_all_range(text: &str) -> Option<(usize, usize)> {
    let end = text.chars().count();
    (end > 0).then_some((0, end))
}

pub fn selected_text(text: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    text.chars().skip(start).take(end - start).collect()
}

pub fn selection_range(anchor: usize, head: usize) -> (usize, usize) {
    if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    }
}

pub fn line_range_at(text: &str, line: usize) -> (usize, usize) {
    let start = cursor_from_line_col(text, line, 0);
    let end = cursor_from_line_col(text, line, line_len_at(text, line));
    (start, end)
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub fn word_range_at(text: &str, cursor: usize) -> Option<(usize, usize)> {
    let char_count = text.chars().count();
    if char_count == 0 {
        return None;
    }
    let cursor = cursor.min(char_count);

    let anchor = if cursor < char_count {
        let ch = text.chars().nth(cursor).unwrap();
        if is_word_char(ch) {
            cursor
        } else if cursor > 0 && is_word_char(text.chars().nth(cursor - 1).unwrap()) {
            cursor - 1
        } else {
            return None;
        }
    } else if is_word_char(text.chars().nth(cursor - 1).unwrap()) {
        cursor - 1
    } else {
        return None;
    };

    let mut start = anchor;
    while start > 0 && is_word_char(text.chars().nth(start - 1).unwrap()) {
        start -= 1;
    }

    let mut end = anchor + 1;
    while end < char_count && is_word_char(text.chars().nth(end).unwrap()) {
        end += 1;
    }

    (start < end).then_some((start, end))
}

pub fn delete_char_range(text: &mut String, start: usize, end: usize) {
    if start >= end {
        return;
    }
    let start_byte = text
        .char_indices()
        .nth(start)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    let end_byte = text
        .char_indices()
        .nth(end)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    text.replace_range(start_byte..end_byte, "");
}

pub fn cursor_from_line_col(text: &str, target_line: usize, target_col: usize) -> usize {
    let mut line = 0usize;
    let mut col = 0usize;
    let mut cursor = 0usize;
    for ch in text.chars() {
        if line == target_line && col >= target_col {
            return cursor;
        }
        if ch == '\n' {
            if line == target_line {
                return cursor;
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
        cursor += 1;
    }
    if line == target_line {
        cursor
    } else {
        text.chars().count()
    }
}

fn atomic_write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let data = serde_json::to_string_pretty(value)?;
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn cursor_line_col_tracks_newlines() {
        let text = "one\ntwo";
        assert_eq!(cursor_line_col(text, 0), (0, 0));
        assert_eq!(cursor_line_col(text, 3), (0, 3));
        assert_eq!(cursor_line_col(text, 4), (1, 0));
        assert_eq!(cursor_line_col(text, 7), (1, 3));
    }

    #[test]
    fn cursor_from_line_col_round_trips_with_cursor_line_col() {
        let text = "one\ntwo\n";
        for cursor in 0..=text.chars().count() {
            let (line, col) = cursor_line_col(text, cursor);
            assert_eq!(
                cursor_from_line_col(text, line, col),
                cursor,
                "cursor {cursor} -> ({line}, {col})"
            );
        }
    }

    #[test]
    fn line_len_at_matches_cursor_line_col() {
        let text = "hello\nworld";
        assert_eq!(line_len_at(text, 0), 5);
        assert_eq!(line_len_at(text, 1), 5);
        assert_eq!(line_len_at(text, 2), 0);
        assert_eq!(line_len_at("hello\n", 1), 0);
    }

    #[test]
    fn line_range_at_selects_line_without_newline() {
        let text = "hello\nworld";
        assert_eq!(line_range_at(text, 0), (0, 5));
        assert_eq!(line_range_at(text, 1), (6, 11));
    }

    #[test]
    fn word_range_at_selects_word_under_cursor() {
        let text = "hello world";
        assert_eq!(word_range_at(text, 0), Some((0, 5)));
        assert_eq!(word_range_at(text, 2), Some((0, 5)));
        assert_eq!(word_range_at(text, 6), Some((6, 11)));
        assert_eq!(word_range_at(text, 5), Some((0, 5)));
        assert_eq!(word_range_at(text, 11), Some((6, 11)));
    }

    #[test]
    fn word_range_at_returns_none_on_whitespace() {
        assert_eq!(word_range_at("   ", 1), None);
        assert_eq!(word_range_at("hello  world", 5), Some((0, 5)));
    }

    #[test]
    fn delete_char_range_removes_selected_span() {
        let mut text = "hello\nworld".to_string();
        delete_char_range(&mut text, 0, 5);
        assert_eq!(text, "\nworld");
    }

    #[test]
    fn select_all_range_covers_entire_note() {
        assert_eq!(select_all_range(""), None);
        assert_eq!(select_all_range("abc"), Some((0, 3)));
    }

    #[test]
    fn selection_range_normalizes_anchor_and_head() {
        assert_eq!(selection_range(2, 7), (2, 7));
        assert_eq!(selection_range(7, 2), (2, 7));
    }

    #[test]
    fn wrapped_display_lines_wraps_at_word_boundaries() {
        let lines = wrapped_display_lines("hello world foo", 8);
        assert_eq!(
            lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(),
            vec!["hello", "world", "foo"]
        );
    }

    #[test]
    fn wrapped_display_lines_breaks_long_logical_lines() {
        let lines = wrapped_display_lines("abcdef", 3);
        assert_eq!(
            lines,
            vec![
                DisplayLine {
                    start: 0,
                    text: "abc".into(),
                },
                DisplayLine {
                    start: 3,
                    text: "def".into(),
                },
            ]
        );
    }

    #[test]
    fn wrapped_display_lines_preserve_explicit_newlines() {
        let lines = wrapped_display_lines("hi\nthere", 10);
        assert_eq!(
            lines,
            vec![
                DisplayLine {
                    start: 0,
                    text: "hi".into(),
                },
                DisplayLine {
                    start: 3,
                    text: "there".into(),
                },
            ]
        );
    }

    #[test]
    fn display_line_index_follows_wrapped_rows() {
        let text = "abcdef";
        assert_eq!(display_line_index(text, 0, 3), 0);
        assert_eq!(display_line_index(text, 3, 3), 1);
        assert_eq!(display_line_index(text, 5, 3), 1);
    }

    #[test]
    fn save_writes_each_note_to_its_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let note_a = Note::new("Alpha", "one", true);
        let note_b = Note::new("Beta", "two", false);
        let notepad = SidebarNotepad {
            expanded: true,
            notes: vec![note_a.clone(), note_b.clone()],
            active_note_id: Some(note_a.id.clone()),
            ..SidebarNotepad::default()
        };
        save(&config, &notepad).unwrap();

        let loaded = load(&config);
        assert_eq!(loaded.notes.len(), 2);
        assert_eq!(loaded.notes[0].title, "Alpha");
        assert_eq!(loaded.notes[1].text, "two");
        assert!(config.sidebar_note_path(&note_a.id).exists());
        assert!(config.sidebar_note_path(&note_b.id).exists());
        assert!(config.sidebar_notepad_prefs_path().exists());
    }

    #[test]
    fn save_creates_backup_before_overwriting_non_empty_notes() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let note = Note::new("Note 1", "keep me", true);
        let note_id = note.id.clone();
        let original = SidebarNotepad {
            expanded: true,
            notes: vec![note.clone()],
            active_note_id: Some(note_id.clone()),
            ..SidebarNotepad::default()
        };
        save(&config, &original).unwrap();

        let cleared = SidebarNotepad {
            notes: vec![Note {
                text: String::new(),
                ..note
            }],
            ..original
        };
        save(&config, &cleared).unwrap();

        let backup_path = prefs_backup_path(&config.sidebar_notepad_prefs_path());
        let backup_prefs: NotepadPrefs =
            serde_json::from_str(&fs::read_to_string(backup_path).unwrap()).unwrap();
        assert_eq!(backup_prefs.note_order, vec![note_id.clone()]);
        let current = load_note_from_path(&config.sidebar_note_path(&note_id)).unwrap();
        assert!(current.text.is_empty());
    }

    #[test]
    fn load_returns_default_when_prefs_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        assert_eq!(load(&config), SidebarNotepad::default());
    }

    #[test]
    fn load_discovers_note_files_not_listed_in_note_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let listed = Note::new("Listed", "one", true);
        let orphan = Note::new("Orphan", "two", false);
        save_note_file(&config, &orphan).unwrap();
        atomic_write_json(
            &config.sidebar_notepad_prefs_path(),
            &NotepadPrefs {
                expanded: true,
                active_note_id: Some(listed.id.clone()),
                sessions_expanded: true,
                notes_list_expanded: false,
                welcome_seeded: false,
                note_order: vec![listed.id.clone()],
            },
        )
        .unwrap();
        save_note_file(&config, &listed).unwrap();

        let loaded = load(&config);
        assert_eq!(loaded.notes.len(), 2);
        assert!(loaded.notes.iter().any(|note| note.title == "Orphan"));
    }

    #[test]
    fn save_writes_note_files_before_prefs() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let note = Note::new("Alpha", "body", true);
        let notepad = SidebarNotepad {
            notes: vec![note.clone()],
            active_note_id: Some(note.id.clone()),
            ..SidebarNotepad::default()
        };
        save(&config, &notepad).unwrap();

        let note_data = fs::read_to_string(config.sidebar_note_path(&note.id)).unwrap();
        assert!(note_data.contains("body"));
        let prefs: NotepadPrefs =
            serde_json::from_str(&fs::read_to_string(config.sidebar_notepad_prefs_path()).unwrap())
                .unwrap();
        assert_eq!(prefs.note_order, vec![note.id]);
    }

    #[test]
    fn three_notes_round_trip_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let notes = vec![
            Note::new("One", "first", true),
            Note::new("Two", "second", false),
            Note::new("Three", "third", false),
        ];
        let notepad = SidebarNotepad {
            notes: notes.clone(),
            active_note_id: Some(notes[0].id.clone()),
            ..SidebarNotepad::default()
        };
        save(&config, &notepad).unwrap();

        for note in &notes {
            assert!(config.sidebar_note_path(&note.id).exists());
        }

        let loaded = load(&config);
        assert_eq!(loaded.notes.len(), 3);
        assert_eq!(loaded.notes[0].title, "One");
        assert_eq!(loaded.notes[1].text, "second");
        assert_eq!(loaded.notes[2].text, "third");
    }

    #[test]
    fn save_note_file_persists_body_without_full_save() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let note = Note::new("Draft", "typed while sidebar open", true);
        save_note_file(&config, &note).unwrap();

        let loaded = load_note_from_path(&config.sidebar_note_path(&note.id)).unwrap();
        assert_eq!(loaded.text, "typed while sidebar open");
        assert!(!config.sidebar_notepad_prefs_path().exists());
    }

    #[test]
    fn empty_in_memory_save_does_not_delete_orphan_note_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let orphan = Note::new("Orphan", "keep me", true);
        save_note_file(&config, &orphan).unwrap();
        atomic_write_json(
            &config.sidebar_notepad_prefs_path(),
            &NotepadPrefs {
                expanded: false,
                active_note_id: None,
                sessions_expanded: true,
                notes_list_expanded: false,
                welcome_seeded: false,
                note_order: vec![orphan.id.clone()],
            },
        )
        .unwrap();

        save(
            &config,
            &SidebarNotepad {
                notes: Vec::new(),
                ..SidebarNotepad::default()
            },
        )
        .unwrap();

        assert!(config.sidebar_note_path(&orphan.id).exists());
        let loaded = load(&config);
        assert_eq!(loaded.notes.len(), 1);
        assert_eq!(loaded.notes[0].text, "keep me");
    }

    #[test]
    fn delete_note_file_removes_single_note_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let note = Note::new("Gone", "bye", true);
        save_note_file(&config, &note).unwrap();
        delete_note_file(&config, &note.id).unwrap();
        assert!(!config.sidebar_note_path(&note.id).exists());
    }

    #[test]
    fn save_never_prunes_orphan_note_files() {
        let (_dir, config) = isolated_test_config();
        let keep = Note::new("Keep", "stay", true);
        let drop = Note::new("Drop", "gone", false);
        save(
            &config,
            &SidebarNotepad {
                notes: vec![keep.clone(), drop.clone()],
                active_note_id: Some(keep.id.clone()),
                ..SidebarNotepad::default()
            },
        )
        .unwrap();
        assert!(config.sidebar_note_path(&drop.id).exists());

        save(
            &config,
            &SidebarNotepad {
                notes: vec![keep.clone()],
                active_note_id: Some(keep.id.clone()),
                ..SidebarNotepad::default()
            },
        )
        .unwrap();
        assert!(
            config.sidebar_note_path(&drop.id).exists(),
            "save must not delete note files missing from in-memory state"
        );
        delete_note_file(&config, &drop.id).unwrap();
        assert!(!config.sidebar_note_path(&drop.id).exists());
        assert!(config.sidebar_note_path(&keep.id).exists());
    }

    #[test]
    fn isolated_test_config_does_not_target_live_state_dir() {
        let (_dir, config) = isolated_test_config();
        assert!(!is_live_notepad_config(&config));
    }

    #[test]
    #[should_panic(expected = "notepad persistence must use an isolated config")]
    fn save_panics_when_tests_target_live_notepad_dir() {
        let config = Config::default();
        save(&config, &SidebarNotepad::default()).unwrap();
    }
}
