use std::path::Path;

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

impl WorkspaceCatalog {
    pub fn workspace_ref_with_command<'a>(title: &'a str, command: &'a str) -> WorkspaceRef<'a> {
        WorkspaceRef { title, command }
    }

    pub fn load(path: &Path) -> Self {
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
