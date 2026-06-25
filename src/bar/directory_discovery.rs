use crate::config::Config;
use crate::session::workspace::WorkspaceCatalog;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_SCAN_DEPTH: usize = 4;
/// Safety valve for runaway scans — not a UI page size (the picker paginates on demand).
const MAX_DISCOVERED_DIRECTORIES: usize = 4096;
const MAX_FS_COMPLETIONS: usize = 8;

const REPO_MARKERS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "Gemfile",
    "composer.json",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "mix.exs",
    "deno.json",
    "deno.jsonc",
];

const SKIP_DIR_NAMES: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "venv",
    ".venv",
    "__pycache__",
    ".next",
    ".turbo",
    ".cache",
    "coverage",
    ".idea",
    ".vscode",
    "Pods",
    "DerivedData",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DirectorySource {
    WorkspaceConfig = 0,
    GitRoot = 1,
    RepoMarker = 2,
    ScanRoot = 3,
}

#[derive(Debug, Clone)]
struct DiscoveredDirectory {
    label: String,
    cwd: String,
    source: DirectorySource,
}

#[derive(Debug, Clone)]
pub struct DirectoryIndex {
    home: String,
    directories: Vec<DiscoveredDirectory>,
}

impl DirectoryIndex {
    pub fn build(config: &Config) -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut index = Self {
            home: home.clone(),
            directories: Vec::new(),
        };
        if home.is_empty() {
            return index;
        }
        index.discover(config);
        index
    }

    pub fn browse_suggestions(&self) -> Vec<(String, String)> {
        self.suggestions_for_query("")
    }

    pub fn suggestions_for_query(&self, query: &str) -> Vec<(String, String)> {
        let query = query.trim().to_lowercase();
        let mut matches: Vec<&DiscoveredDirectory> = self
            .directories
            .iter()
            .filter(|dir| query.is_empty() || directory_matches_query(dir, &query))
            .collect();
        matches.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.label.cmp(&right.label))
        });
        let mut out = Vec::new();
        if query.is_empty() || "~".starts_with(&query) || "home".contains(&query) {
            out.push(("~".into(), self.home.clone()));
        }
        for dir in matches {
            if out.iter().any(|(label, _)| label == &dir.label) {
                continue;
            }
            out.push((dir.label.clone(), dir.cwd.clone()));
        }
        out
    }

    pub fn completions_for_input(&self, input: &str) -> Vec<(String, String)> {
        let trimmed = input.trim();
        if trimmed.is_empty() || trimmed == "~" {
            return self.browse_suggestions();
        }

        let mut out = self.suggestions_for_query(trimmed);
        for (label, cwd) in filesystem_completions(trimmed, &self.home) {
            if !out.iter().any(|(existing, _)| existing == &label) {
                out.push((label, cwd));
            }
        }
        if out.len() > MAX_DISCOVERED_DIRECTORIES {
            out.truncate(MAX_DISCOVERED_DIRECTORIES);
        }
        out
    }

    fn discover(&mut self, config: &Config) {
        let mut seen = HashSet::new();
        let mut ranked: HashMap<String, (String, DirectorySource)> = HashMap::new();

        let catalog = WorkspaceCatalog::load(&config.workspaces_path);
        for entry in &catalog.entries {
            if let Some((label, cwd)) = canonical_directory_path(&entry.cwd, &self.home) {
                ranked
                    .entry(cwd)
                    .and_modify(|(_, source)| *source = DirectorySource::WorkspaceConfig)
                    .or_insert((label, DirectorySource::WorkspaceConfig));
            }
        }

        for root in scan_roots(&self.home) {
            let source = DirectorySource::ScanRoot;
            let _ = source;
            let absolute = expand_tilde(&root, &self.home).unwrap_or(root);
            scan_directory(Path::new(&absolute), 0, &self.home, &mut ranked, &mut seen);
        }

        self.directories = ranked
            .into_iter()
            .map(|(cwd, (label, source))| DiscoveredDirectory { label, cwd, source })
            .collect();
        self.directories.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.label.cmp(&right.label))
        });
        if self.directories.len() > MAX_DISCOVERED_DIRECTORIES {
            self.directories.truncate(MAX_DISCOVERED_DIRECTORIES);
        }
    }
}

/// Match a path picker query against a tilde display label.
///
/// Supports full-path substring search, any path segment prefix, and basename-only
/// search for bare names (`sess`) or single-segment tilde queries (`~/sess`).
pub fn path_query_matches_label(query: &str, label: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let label_lower = label.to_lowercase();
    if label_lower.contains(&query) {
        return true;
    }
    if label_lower
        .split('/')
        .any(|segment| segment.starts_with(&query))
    {
        return true;
    }
    if let Some(basename_query) = basename_query_str(&query) {
        if let Some(last) = label_lower.rsplit('/').next() {
            return last.starts_with(basename_query);
        }
    }
    false
}

fn basename_query_str(query: &str) -> Option<&str> {
    if let Some(rest) = query.strip_prefix("~/") {
        if !rest.contains('/') {
            return Some(rest);
        }
    } else if !query.contains('/') {
        return Some(query);
    }
    None
}

fn directory_matches_query(dir: &DiscoveredDirectory, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    path_query_matches_label(query, &dir.label)
        || dir
            .cwd
            .to_lowercase()
            .contains(&query.trim().to_lowercase())
}

fn scan_roots(home: &str) -> Vec<String> {
    let mut roots = Vec::new();
    // Support both old and new env var names for user-specified scan roots.
    // No built-in bias towards any particular folder name (e.g. no auto ~/projects).
    // Users can set SESSIONS_DIRECTORY_ROOTS (or legacy SESSIONS_PROJECT_ROOTS) to
    // a : separated list of directories to scan for git roots / repo markers.
    // Directory discovery for arbitrary locations is primarily via typing (FS
    // completions under ~ for bare names like "pictures").
    for var in ["SESSIONS_DIRECTORY_ROOTS", "SESSIONS_PROJECT_ROOTS"] {
        if let Ok(extra) = std::env::var(var) {
            for part in extra.split(':') {
                let trimmed = part.trim();
                if !trimmed.is_empty() {
                    roots.push(trimmed.to_string());
                }
            }
        }
    }
    roots
        .into_iter()
        .filter(|root| {
            expand_tilde(root, home)
                .map(|path| Path::new(&path).is_dir())
                .unwrap_or(false)
        })
        .collect()
}

fn scan_directory(
    dir: &Path,
    depth: usize,
    home: &str,
    ranked: &mut HashMap<String, (String, DirectorySource)>,
    seen: &mut HashSet<PathBuf>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let canonical = match dir.canonicalize() {
        Ok(path) => path,
        Err(_) => dir.to_path_buf(),
    };
    if !seen.insert(canonical.clone()) {
        return;
    }

    let is_git_root = canonical.join(".git").is_dir();
    let has_marker = REPO_MARKERS
        .iter()
        .any(|marker| canonical.join(marker).is_file());
    if is_git_root || has_marker {
        if let Some((label, cwd)) = canonical_directory_path(&canonical.display().to_string(), home) {
            let source = if is_git_root {
                DirectorySource::GitRoot
            } else {
                DirectorySource::RepoMarker
            };
            ranked
                .entry(cwd)
                .and_modify(|(_, existing)| {
                    if source < *existing {
                        *existing = source;
                    }
                })
                .or_insert((label, source));
        }
        if is_git_root {
            return;
        }
    }

    if depth == MAX_SCAN_DEPTH {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = read_dir
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !should_skip_dir(path))
        .collect();
    children.sort();
    for child in children {
        scan_directory(&child, depth + 1, home, ranked, seen);
    }
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with('.') && name != ".workspace" || SKIP_DIR_NAMES.contains(&name))
        .unwrap_or(true)
}

fn canonical_directory_path(path: &str, home: &str) -> Option<(String, String)> {
    let expanded = expand_tilde(path, home)?;
    let canonical = Path::new(&expanded)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&expanded));
    if !canonical.is_dir() {
        return None;
    }
    let cwd = canonical.display().to_string();
    Some((format_tilde_path(home, &cwd), cwd))
}

fn filesystem_completions(input: &str, home: &str) -> Vec<(String, String)> {
    let Some((parent, prefix)) = completion_parent_and_prefix(input, home) else {
        return Vec::new();
    };
    let Ok(read_dir) = std::fs::read_dir(&parent) else {
        return Vec::new();
    };
    let prefix_lower = prefix.to_lowercase();
    let input_ends_with_slash = input.trim_end().ends_with('/');
    let mut matches: Vec<PathBuf> = read_dir
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            if prefix_lower.is_empty() {
                return true;
            }
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_lowercase().starts_with(&prefix_lower))
                .unwrap_or(false)
        })
        .collect();
    matches.sort();
    matches
        .into_iter()
        .take(MAX_FS_COMPLETIONS)
        .map(|path| {
            let cwd = path.display().to_string();
            let label = if input_ends_with_slash || prefix.is_empty() {
                format_tilde_path(home, &cwd)
            } else {
                completion_display_label(input, &path, home)
            };
            (label, cwd)
        })
        .collect()
}

fn completion_parent_and_prefix(input: &str, home: &str) -> Option<(PathBuf, String)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded = expand_tilde(trimmed, home)?;
    let path = Path::new(&expanded);
    if trimmed.ends_with('/') {
        return Some((path.to_path_buf(), String::new()));
    }
    let prefix = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    let parent = path.parent().unwrap_or_else(|| Path::new("/"));
    let parent = if parent.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        parent.to_path_buf()
    };
    Some((parent, prefix))
}

fn completion_display_label(_input: &str, matched: &Path, home: &str) -> String {
    // Always format matched path with ~ for home subdirs. This ensures that even
    // when user types bare "pictures/..." or "p" the previewed entries show as
    // "~/Pictures/..." or "~/Pictures" (with on-disk casing).
    format_tilde_path(home, &matched.display().to_string())
}

pub fn expand_tilde(path: &str, home: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "~" {
        return Some(home.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return Some(format!("{home}/{rest}"));
    }
    if trimmed.starts_with('~') {
        return None;
    }
    if trimmed.starts_with('/') {
        return Some(trimmed.to_string());
    }
    // Bare names (e.g. "Pictures", "pictures", "dev/myapp") resolve under $HOME for
    // effortless typing in the new session "Directories" picker. Case folding for
    // preview comes from FS; resolution on macOS is case-insensitive.
    let rest = trimmed.strip_prefix("./").unwrap_or(trimmed);
    if rest.is_empty() || rest == "." {
        return Some(home.to_string());
    }
    Some(format!("{home}/{}", rest))
}

pub fn format_tilde_path(home: &str, path: &str) -> String {
    if path == home {
        return "~".into();
    }
    if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
        return format!("~/{rest}");
    }
    path.to_string()
}

#[cfg(test)]
impl DirectoryIndex {
    pub fn from_test_entries(home: impl Into<String>, entries: Vec<(String, String)>) -> Self {
        Self {
            home: home.into(),
            directories: entries
                .into_iter()
                .map(|(label, cwd)| DiscoveredDirectory {
                    label,
                    cwd,
                    source: DirectorySource::ScanRoot,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn scan_directory_finds_git_roots_and_project_markers() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_string_lossy().to_string();
        let projects = temp.path().join("projects");
        fs::create_dir_all(projects.join("alpha").join(".git")).unwrap();
        fs::create_dir_all(projects.join("group").join("beta").join(".git")).unwrap();
        write_file(
            &projects.join("mono").join("frontend").join("package.json"),
            "{}",
        );

        let mut ranked = HashMap::new();
        let mut seen = HashSet::new();
        scan_directory(&projects, 0, &home, &mut ranked, &mut seen);
        let labels: Vec<String> = ranked.values().map(|(label, _)| label.clone()).collect();
        assert!(labels.iter().any(|label| label.contains("alpha")));
        assert!(labels.iter().any(|label| label.contains("beta")));
        assert!(labels.iter().any(|label| label.contains("mono/frontend")));
    }

    #[test]
    fn query_matches_path_segments() {
        let dir = DiscoveredDirectory {
            label: "~/projects/superflip/superflip-frontend".into(),
            cwd: "/home/test/projects/superflip/superflip-frontend".into(),
            source: DirectorySource::WorkspaceConfig,
        };
        assert!(directory_matches_query(&dir, "super"));
        assert!(directory_matches_query(&dir, "frontend"));
        assert!(!directory_matches_query(&dir, "backend"));
    }

    #[test]
    fn query_matches_basename_only() {
        assert!(path_query_matches_label("ses", "~/projects/sessions-cli"));
        assert!(path_query_matches_label("~/ses", "~/projects/sessions-cli"));
        assert!(path_query_matches_label("sessions-cli", "~/projects/sessions-cli"));
        assert!(!path_query_matches_label("cloud", "~/projects/sessions-cli"));
    }

    #[test]
    fn suggestions_put_home_first() {
        let index = DirectoryIndex {
            home: "/home/test".into(),
            directories: vec![DiscoveredDirectory {
                label: "~/projects/foo".into(),
                cwd: "/home/test/projects/foo".into(),
                source: DirectorySource::GitRoot,
            }],
        };
        let suggestions = index.browse_suggestions();
        assert_eq!(
            suggestions.first().map(|(label, _)| label.as_str()),
            Some("~")
        );
        assert!(suggestions
            .iter()
            .any(|(label, _)| label == "~/projects/foo"));
    }

    #[test]
    fn discover_includes_workspace_config_paths() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let configured = home.join("configured-repo");
        fs::create_dir_all(&configured).unwrap();
        let workspaces = temp.path().join("workspaces.toml");
        write_file(
            &workspaces,
            format!(
                r#"
[[workspace]]
title = "configured"
cwd = "{}"
command = "grok"
"#,
                configured.display()
            )
            .as_str(),
        );

        let mut config = Config::default();
        config.home = home.clone();
        config.workspaces_path = workspaces;

        let index = DirectoryIndex::build(&config);
        assert!(index
            .browse_suggestions()
            .iter()
            .any(|(label, _)| label.contains("configured-repo")));
    }

    #[test]
    fn discover_keeps_more_than_legacy_cap() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_string_lossy().to_string();
        let root = temp.path().join("scan");
        fs::create_dir_all(&root).unwrap();
        for i in 0..150 {
            fs::create_dir_all(root.join(format!("repo-{i}")).join(".git")).unwrap();
        }
        std::env::set_var("HOME", &home);
        std::env::set_var("SESSIONS_DIRECTORY_ROOTS", root.display().to_string());
        let index = DirectoryIndex::build(&Config::default());
        std::env::remove_var("HOME");
        std::env::remove_var("SESSIONS_DIRECTORY_ROOTS");
        assert!(
            index.browse_suggestions().len() > 128,
            "expected more than the old 128 cap, got {}",
            index.browse_suggestions().len()
        );
    }

    #[test]
    fn completions_merge_project_search_and_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_string_lossy().to_string();
        let projects = temp.path().join("projects");
        fs::create_dir_all(projects.join("sessions-cli").join(".git")).unwrap();
        fs::create_dir_all(projects.join("sessions-cloud").join(".git")).unwrap();

        let index = DirectoryIndex {
            home: home.clone(),
            directories: vec![
                DiscoveredDirectory {
                    label: "~/projects/sessions-cli".into(),
                    cwd: projects.join("sessions-cli").display().to_string(),
                    source: DirectorySource::GitRoot,
                },
                DiscoveredDirectory {
                    label: "~/projects/sessions-cloud".into(),
                    cwd: projects.join("sessions-cloud").display().to_string(),
                    source: DirectorySource::GitRoot,
                },
            ],
        };
        let matches = index.completions_for_input("~/projects/sess");
        assert!(matches
            .iter()
            .any(|(label, _)| label.contains("sessions-cli")));
    }
}
