use crate::config::Config;
use crate::session::workspace::WorkspaceCatalog;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_SCAN_DEPTH: usize = 5;
/// Safety valve for runaway scans — not a UI page size (the picker paginates on demand).
const MAX_DISCOVERED_DIRECTORIES: usize = 4096;
/// Children listed when the user types an explicit parent (`~/side-projects/…`).
/// Was 8 — hubs with 9+ category folders (e.g. side-projects) silently dropped the rest.
const MAX_FS_COMPLETIONS: usize = 64;
/// Index scan-root hubs and their direct children so category folders like
/// `~/side-projects/dev-tools` are searchable even without a `.git` of their own.
const INDEX_INTERMEDIATE_MAX_DEPTH: usize = 1;

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
    // Archived / bulk trees — keep project search focused on active work.
    "_archive",
    "archive",
];

/// Common home-level project collection folder names (portable, not machine-specific).
const DEFAULT_PROJECT_ROOT_NAMES: &[&str] =
    &["side-projects", "projects", "Developer", "dev", "code"];

/// Home children we never treat as project hubs (OS / media / bulk dumps).
const HOME_HUB_SKIP: &[&str] = &[
    "Library",
    "Applications",
    "Movies",
    "Music",
    "Pictures",
    "Downloads",
    "Desktop",
    "Documents",
    "Public",
    "Videos",
    "Backups",
    "bin",
    "tmp",
    "opt",
    "go",
    "snap",
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
        // Prefix match only — `"home".contains("e")` used to inject ~ for almost any letter.
        if query.is_empty()
            || "~".starts_with(&query)
            || "home".starts_with(&query)
            || query == "~/"
        {
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
    // 1) Common collection folders under $HOME (portable names only).
    for name in DEFAULT_PROJECT_ROOT_NAMES {
        roots.push(format!("{home}/{name}"));
    }
    // 2) Other home-level "hubs": dirs with 2+ direct project children (git/markers).
    //    Picks up monorepo workspaces without hardcoding private folder names.
    roots.extend(discover_home_project_hubs(home));
    // 3) Optional extras: SESSIONS_DIRECTORY_ROOTS (or legacy SESSIONS_PROJECT_ROOTS),
    //    colon-separated list of additional scan roots (tilde ok).
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
    let mut seen = HashSet::new();
    roots
        .into_iter()
        .filter_map(|root| {
            let absolute = expand_tilde(&root, home).unwrap_or(root);
            if !Path::new(&absolute).is_dir() {
                return None;
            }
            if !seen.insert(absolute.clone()) {
                return None;
            }
            Some(absolute)
        })
        .collect()
}

/// Home-level dirs that look like primary project collections (cheap, depth-1 check).
fn discover_home_project_hubs(home: &str) -> Vec<String> {
    let home_path = Path::new(home);
    let Ok(read_dir) = std::fs::read_dir(home_path) else {
        return Vec::new();
    };
    let mut hubs = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || HOME_HUB_SKIP.contains(&name) {
            continue;
        }
        if DEFAULT_PROJECT_ROOT_NAMES.contains(&name) {
            continue; // already queued
        }
        if count_direct_project_children(&path) >= 2 {
            hubs.push(path.display().to_string());
        }
    }
    hubs.sort();
    hubs
}

fn count_direct_project_children(dir: &Path) -> usize {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return 0;
    };
    read_dir
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !should_skip_dir(path) && is_project_directory(path))
        .count()
}

fn is_project_directory(path: &Path) -> bool {
    let git = path.join(".git");
    if git.is_dir() || git.is_file() {
        return true;
    }
    REPO_MARKERS
        .iter()
        .any(|marker| path.join(marker).is_file())
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

    // Treat `.git` as a file (worktrees / submodules) the same as a directory.
    let is_git_root = canonical.join(".git").is_dir() || canonical.join(".git").is_file();
    let has_marker = REPO_MARKERS
        .iter()
        .any(|marker| canonical.join(marker).is_file());

    // Hub roots + first-level category folders (e.g. ~/side-projects/dev-tools) so
    // users can search and Tab into them, not only leaf git repos.
    if depth <= INDEX_INTERMEDIATE_MAX_DEPTH {
        if let Some((label, cwd)) = canonical_directory_path(&canonical.display().to_string(), home)
        {
            ranked
                .entry(cwd)
                .or_insert((label, DirectorySource::ScanRoot));
        }
    }

    if is_git_root || has_marker {
        if let Some((label, cwd)) = canonical_directory_path(&canonical.display().to_string(), home)
        {
            let source = if is_git_root {
                DirectorySource::GitRoot
            } else {
                DirectorySource::RepoMarker
            };
            ranked
                .entry(cwd)
                .and_modify(|(existing_label, existing)| {
                    *existing_label = label.clone();
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
        .filter(|path| path.is_dir() && !should_skip_dir(path))
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
            label: "~/projects/acme/acme-frontend".into(),
            cwd: "/home/test/projects/acme/acme-frontend".into(),
            source: DirectorySource::WorkspaceConfig,
        };
        assert!(directory_matches_query(&dir, "acme"));
        assert!(directory_matches_query(&dir, "frontend"));
        assert!(!directory_matches_query(&dir, "backend"));
    }

    #[test]
    fn default_scan_roots_include_side_projects_and_discovered_hubs() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_string_lossy().to_string();
        fs::create_dir_all(temp.path().join("side-projects")).unwrap();
        // Hub: two direct git projects (e.g. monorepo workspace).
        let hub = temp.path().join("work-hub");
        fs::create_dir_all(hub.join("frontend").join(".git")).unwrap();
        fs::create_dir_all(hub.join("backend").join(".git")).unwrap();
        // Noise: single child or empty dirs should not become roots.
        fs::create_dir_all(temp.path().join("other-noise").join("lonely").join(".git")).unwrap();
        fs::create_dir_all(temp.path().join("empty-dir")).unwrap();
        let roots = scan_roots(&home);
        assert!(roots.iter().any(|r| r.ends_with("/side-projects")));
        assert!(roots.iter().any(|r| r.ends_with("/work-hub")));
        assert!(!roots.iter().any(|r| r.ends_with("/other-noise")));
        assert!(!roots.iter().any(|r| r.ends_with("/empty-dir")));
    }

    #[test]
    fn basename_search_finds_project_under_default_roots() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_string_lossy().to_string();
        let project = temp
            .path()
            .join("side-projects")
            .join("dev-tools")
            .join("sessions-cli");
        fs::create_dir_all(project.join(".git")).unwrap();
        // Noise under archive should be skipped.
        let archived = temp
            .path()
            .join("side-projects")
            .join("_archive")
            .join("old-sessions");
        fs::create_dir_all(archived.join(".git")).unwrap();

        let mut ranked = HashMap::new();
        let mut seen = HashSet::new();
        for root in scan_roots(&home) {
            scan_directory(Path::new(&root), 0, &home, &mut ranked, &mut seen);
        }
        let labels: Vec<String> = ranked.values().map(|(label, _)| label.clone()).collect();
        assert!(
            labels
                .iter()
                .any(|l| path_query_matches_label("sessions", l)),
            "expected sessions-cli match, got {labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l.contains("_archive")),
            "archive projects should not be indexed: {labels:?}"
        );
    }

    #[test]
    fn query_matches_basename_only() {
        assert!(path_query_matches_label("ses", "~/projects/sessions-cli"));
        assert!(path_query_matches_label("~/ses", "~/projects/sessions-cli"));
        assert!(path_query_matches_label(
            "sessions-cli",
            "~/projects/sessions-cli"
        ));
        assert!(!path_query_matches_label(
            "cloud",
            "~/projects/sessions-cli"
        ));
    }

    #[test]
    fn home_suggestion_is_prefix_only_not_substring() {
        let index = DirectoryIndex {
            home: "/home/test".into(),
            directories: vec![DiscoveredDirectory {
                label: "~/projects/sessions-cli".into(),
                cwd: "/home/test/projects/sessions-cli".into(),
                source: DirectorySource::GitRoot,
            }],
        };
        // Old bug: `"home".contains("e")` injected ~ for almost every letter query.
        let for_e = index.suggestions_for_query("e");
        assert!(
            !for_e.iter().any(|(label, _)| label == "~"),
            "letter e must not force home entry: {for_e:?}"
        );
        let for_ho = index.suggestions_for_query("ho");
        assert!(
            for_ho.iter().any(|(label, _)| label == "~"),
            "prefix of home still injects ~: {for_ho:?}"
        );
    }

    #[test]
    fn intermediate_category_folders_are_indexed() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_string_lossy().to_string();
        let category = temp.path().join("side-projects").join("dev-tools");
        let project = category.join("sessions-cli");
        fs::create_dir_all(project.join(".git")).unwrap();
        let mut ranked = HashMap::new();
        let mut seen = HashSet::new();
        for root in scan_roots(&home) {
            scan_directory(Path::new(&root), 0, &home, &mut ranked, &mut seen);
        }
        let labels: Vec<String> = ranked.values().map(|(label, _)| label.clone()).collect();
        assert!(
            labels
                .iter()
                .any(|l| l.ends_with("side-projects") || l.contains("~/side-projects")),
            "hub root should be indexed: {labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|l| l.contains("dev-tools") && !l.contains("sessions-cli")),
            "category folder should be indexed: {labels:?}"
        );
        assert!(
            labels.iter().any(|l| l.contains("sessions-cli")),
            "project still indexed: {labels:?}"
        );
    }

    #[test]
    fn filesystem_completions_skip_archive_and_list_beyond_eight() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_string_lossy().to_string();
        let hub = temp.path().join("side-projects");
        for name in [
            "_archive",
            "business",
            "career",
            "content",
            "dev-tools",
            "devices",
            "distraction",
            "productivity",
            "websites",
            "extra-nine",
            "extra-ten",
        ] {
            fs::create_dir_all(hub.join(name)).unwrap();
        }
        let completions = filesystem_completions("~/side-projects/", &home);
        assert!(
            !completions
                .iter()
                .any(|(label, _)| label.contains("_archive")),
            "archive must be skipped: {completions:?}"
        );
        assert!(
            completions
                .iter()
                .any(|(label, _)| label.contains("websites")),
            "must not drop children past the old 8-cap: {completions:?}"
        );
        assert!(
            completions
                .iter()
                .any(|(label, _)| label.contains("extra-ten")),
            "64-cap should include 10th child: {completions:?}"
        );
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
        let prev_home = std::env::var_os("HOME");
        let prev_roots = std::env::var_os("SESSIONS_DIRECTORY_ROOTS");
        std::env::set_var("HOME", &home);
        std::env::set_var("SESSIONS_DIRECTORY_ROOTS", root.display().to_string());
        let index = DirectoryIndex::build(&Config::default());
        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match prev_roots {
            Some(r) => std::env::set_var("SESSIONS_DIRECTORY_ROOTS", r),
            None => std::env::remove_var("SESSIONS_DIRECTORY_ROOTS"),
        }
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
