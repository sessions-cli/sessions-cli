//! New-session launch orchestration.

use super::input::NewSessionAction;
use super::state::{
    clear_draft, expand_workspace_path, load_defaults, load_workspaces, refresh_daemon,
    save_defaults, Focus, LaunchMode, LaunchOutcome, NewSessionState,
};
use crate::agents;
use crate::config::Config;
use crate::daemon::tmux;
use crate::session::workspace_usage::WorkspaceUsageStore;
use crate::session::{create_with_launch_command, ManifestSource};
use anyhow::{Context, Result};
use std::time::Duration;

impl NewSessionState {
    pub(in crate::bar::overlay::new_session) fn try_launch_console_foreground(
        &mut self,
        config: &Config,
    ) -> Result<Option<NewSessionAction>> {
        if self.selected_agent().id != "console" {
            return Ok(None);
        }
        self.confirm_agent_enter();
        if self.launch(config, LaunchMode::Open)? == LaunchOutcome::Opened {
            return Ok(Some(NewSessionAction::Launched));
        }
        Ok(None)
    }

    /// Reset the form after a background launch so another session can be queued
    /// without closing or respawning the new-session pane.
    pub(in crate::bar::overlay::new_session) fn prepare_for_next_session(
        &mut self,
        config: &Config,
    ) {
        self.workspaces = load_workspaces(config, &self.workspace_usage, self.rank_mode);
        let preset_label = self.defaults.workspace_label.as_deref();
        let preset_path = self
            .defaults
            .custom_workspace_path
            .as_deref()
            .filter(|path| !path.is_empty());
        self.workspace_idx = preset_label
            .and_then(|label| self.workspaces.iter().position(|w| w.label == label))
            .unwrap_or(0);
        if let Some(path) = preset_path {
            self.workspace_path_input = path.to_string();
            self.workspace_user_editing = true;
        } else if !self.workspaces.is_empty() {
            self.workspace_path_input.clear();
            self.workspace_user_editing = false;
        } else {
            self.workspace_path_input = "~/".to_string();
            self.workspace_user_editing = true;
        }
        if !self.workspace_path_input.trim().is_empty()
            && expand_workspace_path(&self.workspace_path_input).is_err()
        {
            if self.workspaces.is_empty() {
                self.workspace_path_input = "~/".to_string();
                self.workspace_user_editing = true;
            } else {
                self.workspace_path_input.clear();
                self.workspace_user_editing = false;
            }
        } else if self.workspace_path_input.trim().is_empty() && self.workspaces.is_empty() {
            self.workspace_path_input = "~/".to_string();
            self.workspace_user_editing = true;
        }
        self.workspace_confirmed = false;
        self.agent_idx = agents::AGENTS
            .iter()
            .position(|a| a.id == self.defaults.agent_id)
            .unwrap_or(0);
        self.agent_confirmed = false;
        self.model_confirmed = false;
        self.sync_model_idx();
        self.prompt.clear();
        self.prompt_cursor = 0;
        self.prompt_scroll = 0;
        self.prompt_selection = None;
        self.prompt_select_anchor = None;
        self.prompt_drag_selecting = false;
        self.prompt_last_click = None;
        self.focus = Focus::Workspace;
        self.status.clear();
        self.reset_directory_display_limit();
        self.workspace_popup_highlight = self.default_workspace_popup_highlight();
    }

    pub(in crate::bar::overlay::new_session) fn launch(
        &mut self,
        config: &Config,
        mode: LaunchMode,
    ) -> Result<LaunchOutcome> {
        self.launch_mode = mode;
        let (cwd, workspace_label) = match self.resolve_workspace() {
            Ok(workspace) => workspace,
            Err(error) => {
                self.status = error.to_string();
                return Ok(LaunchOutcome::Failed);
            }
        };
        let agent = self.selected_agent();
        let model_id = self.selected_model_id();
        let prompt = self.prompt.trim();
        let focus = mode == LaunchMode::Open;
        let _window_index = if agent.id == "console" {
            tmux::create_terminal_window_in_cwd(&config, &cwd, focus)
                .with_context(|| format!("create terminal in {}", workspace_label))?
        } else {
            let deliver_prompt_via_tmux = !prompt.is_empty()
                && agents::deliver_prompt_via_tmux(agent.id, model_id, &cwd, prompt);
            let launch_command = agents::build_launch_command_with_prompt(
                agent.id,
                model_id,
                if deliver_prompt_via_tmux {
                    None
                } else {
                    Some(prompt)
                },
            );
            let created = create_with_launch_command(
                config, &cwd, &launch_command, ManifestSource::NewChat, focus,
                Some(model_id), if prompt.is_empty() { None } else { Some(prompt) },
            ).with_context(|| format!("create {} ({}) session in {}", agent.label, model_id, workspace_label))?;
            let wi = created.index;

            if deliver_prompt_via_tmux {
                std::thread::sleep(Duration::from_millis(2_000));
                if let Err(e) = tmux::send_literal_to_window(&config.tmux_session, wi, prompt) {
                    self.status = format!("opened; prompt failed: {e}");
                    refresh_daemon(config);
                    return Ok(LaunchOutcome::Failed);
                }
                let _ = tmux::send_keys_to_window(&config.tmux_session, wi, &["Enter"]);
            }
            wi
        };

        if mode == LaunchMode::Open {
            let _ = tmux::restore_workspace_attach(&config.tmux_ui_session, &config.tmux_session);
        }
        if let Err(error) =
            WorkspaceUsageStore::record_focus_at(&config.home, &cwd, &workspace_label)
        {
            eprintln!("workspace usage: {error}");
        }
        refresh_daemon(config);
        if mode == LaunchMode::Background {
            self.prepare_for_next_session(config);
            let _ = self.save_draft(config);
            return Ok(LaunchOutcome::Backgrounded);
        }
        let _ = clear_draft(config);
        self.status.clear();
        Ok(LaunchOutcome::Opened)
    }
}

pub fn preset_workspace_for_new_session(config: &Config, workspace_label: &str) -> Result<()> {
    let mut defaults = load_defaults(config);
    defaults.workspace_label = Some(workspace_label.to_string());
    defaults.custom_workspace_path = None;
    save_defaults(config, &defaults)
}