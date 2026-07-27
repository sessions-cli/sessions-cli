use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use crate::pty::naming::{parse_description, workspace_project};

#[derive(Debug, Clone, Copy)]
pub struct WorkspaceRef<'a> {
    pub title: &'a str,
    pub command: &'a str,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceCatalog {
    pub entries: Vec<WorkspaceEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorkspaceEntry {
    pub title: String,
    pub cwd: String,
    #[serde(default)]
    pub command: String,
}

struct CatalogCache {
    path: PathBuf,
    mtime: Option<SystemTime>,
    catalog: WorkspaceCatalog,
}

static CATALOG_CACHE: LazyLock<Mutex<Option<CatalogCache>>> = LazyLock::new(|| Mutex::new(None));

fn path_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl WorkspaceCatalog {
    pub fn workspace_ref_with_command<'a>(title: &'a str, command: &'a str) -> WorkspaceRef<'a> {
        WorkspaceRef { title, command }
    }

    pub fn load(path: &Path) -> Self {
        let mtime = path_mtime(path);
        if let Ok(guard) = CATALOG_CACHE.lock() {
            if let Some(cache) = guard.as_ref() {
                if cache.path == path && cache.mtime == mtime {
                    return cache.catalog.clone();
                }
            }
        }

        let catalog = Self::load_uncached(path);
        if let Ok(mut guard) = CATALOG_CACHE.lock() {
            *guard = Some(CatalogCache {
                path: path.to_path_buf(),
                mtime,
                catalog: catalog.clone(),
            });
        }
        catalog
    }

    fn load_uncached(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        #[derive(serde::Deserialize)]
        struct File {
            #[serde(default)]
            workspace: Vec<WorkspaceEntry>,
        }
        let Ok(file) = toml::from_str::<File>(&raw) else {
            return Self::default();
        };
        Self {
            entries: file.workspace,
        }
    }

    pub fn entry_for_window_index(&self, index: u32) -> Option<&WorkspaceEntry> {
        self.entries.get(index.saturating_sub(1) as usize)
    }

    pub fn entry_for_cwd(&self, cwd: &str) -> Option<&WorkspaceEntry> {
        let cwd = cwd.trim_end_matches('/');
        self.entries
            .iter()
            .find(|entry| entry.cwd.trim_end_matches('/') == cwd)
    }

    pub fn workspace_ref_for_window(&self, index: u32, cwd: &str) -> Option<WorkspaceRef<'_>> {
        let cwd = cwd.trim_end_matches('/');
        let entry = self
            .entry_for_window_index(index)
            .filter(|entry| entry.cwd.trim_end_matches('/') == cwd)?;
        Some(WorkspaceRef {
            title: entry.title.as_str(),
            command: entry.command.as_str(),
        })
    }

    pub fn bootstrap_command_for_window(&self, index: u32, cwd: &str) -> Option<&str> {
        let cwd = cwd.trim_end_matches('/');
        self.entry_for_window_index(index)
            .filter(|entry| entry.cwd.trim_end_matches('/') == cwd)
            .map(|entry| entry.command.as_str())
    }

    pub fn thread_for_window_index(&self, index: u32) -> Option<String> {
        let entry = self.entry_for_window_index(index)?;
        let thread = parse_description(&entry.title);
        (!thread.is_empty() && thread != "session").then_some(thread)
    }

    pub fn thread_for_cwd(&self, cwd: &str) -> Option<String> {
        let entry = self.entry_for_cwd(cwd)?;
        let thread = parse_description(&entry.title);
        (!thread.is_empty() && thread != "session").then_some(thread)
    }

    pub fn project_for_title(&self, title: &str, cwd: &str, home: &Path) -> String {
        workspace_project(title, cwd, home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_reuses_cache_when_mtime_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workspaces.toml");
        {
            let mut f = std::fs::File::create(&path).expect("create");
            writeln!(
                f,
                r#"
[[workspace]]
title = "alpha"
cwd = "/tmp/alpha"
command = "echo hi"
"#
            )
            .expect("write");
        }
        let first = WorkspaceCatalog::load(&path);
        assert_eq!(first.entries.len(), 1);
        assert_eq!(first.entries[0].title, "alpha");

        // Same mtime → cache hit (clone of prior parse).
        let second = WorkspaceCatalog::load(&path);
        assert_eq!(second.entries.len(), 1);
        assert_eq!(second.entries[0].title, "alpha");

        // Touch content + mtime so the next load re-parses.
        std::thread::sleep(std::time::Duration::from_millis(20));
        {
            let mut f = std::fs::File::create(&path).expect("rewrite");
            writeln!(
                f,
                r#"
[[workspace]]
title = "beta"
cwd = "/tmp/beta"
command = "echo bye"
"#
            )
            .expect("write");
        }
        let third = WorkspaceCatalog::load(&path);
        assert_eq!(third.entries.len(), 1);
        assert_eq!(third.entries[0].title, "beta");
    }
}
