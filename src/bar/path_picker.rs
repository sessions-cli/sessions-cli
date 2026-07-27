//! Shared directory path picker used by New Session (session path) and Automations
//! (project path).
//!
//! UX goals (aligned with shell completion + IDE folder pickers):
//! - Free-form typing always works (`~/…`, absolute, bare names under `$HOME`)
//! - Active sessions and recent usage surface first when browsing
//! - Type-to-filter with basename / segment matching
//! - Ghost-text + Tab completion
//! - Live validation and tilde-normalized display
//! - Paginated directory lists (no hard 24-item wall)

use crate::bar::directory_discovery::{
    expand_tilde, format_tilde_path, path_query_matches_label, DirectoryIndex,
};
use crate::bar::notepad;
use crate::config::Config;
use crate::model::{ClientCommand, Session};
use crate::session::workspace_usage::{load_rank_mode, WorkspaceRankMode, WorkspaceUsageStore};
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub const DISPLAY_INITIAL: usize = 48;
pub const DISPLAY_PAGE: usize = 48;
pub const MAX_CLOSED_SUGGESTIONS: usize = 24;
pub const HEADER_ROWS: u16 = 1;

pub const ACTIVE_SECTION: &str = "Active sessions";
pub const DIRECTORIES_SECTION: &str = "Directories";
pub const TYPED_SECTION: &str = "Path";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathChoice {
    pub label: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathPopupKind {
    Section,
    /// Path from an active session (or other “active” source).
    Active,
    /// Discovered / typed / recent directory path.
    Path,
}

#[derive(Debug, Clone)]
pub struct PathPopupEntry {
    pub kind: PathPopupKind,
    pub label: String,
    /// Absolute cwd when known; `None` for typed text that does not resolve yet.
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PathGhostHint {
    /// Grey suffix continuing the typed prefix (e.g. `sess` + `ions-cli`).
    Suffix(String),
    /// Grey full tilde path when the match is basename-only (e.g. `ses` → `~/projects/sessions-cli`).
    FullPath(String),
}

/// Path field shared by session path and automation project path.
#[derive(Clone)]
pub struct PathPickerState {
    pub directory_index: DirectoryIndex,
    pub usage: WorkspaceUsageStore,
    pub rank_mode: WorkspaceRankMode,
    pub active: Vec<PathChoice>,
    pub input: String,
    pub cursor: usize,
    pub user_editing: bool,
    pub confirmed: bool,
    pub highlight: usize,
    pub display_limit: usize,
    pub open: bool,
    home: String,
}

impl PathPickerState {
    pub fn load(config: &Config) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| config.home.display().to_string());
        let directory_index = DirectoryIndex::build(config);
        let usage = WorkspaceUsageStore::load(&config.home);
        let rank_mode = load_rank_mode(&config.home);
        let active = load_active_session_paths(config);
        let mut picker = Self {
            directory_index,
            usage,
            rank_mode,
            active,
            input: String::new(),
            cursor: 0,
            user_editing: false,
            confirmed: false,
            highlight: 0,
            display_limit: DISPLAY_INITIAL,
            open: false,
            home,
        };
        picker.reset_to_default();
        picker
    }

    pub fn refresh_sources(&mut self, config: &Config) {
        self.directory_index = DirectoryIndex::build(config);
        self.usage = WorkspaceUsageStore::load(&config.home);
        self.rank_mode = load_rank_mode(&config.home);
        self.active = load_active_session_paths(config);
    }

    /// Smart default: most-used active path, else top directory, else `~/`.
    pub fn reset_to_default(&mut self) {
        let default = self.default_path();
        self.input = default.label;
        self.cursor = self.input.chars().count();
        self.user_editing = false;
        self.confirmed = false;
        self.open = false;
        self.display_limit = DISPLAY_INITIAL;
        self.highlight = 0;
        // Keep absolute path in sync via commit if possible
        if let Ok((cwd, label)) = self.resolve() {
            let _ = cwd;
            self.input = label;
            self.cursor = self.input.chars().count();
        }
    }

    pub fn set_path(&mut self, path: &str) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            self.reset_to_default();
            return;
        }
        match expand_and_validate(trimmed) {
            Ok(cwd) => {
                self.input = format_tilde_path(&self.home, &cwd);
                self.cursor = self.input.chars().count();
                self.user_editing = false;
                self.confirmed = true;
            }
            Err(_) => {
                self.input = trimmed.to_string();
                self.cursor = self.input.chars().count();
                self.user_editing = true;
                self.confirmed = false;
            }
        }
        self.open = false;
        self.display_limit = DISPLAY_INITIAL;
        self.highlight = self.first_highlight_for_query();
    }

    pub fn default_path(&self) -> PathChoice {
        // Prefer ranked active sessions
        if let Some(best) = self.ranked_active().into_iter().next() {
            return best;
        }
        // Then usage history
        let empty = HashSet::new();
        if let Some((label, cwd)) = self
            .usage
            .closed_suggestions(&empty, self.rank_mode, 1)
            .into_iter()
            .next()
        {
            return PathChoice { label, cwd };
        }
        // Then first directory index entry (usually ~)
        if let Some((label, cwd)) = self.directory_index.browse_suggestions().into_iter().next() {
            return PathChoice { label, cwd };
        }
        PathChoice {
            label: "~/".into(),
            cwd: self.home.clone(),
        }
    }

    pub fn display_value(&self) -> String {
        if self.input.trim().is_empty() {
            return "Select a path…".into();
        }
        self.input.clone()
    }

    pub fn header_display(&self) -> String {
        if self.open {
            if !self.user_editing && !self.input.is_empty() {
                // Preview highlighted row while browsing
                let entries = self.build_popup();
                if let Some(entry) = entries.get(self.highlight) {
                    if entry.kind != PathPopupKind::Section {
                        return entry.label.clone();
                    }
                }
            }
            if self.user_editing {
                return self.input.clone();
            }
        }
        self.display_value()
    }

    pub fn is_typing(&self) -> bool {
        self.user_editing
    }

    pub fn path_input_error(&self) -> Option<String> {
        let input = self.input.trim();
        if input.is_empty() || !self.user_editing {
            return None;
        }
        if input == "~" || input == "~/" {
            return None;
        }
        expand_and_validate(input).err().map(|e| e.to_string())
    }

    pub fn ranked_active(&self) -> Vec<PathChoice> {
        let mut ranked: Vec<(PathChoice, (i64, i64))> = self
            .active
            .iter()
            .map(|p| {
                let score = self.usage.rank_score(&p.cwd, None, self.rank_mode);
                (p.clone(), score)
            })
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.label.cmp(&b.0.label)));
        ranked.into_iter().map(|(p, _)| p).collect()
    }

    pub fn directory_completions(&self) -> Vec<(String, String)> {
        let mut out = if self.user_editing {
            self.directory_index.completions_for_input(&self.input)
        } else {
            self.directory_index.browse_suggestions()
        };
        if self.input.trim().is_empty() || !self.user_editing {
            let active_cwds: HashSet<String> = self.active.iter().map(|p| p.cwd.clone()).collect();
            for (label, cwd) in
                self.usage
                    .closed_suggestions(&active_cwds, self.rank_mode, MAX_CLOSED_SUGGESTIONS)
            {
                if !out
                    .iter()
                    .any(|(existing, path)| existing == &label || path == &cwd)
                {
                    out.push((label, cwd));
                }
            }
            self.sort_by_usage(&mut out);
        }
        out
    }

    fn sort_by_usage(&self, completions: &mut [(String, String)]) {
        completions.sort_by(|left, right| {
            let left_score = self.usage.rank_score(&left.1, None, self.rank_mode);
            let right_score = self.usage.rank_score(&right.1, None, self.rank_mode);
            right_score
                .cmp(&left_score)
                .then_with(|| left.0.cmp(&right.0))
        });
    }

    pub fn build_popup(&self) -> Vec<PathPopupEntry> {
        let mut rows = Vec::new();
        let query = if self.user_editing {
            self.input.trim().to_lowercase()
        } else {
            String::new()
        };
        let typing = self.user_editing;

        if typing {
            rows.push(PathPopupEntry {
                kind: PathPopupKind::Section,
                label: TYPED_SECTION.into(),
                cwd: None,
            });
            let typed = self.input.trim();
            if !typed.is_empty() {
                let (label, cwd) = match expand_and_validate(typed) {
                    Ok(resolved) => {
                        let is_home =
                            resolved == self.home || resolved.trim_end_matches('/') == self.home;
                        let l = if is_home && (typed == "~" || typed == "~/") {
                            "~/".to_string()
                        } else {
                            format_tilde_path(&self.home, &resolved)
                        };
                        (l, Some(resolved))
                    }
                    Err(_) => (typed.to_string(), None),
                };
                rows.push(PathPopupEntry {
                    kind: PathPopupKind::Path,
                    label,
                    cwd,
                });
            } else {
                rows.push(PathPopupEntry {
                    kind: PathPopupKind::Section,
                    label: "  type ~/path or pictures".into(),
                    cwd: None,
                });
            }
        }

        rows.push(PathPopupEntry {
            kind: PathPopupKind::Section,
            label: ACTIVE_SECTION.into(),
            cwd: None,
        });

        let mut matched_active = 0usize;
        for path in self.ranked_active() {
            let matches = query.is_empty()
                || path_query_matches_label(&query, &path.label)
                || path.cwd.to_lowercase().contains(&query);
            if matches {
                matched_active += 1;
                rows.push(PathPopupEntry {
                    kind: PathPopupKind::Active,
                    label: path.label,
                    cwd: Some(path.cwd),
                });
            }
        }
        if matched_active == 0 {
            rows.push(PathPopupEntry {
                kind: PathPopupKind::Section,
                label: if typing {
                    "  (no match)".into()
                } else {
                    "  (none yet — type a path or pick below)".into()
                },
                cwd: None,
            });
        }

        let all_completions = self.directory_completions();
        let total = all_completions.len();
        let shown = self.display_limit.min(total);
        rows.push(PathPopupEntry {
            kind: PathPopupKind::Section,
            label: DIRECTORIES_SECTION.into(),
            cwd: None,
        });
        if total == 0 {
            rows.push(PathPopupEntry {
                kind: PathPopupKind::Section,
                label: if typing {
                    "  Tab to complete".into()
                } else {
                    "  type ~/path or pick below".into()
                },
                cwd: None,
            });
        } else {
            for (label, cwd) in all_completions.into_iter().take(shown) {
                // Skip duplicates already listed under Active
                if rows.iter().any(|r| r.cwd.as_deref() == Some(cwd.as_str())) {
                    continue;
                }
                rows.push(PathPopupEntry {
                    kind: PathPopupKind::Path,
                    label,
                    cwd: Some(cwd),
                });
            }
            if shown < total {
                rows.push(PathPopupEntry {
                    kind: PathPopupKind::Section,
                    label: format!("  ↓ {} more — press ↓ to load", total - shown),
                    cwd: None,
                });
            }
        }

        rows
    }

    pub fn selectable_indices(entries: &[PathPopupEntry]) -> Vec<usize> {
        entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                if entry.kind != PathPopupKind::Section {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn first_highlight_for_query(&self) -> usize {
        let entries = self.build_popup();
        let query_empty = self.input.trim().is_empty() || !self.user_editing;
        if query_empty {
            if let Some((idx, _)) = entries
                .iter()
                .enumerate()
                .find(|(_, e)| e.kind == PathPopupKind::Active)
            {
                return idx;
            }
        }
        entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.kind != PathPopupKind::Section)
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    pub fn maybe_expand_list(&mut self) {
        let total = self.directory_completions().len();
        if self.display_limit >= total {
            return;
        }
        let entries = self.build_popup();
        let last_dir_idx = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == PathPopupKind::Path && e.cwd.is_some())
            .map(|(idx, _)| idx)
            .next_back();
        if let Some(last_idx) = last_dir_idx {
            if self.highlight >= last_idx.saturating_sub(2) {
                self.display_limit = self.display_limit.saturating_add(DISPLAY_PAGE).min(total);
            }
        }
    }

    pub fn cycle(&mut self, delta: i32) {
        if delta > 0 {
            self.maybe_expand_list();
        }
        let entries = self.build_popup();
        let selectable = Self::selectable_indices(&entries);
        if selectable.is_empty() {
            return;
        }
        let current = selectable
            .iter()
            .position(|&idx| idx == self.highlight)
            .unwrap_or(0);
        if delta < 0 && current == 0 && !self.user_editing {
            self.begin_edit();
            return;
        }
        let next = (current as i32 + delta).rem_euclid(selectable.len() as i32) as usize;
        self.highlight = selectable[next];
    }

    pub fn begin_edit(&mut self) {
        if self.user_editing {
            return;
        }
        let display = self.header_display();
        if display == "Select a path…" {
            self.input = "~/".into();
        } else {
            self.input = display;
        }
        self.user_editing = true;
        self.cursor = self.input.chars().count();
        self.confirmed = false;
        self.display_limit = DISPLAY_INITIAL;
        self.highlight = self.first_highlight_for_query();
        self.open = true;
    }

    pub fn open_menu(&mut self) {
        self.open = true;
        if self.user_editing {
            self.highlight = self.first_highlight_for_query();
        } else {
            // Sync highlight to current committed path if possible
            let label = self.input.clone();
            let entries = self.build_popup();
            self.highlight = entries
                .iter()
                .enumerate()
                .find(|(_, e)| e.kind != PathPopupKind::Section && e.label == label)
                .map(|(i, _)| i)
                .unwrap_or_else(|| self.first_highlight_for_query());
        }
    }

    pub fn close_menu(&mut self) {
        self.open = false;
    }

    pub fn apply_highlight(&mut self) {
        let entries = self.build_popup();
        if let Some(entry) = entries.get(self.highlight) {
            match entry.kind {
                PathPopupKind::Active | PathPopupKind::Path => {
                    let from_edit = entry.cwd.is_none();
                    self.input = entry.label.clone();
                    self.cursor = self.input.chars().count();
                    self.user_editing = from_edit;
                    self.confirmed = false;
                    if from_edit {
                        self.highlight = self.first_highlight_for_query();
                    }
                }
                PathPopupKind::Section => {}
            }
        }
    }

    /// Confirm highlighted / typed path. Returns absolute cwd on success.
    ///
    /// On the live typed row: keep a resolvable typed path (do not let a stale
    /// highlight replace it). When typed text is not a directory yet, accept a
    /// unique basename/path completion so Enter works like the ghost hint.
    pub fn confirm(&mut self, show_errors: bool) -> Result<String, String> {
        if self.user_editing && self.on_typed_row() {
            let typed = self.input.trim().to_string();
            if !typed.is_empty() {
                if expand_and_validate(&typed).is_ok() {
                    return self.finish_confirm(show_errors);
                }
                if let Some(completion) = self.unique_enter_completion() {
                    self.input = completion_label_with_dir_slash(&completion);
                    self.cursor = self.input.chars().count();
                    self.user_editing = true;
                    self.confirmed = false;
                    self.highlight = self.first_highlight_for_query();
                    return self.finish_confirm(show_errors);
                }
            }
        }
        self.apply_highlight();
        self.finish_confirm(show_errors)
    }

    fn finish_confirm(&mut self, show_errors: bool) -> Result<String, String> {
        match self.resolve() {
            Ok((cwd, label)) => {
                self.input = label;
                self.cursor = self.input.chars().count();
                self.user_editing = false;
                self.confirmed = true;
                self.open = false;
                Ok(cwd)
            }
            Err(e) => {
                self.confirmed = false;
                let _ = show_errors;
                Err(e.to_string())
            }
        }
    }

    fn unique_enter_completion(&self) -> Option<(String, String)> {
        let input = self.input.trim();
        if input.is_empty() {
            return None;
        }
        let input_lower = input.to_lowercase();
        let candidates = self.path_completion_candidates();
        let exact: Vec<_> = candidates
            .iter()
            .filter(|(label, _)| label.to_lowercase() == input_lower)
            .cloned()
            .collect();
        if exact.len() == 1 {
            return Some(exact[0].clone());
        }
        let matching: Vec<_> = candidates
            .iter()
            .filter(|(label, cwd)| {
                let label_l = label.to_lowercase();
                label_l != input_lower
                    && (label_l.starts_with(&input_lower)
                        || path_query_matches_label(input, label)
                        || cwd.to_lowercase().contains(&input_lower))
            })
            .cloned()
            .collect();
        if matching.len() == 1 {
            return Some(matching[0].clone());
        }
        if let Some(completed) = longest_path_completion(input, &candidates) {
            let completed_lower = completed.to_lowercase();
            if completed_lower != input_lower {
                let resolved: Vec<_> = candidates
                    .iter()
                    .filter(|(label, _)| label.to_lowercase().starts_with(&completed_lower))
                    .cloned()
                    .collect();
                if resolved.len() == 1 {
                    return Some(resolved[0].clone());
                }
            }
        }
        None
    }

    pub fn confirm_on_blur(&mut self) {
        if self.confirmed {
            return;
        }
        let _ = self.confirm(false);
    }

    pub fn resolve(&self) -> Result<(String, String)> {
        let input = self.input.trim();
        if input.is_empty() {
            bail!("pick a path or type ~/path");
        }
        let cwd = expand_and_validate(input)?;
        let label = format_tilde_path(&self.home, &cwd);
        Ok((cwd, label))
    }

    /// Absolute cwd for save / launch.
    pub fn resolved_cwd(&self) -> Result<String> {
        self.resolve().map(|(cwd, _)| cwd)
    }

    pub fn record_usage(&self, config: &Config) {
        if let Ok((cwd, label)) = self.resolve() {
            let _ = WorkspaceUsageStore::record_focus_at(&config.home, &cwd, &label);
        }
    }

    // ── Typing / completion ──────────────────────────────────────────

    pub fn on_typed_row(&self) -> bool {
        if !self.user_editing {
            return false;
        }
        let entries = self.build_popup();
        entries.get(self.highlight).is_some_and(|entry| {
            entry.kind == PathPopupKind::Path && entry.label.trim() == self.input.trim()
        })
    }

    pub fn path_completion_candidates(&self) -> Vec<(String, String)> {
        let mut out = self.directory_completions();
        let query = self.input.trim();
        if query.is_empty() {
            return out;
        }
        for path in &self.active {
            if path_query_matches_label(query, &path.label)
                && !out
                    .iter()
                    .any(|(label, cwd)| label == &path.label || cwd == &path.cwd)
            {
                out.push((path.label.clone(), path.cwd.clone()));
            }
        }
        out
    }

    pub fn ghost_hint(&self) -> Option<PathGhostHint> {
        if !self.on_typed_row() {
            return None;
        }
        let input = self.input.trim();
        if input.is_empty() {
            return None;
        }
        let completions = self.path_completion_candidates();
        if completions.is_empty() {
            return None;
        }
        let input_lower = input.to_lowercase();
        if let Some(completed) = longest_path_completion(input, &completions) {
            let completed_lower = completed.to_lowercase();
            if completed_lower != input_lower {
                if completed_lower.starts_with(&input_lower) {
                    let suffix = completed
                        .chars()
                        .skip(input.chars().count())
                        .collect::<String>();
                    if !suffix.is_empty() {
                        return Some(PathGhostHint::Suffix(suffix));
                    }
                }
                return Some(PathGhostHint::FullPath(completed));
            }
        }
        let matching: Vec<_> = completions
            .iter()
            .filter(|(label, _)| path_query_matches_label(input, label))
            .collect();
        if matching.len() == 1 {
            let label = &matching[0].0;
            let label_lower = label.to_lowercase();
            if label_lower != input_lower {
                if label_lower.starts_with(&input_lower) {
                    let suffix = label
                        .chars()
                        .skip(input.chars().count())
                        .collect::<String>();
                    if !suffix.is_empty() {
                        return Some(PathGhostHint::Suffix(suffix));
                    }
                }
                return Some(PathGhostHint::FullPath(label.clone()));
            }
        }
        None
    }

    pub fn tab_complete(&mut self) -> bool {
        if self.try_filesystem_tab_completion() {
            return true;
        }
        let entries = self.build_popup();
        if let Some(entry) = entries.get(self.highlight) {
            match entry.kind {
                PathPopupKind::Active | PathPopupKind::Path => {
                    let from_edit = entry.cwd.is_none();
                    self.input = entry.label.clone();
                    self.cursor = self.input.chars().count();
                    self.user_editing = from_edit;
                    self.confirmed = false;
                    return true;
                }
                PathPopupKind::Section => {}
            }
        }
        false
    }

    fn try_filesystem_tab_completion(&mut self) -> bool {
        if !self.user_editing || !self.on_typed_row() {
            return false;
        }
        let input = self.input.trim();
        if input.is_empty() {
            return false;
        }
        let completions = self.path_completion_candidates();
        if completions.is_empty() {
            return false;
        }
        let input_lower = input.to_lowercase();
        let exact: Vec<_> = completions
            .iter()
            .filter(|(label, _)| label.to_lowercase() == input_lower)
            .collect();
        if exact.len() == 1 {
            self.apply_completion(exact[0]);
            return true;
        }
        if completions.len() == 1 {
            self.apply_completion(&completions[0]);
            return true;
        }
        if let Some(completed) = longest_path_completion(&self.input, &completions) {
            let completed_lower = completed.to_lowercase();
            let matching: Vec<_> = completions
                .iter()
                .filter(|(label, _)| label.to_lowercase().starts_with(&completed_lower))
                .collect();
            if matching.len() == 1 {
                self.apply_completion(matching[0]);
            } else {
                self.cursor = completed.chars().count();
                self.input = completed;
                self.user_editing = true;
                self.confirmed = false;
                self.display_limit = DISPLAY_INITIAL;
                self.highlight = self.first_highlight_for_query();
            }
            return true;
        }
        false
    }

    fn apply_completion(&mut self, completion: &(String, String)) {
        self.input = completion_label_with_dir_slash(completion);
        self.cursor = self.input.chars().count();
        self.user_editing = true;
        self.confirmed = false;
        self.display_limit = DISPLAY_INITIAL;
        self.highlight = self.first_highlight_for_query();
    }

    pub fn insert_char(&mut self, ch: char) {
        if !self.user_editing {
            self.input.clear();
            self.user_editing = true;
            self.cursor = 0;
        }
        if self.input.trim() == "~" {
            self.input = "~/".to_string();
            self.cursor = 2;
        }
        self.sync_cursor();
        let cursor = self.cursor;
        let byte_idx = self
            .input
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.input.len());
        self.input.insert(byte_idx, ch);
        self.cursor = cursor + 1;
        self.confirmed = false;
        self.open = true;
        self.display_limit = DISPLAY_INITIAL;
        self.highlight = self.first_highlight_for_query();
    }

    pub fn backspace(&mut self) {
        if !self.user_editing {
            self.begin_edit();
        }
        self.sync_cursor();
        if self.cursor == 0 {
            return;
        }
        let cursor = self.cursor;
        let byte_idx = self
            .input
            .char_indices()
            .nth(cursor - 1)
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let next_byte = self
            .input
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.input.len());
        self.input.replace_range(byte_idx..next_byte, "");
        self.cursor = cursor - 1;
        if self.input.is_empty() {
            self.user_editing = false;
            self.cursor = 0;
            self.highlight = self.first_highlight_for_query();
        } else {
            self.display_limit = DISPLAY_INITIAL;
            self.highlight = self.first_highlight_for_query();
        }
        self.confirmed = false;
        self.open = true;
    }

    pub fn forward_delete(&mut self) {
        if !self.user_editing {
            return;
        }
        self.sync_cursor();
        let len = self.input.chars().count();
        if self.cursor >= len {
            return;
        }
        let cursor = self.cursor;
        let byte_idx = self
            .input
            .char_indices()
            .nth(cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.input.len());
        let next_byte = self
            .input
            .char_indices()
            .nth(cursor + 1)
            .map(|(idx, _)| idx)
            .unwrap_or(self.input.len());
        self.input.replace_range(byte_idx..next_byte, "");
        if self.input.is_empty() {
            self.user_editing = false;
            self.cursor = 0;
            self.highlight = self.first_highlight_for_query();
        } else {
            self.display_limit = DISPLAY_INITIAL;
            self.highlight = self.first_highlight_for_query();
        }
        self.confirmed = false;
        self.open = true;
    }

    pub fn apply_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.input = text.to_string();
        self.cursor = text.chars().count();
        self.user_editing = true;
        self.confirmed = false;
        self.open = true;
        self.display_limit = DISPLAY_INITIAL;
        self.highlight = self.first_highlight_for_query();
    }

    pub fn move_cursor(&mut self, delta: i32) {
        if !self.user_editing {
            self.begin_edit();
        }
        let len = self.input.chars().count();
        let cursor = self.cursor as i32;
        self.cursor = (cursor + delta).clamp(0, len as i32) as usize;
    }

    fn sync_cursor(&mut self) {
        self.cursor = notepad::clamp_cursor(&self.input, self.cursor);
    }

    pub fn row_label(entry: &PathPopupEntry) -> String {
        match entry.kind {
            PathPopupKind::Section => entry.label.clone(),
            PathPopupKind::Active | PathPopupKind::Path => {
                let mut label = entry.label.clone();
                // Strip stray single-letter prefixes (agent shorthand artifacts).
                if label.len() > 2 {
                    let bytes = label.as_bytes();
                    if bytes[1] == b' '
                        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'O' || bytes[0] == b'G')
                    {
                        label = label[2..].to_string();
                    }
                }
                if matches!(entry.kind, PathPopupKind::Path) && entry.cwd.is_none() {
                    format!("> {label}")
                } else {
                    label
                }
            }
        }
    }
}

// ── Shared path resolution helpers ───────────────────────────────────

/// Expand `~`, bare names under `$HOME`, and validate the result is a directory.
pub fn expand_and_validate(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("enter a path");
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    let expanded =
        expand_tilde(trimmed, &home).ok_or_else(|| anyhow::anyhow!("could not expand path"))?;
    let path = Path::new(&expanded);
    if !path.is_dir() {
        if let Some(parent) = path.parent() {
            if !parent.exists() || !parent.is_dir() {
                bail!("parent directory does not exist: {trimmed}");
            }
        }
        bail!("not a directory: {trimmed}");
    }
    std::fs::canonicalize(path)
        .map(|resolved| resolved.display().to_string())
        .or(Ok(expanded))
}

/// Expand path for storage (less strict — may not exist yet checks deferred).
/// Uses the same tilde / bare-name rules as the picker.
pub fn expand_path_for_storage(path: &str) -> Result<String> {
    let path = path.trim();
    if path.is_empty() {
        bail!("cwd is empty");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| crate::paths::home().display().to_string());
    let expanded =
        expand_tilde(path, &home).ok_or_else(|| anyhow::anyhow!("could not expand path"))?;
    let abs = Path::new(&expanded);
    let abs = if abs.is_absolute() {
        abs.to_path_buf()
    } else {
        std::env::current_dir()?.join(abs)
    };
    Ok(abs.to_string_lossy().to_string())
}

pub fn completion_label_with_dir_slash(completion: &(String, String)) -> String {
    let mut label = completion.0.clone();
    if Path::new(&completion.1).is_dir() && !label.ends_with('/') {
        label.push('/');
    }
    label
}

pub fn longest_path_completion(input: &str, completions: &[(String, String)]) -> Option<String> {
    if completions.is_empty() {
        return None;
    }
    if completions.len() == 1 {
        return Some(completions[0].0.clone());
    }
    let trimmed = input.trim();
    let labels: Vec<&str> = completions
        .iter()
        .map(|(label, _)| label.as_str())
        .collect();
    let mut prefix = labels[0].to_string();
    for label in labels.iter().skip(1) {
        let mut next = String::new();
        for (left, right) in prefix.chars().zip(label.chars()) {
            if left == right {
                next.push(left);
            } else {
                break;
            }
        }
        prefix = next;
        if prefix.is_empty() {
            break;
        }
    }
    let trimmed_lower = trimmed.to_lowercase();
    let prefix_lower = prefix.to_lowercase();
    if prefix.len() > trimmed.len() && prefix_lower.starts_with(&trimmed_lower) {
        return Some(prefix);
    }
    // Bare input (e.g. "pic") against labels like "~/Pictures".
    if !trimmed.contains('/') && !trimmed.starts_with('~') && !trimmed.starts_with('/') {
        let basenames: Vec<&str> = labels
            .iter()
            .filter_map(|l| l.rsplit_once('/').map(|(_, b)| b).or(Some(*l)))
            .collect();
        if !basenames.is_empty() {
            let mut bprefix = basenames[0].to_string();
            for b in basenames.iter().skip(1) {
                let mut n = String::new();
                for (a, c) in bprefix.chars().zip(b.chars()) {
                    if a == c {
                        n.push(a);
                    } else {
                        break;
                    }
                }
                bprefix = n;
                if bprefix.is_empty() {
                    break;
                }
            }
            let blen = bprefix.len();
            let b_lower = bprefix.to_lowercase();
            if blen > trimmed.len() && b_lower.starts_with(&trimmed_lower) {
                if let Some((dir_part, _)) = labels[0].rsplit_once('/') {
                    return Some(format!("{dir_part}/{bprefix}"));
                } else {
                    return Some(bprefix);
                }
            }
        }
    }
    if prefix == trimmed && completions.len() == 1 {
        Some(completions[0].0.clone())
    } else {
        None
    }
}

pub fn load_active_session_paths(config: &Config) -> Vec<PathChoice> {
    let Ok(mut stream) = UnixStream::connect(&config.socket_path) else {
        return Vec::new();
    };
    let Ok(line) = serde_json::to_string(&ClientCommand::List) else {
        return Vec::new();
    };
    if stream.write_all((line + "\n").as_bytes()).is_err() {
        return Vec::new();
    }
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    if reader.read_line(&mut response).is_err() {
        return Vec::new();
    }
    let Ok(sessions) = serde_json::from_str::<Vec<Session>>(response.trim()) else {
        return Vec::new();
    };
    let mut by_cwd = std::collections::BTreeMap::<String, String>::new();
    for session in sessions {
        if session.cwd.is_empty() {
            continue;
        }
        let label = if session.cwd_label.is_empty() {
            let home = std::env::var("HOME").unwrap_or_default();
            format_tilde_path(&home, &session.cwd)
        } else {
            session.cwd_label.clone()
        };
        by_cwd.entry(session.cwd).or_insert(label);
    }
    by_cwd
        .into_iter()
        .map(|(cwd, label)| PathChoice { label, cwd })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_completion_extends_common_prefix() {
        let completions = vec![
            ("~/projects/sessions-cli".into(), "/tmp".into()),
            ("~/projects/sessions-cli-old".into(), "/tmp2".into()),
        ];
        let completed = longest_path_completion("~/projects/sess", &completions);
        assert_eq!(completed.as_deref(), Some("~/projects/sessions-cli"));
    }

    #[test]
    fn expand_and_validate_home() {
        let home = std::env::var("HOME").expect("HOME");
        let expanded = expand_and_validate("~").expect("~");
        assert_eq!(
            Path::new(&expanded)
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            Path::new(&home)
                .canonicalize()
                .unwrap()
                .display()
                .to_string()
        );
    }

    #[test]
    fn popup_has_active_and_directories_sections() {
        let mut picker = PathPickerState {
            directory_index: DirectoryIndex::from_test_entries(
                "/home/test",
                vec![("~/projects/foo".into(), "/home/test/projects/foo".into())],
            ),
            usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            active: vec![PathChoice {
                label: "~/projects/active".into(),
                cwd: "/home/test/projects/active".into(),
            }],
            input: String::new(),
            cursor: 0,
            user_editing: false,
            confirmed: false,
            highlight: 0,
            display_limit: DISPLAY_INITIAL,
            open: true,
            home: "/home/test".into(),
        };
        picker.input = "~/projects/active".into();
        let entries = picker.build_popup();
        assert!(entries.iter().any(|e| e.label == ACTIVE_SECTION));
        assert!(entries.iter().any(|e| e.label == DIRECTORIES_SECTION));
        assert!(entries
            .iter()
            .any(|e| e.kind == PathPopupKind::Active && e.label == "~/projects/active"));
    }

    #[test]
    fn typing_filters_active_by_basename() {
        let picker = PathPickerState {
            directory_index: DirectoryIndex::from_test_entries("/home/test", vec![]),
            usage: WorkspaceUsageStore::default(),
            rank_mode: WorkspaceRankMode::MostUsed,
            active: vec![PathChoice {
                label: "~/projects/sessions-cli".into(),
                cwd: "/home/test/projects/sessions-cli".into(),
            }],
            input: "ses".into(),
            cursor: 3,
            user_editing: true,
            confirmed: false,
            highlight: 0,
            display_limit: DISPLAY_INITIAL,
            open: true,
            home: "/home/test".into(),
        };
        let entries = picker.build_popup();
        assert!(entries
            .iter()
            .any(|e| { e.kind == PathPopupKind::Active && e.label == "~/projects/sessions-cli" }));
    }
}
