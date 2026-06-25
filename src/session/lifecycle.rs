use crate::agents::build_quick_launch_command;
use crate::config::Config;
use crate::daemon::persist::load_state_or_empty;
use crate::daemon::tmux::{
    self, bind_workspace_keys, configure_workspace_session, select_window, CreatedWindow,
};
use crate::pty::{
    agent_from_command, format_session_title, CONSOLE_LABEL, DEFAULT_AGENT_APP,
};
use crate::model::Session;
use crate::session::managed::{new_sessions_session_id, ManagedLaunchRecord};
use crate::agents::launcher::effective_restore_command_at;
use crate::session::manifest::{
    append_entry, load_manifest, manifest_entry_for_ssn, mark_entry_closed,
    reopen_manifest_entry, ManifestEntry, ManifestSource,
};
use crate::session::restore::workspace_bootstrap_closed;
use crate::session::workspace::WorkspaceEntry;
use anyhow::{Context, Result};
use chrono::Utc;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct CloseTarget {
    pub session_id: Option<String>,
    pub sessions_session_id: Option<String>,
    pub window_index: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct CloseOutcome {
    pub session_id: String,
    pub window_index: u32,
    pub tmux_session: String,
    pub agent_session_id: Option<String>,
    pub tmux_pane_id: String,
    pub sessions_session_id: Option<String>,
    pub removed: Option<Session>,
}

#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub sessions_session_id: String,
    pub source: ManifestSource,
    pub cwd: String,
    pub agent: String,
    pub launch_command: String,
    pub workspace_index: Option<u32>,
    pub focus: bool,
    /// Override tmux window name; bootstrap uses `ws-N-slug` before rename to title.
    pub window_name: Option<String>,
    /// First workspace window uses `tmux new-session` instead of `new-window`.
    pub bootstrap_new_session: bool,
    /// Model for manifest normalization when `user_prompt` is set.
    pub model_id: Option<String>,
    /// Non-empty value triggers manifest stripping (see `manifest_launch_command_for_spec`).
    /// Tmux `launch_command` may still include the prompt; this field is not inferred from it.
    pub user_prompt: Option<String>,
}

impl LaunchSpec {
    pub fn to_manifest_entry(&self, home: &Path) -> ManifestEntry {
        ManifestEntry {
            sessions_session_id: self.sessions_session_id.clone(),
            source: self.source,
            workspace_index: self.workspace_index,
            cwd: self.cwd.clone(),
            cwd_label: crate::pty::format_tilde_path(&self.cwd, home),
            agent: self.agent.clone(),
            launch_command: crate::session::manifest::manifest_launch_command_for_spec(self),
            agent_session_id: None,
            title: None,
            messaged_at: self
                .source
                .stamps_order_on_launch()
                .then(Utc::now),
            closed: false,
        }
    }

    pub fn to_managed_record(&self, created: &CreatedWindow, tmux_session: &str) -> ManagedLaunchRecord {
        ManagedLaunchRecord {
            sessions_session_id: created.sessions_session_id.clone(),
            launch_id: crate::session::new_launch_id(),
            agent: self.agent.clone(),
            tmux_session: tmux_session.to_string(),
            window_index: created.index,
            pane_id: if created.pane_id.is_empty() {
                None
            } else {
                Some(created.pane_id.clone())
            },
            initial_cwd: self.cwd.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            agent_session_id: None,
        }
    }
}

/// Deterministic bootstrap id: `ws:{workspace_index}:{blake3_8(cwd + "\0" + launch_command)}`
pub fn bootstrap_sessions_session_id(
    workspace_index: u32,
    cwd: &str,
    launch_command: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(cwd.as_bytes());
    hasher.update(b"\0");
    hasher.update(launch_command.as_bytes());
    let hash = hasher.finalize();
    let short_hex: String = hash.as_bytes()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("ws:{workspace_index}:{short_hex}")
}

pub fn agent_for_launch_command(launch_command: &str) -> String {
    if launch_command.is_empty() {
        return "console".to_string();
    }
    agent_from_command(launch_command).unwrap_or_else(|| "console".to_string())
}

pub fn launch_spec_for_agent(
    cwd: String,
    agent_id: &str,
    launch_command: Option<String>,
    source: ManifestSource,
    focus: bool,
) -> LaunchSpec {
    let launch_command =
        launch_command.unwrap_or_else(|| build_quick_launch_command(agent_id));
    LaunchSpec {
        sessions_session_id: new_sessions_session_id(),
        source,
        cwd,
        agent: agent_id.to_string(),
        launch_command,
        workspace_index: None,
        focus,
        window_name: None,
        bootstrap_new_session: false,
        model_id: None,
        user_prompt: None,
    }
}

/// Unified create path: managed tmux window + durable record. Manifest append is P1.
pub fn create_unified(config: &Config, spec: LaunchSpec) -> Result<CreatedWindow> {
    append_entry(config, spec.to_manifest_entry(&config.home))?;

    let window_name = spec
        .window_name
        .clone()
        .unwrap_or_else(|| window_name_for_spec(&spec));
    let managed_agent = managed_agent_for_spec(&spec);
    let command = (!spec.launch_command.is_empty()).then_some(spec.launch_command.as_str());

    let created = tmux::create_managed_window_in_cwd(
        &config.tmux_session,
        &spec.cwd,
        managed_agent,
        command,
        &window_name,
        spec.focus,
        &spec.sessions_session_id,
        spec.bootstrap_new_session,
    )?;

    let record = spec.to_managed_record(&created, &config.tmux_session);
    let _ = crate::session::save_managed_record(&config.home, &record);

    Ok(created)
}

pub fn create_quick_agent(
    config: &Config,
    cwd: &str,
    agent_id: &str,
    source: ManifestSource,
    focus: bool,
) -> Result<CreatedWindow> {
    create_unified(
        config,
        launch_spec_for_agent(cwd.to_string(), agent_id, None, source, focus),
    )
}

pub fn create_console(
    config: &Config,
    cwd: &str,
    source: ManifestSource,
    focus: bool,
) -> Result<CreatedWindow> {
    create_unified(
        config,
        launch_spec_for_agent(cwd.to_string(), "console", None, source, focus),
    )
}

pub fn create_with_launch_command(
    config: &Config,
    cwd: &str,
    launch_command: &str,
    source: ManifestSource,
    focus: bool,
    model_id: Option<&str>,
    user_prompt: Option<&str>,
) -> Result<CreatedWindow> {
    let agent_id = agent_for_launch_command(launch_command);
    create_unified(
        config,
        LaunchSpec {
            sessions_session_id: new_sessions_session_id(),
            source,
            cwd: cwd.to_string(),
            agent: agent_id,
            launch_command: launch_command.to_string(),
            workspace_index: None,
            focus,
            window_name: None,
            bootstrap_new_session: false,
            model_id: model_id.map(str::to_string),
            user_prompt: user_prompt.map(str::to_string),
        },
    )
}

pub fn create_instant_agent(config: &Config, agent_id: &str) -> Result<CreatedWindow> {
    let cwd = if agent_id == "console" {
        crate::paths::home().display().to_string()
    } else {
        let (_, cwd) = tmux::active_window_details(&config.tmux_session)?;
        cwd
    };
    create_quick_agent(config, &cwd, agent_id, ManifestSource::InstantKey, true)
}

/// Bootstrap tmux `agents` session from workspaces.toml via `create_unified`.
pub fn bootstrap_workspaces(config: &Config) -> Result<()> {
    tmux::ensure_tmux_available()?;
    if tmux::session_exists(&config.tmux_session) {
        bind_workspace_keys(&config.tmux_session, &tmux::sessions_binary())?;
        configure_workspace_session(&config.tmux_session)?;
        return Ok(());
    }

    let raw = std::fs::read_to_string(&config.workspaces_path)
        .with_context(|| format!("read {}", config.workspaces_path.display()))?;
    let file: BootstrapWorkspacesFile =
        toml::from_str(&raw).with_context(|| "parse workspaces.toml")?;

    if file.workspace.is_empty() {
        anyhow::bail!(
            "no workspaces defined in {}",
            config.workspaces_path.display()
        );
    }

    let manifest = load_manifest(config)?;
    let mut first_window = true;
    let mut created_any = false;
    for (i, ws) in file.workspace.iter().enumerate() {
        if !Path::new(&ws.cwd).is_dir() {
            anyhow::bail!("workspace {} cwd does not exist: {}", ws.title, ws.cwd);
        }
        if workspace_bootstrap_closed(&manifest, i as u32, &ws.cwd, &ws.command) {
            continue;
        }
        let window_name = tmux_window_name(i, &ws.title);
        let sessions_session_id = bootstrap_sessions_session_id(i as u32, &ws.cwd, &ws.command);
        let manifest_entry = manifest_entry_for_ssn(&manifest, &sessions_session_id);
        let launch_command = manifest_entry
            .map(|entry| effective_restore_command_at(&config.home, entry))
            .unwrap_or_else(|| ws.command.clone());
        let agent = agent_for_launch_command(&launch_command);
        let spec = LaunchSpec {
            sessions_session_id,
            source: ManifestSource::WorkspaceBootstrap,
            cwd: ws.cwd.clone(),
            agent,
            launch_command,
            workspace_index: Some(i as u32),
            focus: false,
            window_name: Some(window_name.clone()),
            bootstrap_new_session: first_window,
            model_id: None,
            user_prompt: None,
        };
        let created = create_unified(config, spec)?;
        first_window = false;
        created_any = true;
        if window_name != ws.title {
            let _ = tmux::rename_window(&config.tmux_session, created.index, &ws.title);
        }
    }

    let default_idx = file
        .default_focus
        .min(file.workspace.len().saturating_sub(1) as u32);
    let focus = if !created_any {
        seed_agents_session_from_default_workspace(config, &file, default_idx)?.index
    } else {
        file.default_focus.min(file.workspace.len() as u32)
    };
    select_window(&config.tmux_session, focus)?;
    bind_workspace_keys(&config.tmux_session, &tmux::sessions_binary())?;
    configure_workspace_session(&config.tmux_session)?;
    Ok(())
}

/// Cold boot when every workspace slot is tombstoned: recreate the default
/// workspace so the agents tmux session exists for manifest restore.
fn seed_agents_session_from_default_workspace(
    config: &Config,
    file: &BootstrapWorkspacesFile,
    workspace_index: u32,
) -> Result<CreatedWindow> {
    let idx = workspace_index as usize;
    let ws = file
        .workspace
        .get(idx)
        .with_context(|| format!("workspace index {idx} missing from workspaces.toml"))?;
    if !Path::new(&ws.cwd).is_dir() {
        anyhow::bail!("workspace {} cwd does not exist: {}", ws.title, ws.cwd);
    }
    let window_name = tmux_window_name(idx, &ws.title);
    let sessions_session_id = bootstrap_sessions_session_id(idx as u32, &ws.cwd, &ws.command);
    reopen_manifest_entry(config, &sessions_session_id)?;
    let manifest = load_manifest(config)?;
    let manifest_entry = manifest_entry_for_ssn(&manifest, &sessions_session_id);
    let launch_command = manifest_entry
        .map(|entry| effective_restore_command_at(&config.home, entry))
        .unwrap_or_else(|| ws.command.clone());
    let agent = agent_for_launch_command(&launch_command);
    let spec = LaunchSpec {
        sessions_session_id,
        source: ManifestSource::WorkspaceBootstrap,
        cwd: ws.cwd.clone(),
        agent,
        launch_command,
        workspace_index: Some(idx as u32),
        focus: false,
        window_name: Some(window_name.clone()),
        bootstrap_new_session: true,
        model_id: None,
        user_prompt: None,
    };
    let created = create_unified(config, spec)?;
    if window_name != ws.title {
        let _ = tmux::rename_window(&config.tmux_session, created.index, &ws.title);
    }
    Ok(created)
}

fn window_name_for_spec(spec: &LaunchSpec) -> String {
    if spec.agent == "console" {
        return CONSOLE_LABEL.to_string();
    }
    if spec.agent == "session" || spec.launch_command.is_empty() && spec.agent.is_empty() {
        return "session".to_string();
    }
    format_session_title(&spec.agent, "?")
}

fn managed_agent_for_spec(spec: &LaunchSpec) -> Option<&str> {
    match spec.agent.as_str() {
        "console" | "session" | "" => None,
        agent => Some(agent),
    }
}

fn tmux_window_name(index: usize, title: &str) -> String {
    let slug: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("ws-{}", index + 1)
    } else {
        format!("ws-{}-{slug}", index + 1)
    }
}

#[derive(Debug, Deserialize)]
struct BootstrapWorkspacesFile {
    #[serde(default = "default_focus")]
    default_focus: u32,
    workspace: Vec<WorkspaceEntry>,
}

fn default_focus() -> u32 {
    1
}

/// Unified close path: kill live window, GC managed/env/title artifacts.
pub fn close_unified(config: &Config, target: CloseTarget) -> Result<CloseOutcome> {
    let resolved = resolve_session_for_close(config, &target);
    let window_index = resolved
        .as_ref()
        .map(|session| session.tab_index)
        .or(target.window_index)
        .context("close target missing window index")?;
    let tmux_session = resolved
        .as_ref()
        .map(|session| session.tmux_session.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(config.tmux_session.as_str())
        .to_string();
    let session_id = resolved
        .as_ref()
        .map(|session| session.id.clone())
        .or_else(|| {
            target
                .session_id
                .clone()
                .or_else(|| Some(Session::session_id_from_window(window_index)))
        })
        .context("close target missing session id")?;

    kill_window_if_live(&tmux_session, window_index)?;

    let sessions_session_id = resolved
        .as_ref()
        .and_then(|session| session.sessions_session_id.clone())
        .or(target.sessions_session_id.clone());
    if let Some(ref ssn) = sessions_session_id {
        mark_entry_closed(config, ssn)?;
    }
    let agent_session_id = resolved
        .as_ref()
        .and_then(|session| session.agent_session_id.clone());
    let tmux_pane_id = resolved
        .as_ref()
        .map(|session| session.tmux_pane_id.clone())
        .unwrap_or_default();

    cleanup_closed_session_artifacts(
        config,
        window_index,
        sessions_session_id.as_deref(),
        agent_session_id.as_deref(),
    )?;

    Ok(CloseOutcome {
        session_id,
        window_index,
        tmux_session,
        agent_session_id,
        tmux_pane_id,
        sessions_session_id,
        removed: resolved,
    })
}

fn resolve_session_for_close(config: &Config, target: &CloseTarget) -> Option<Session> {
    let state = load_state_or_empty(config);
    if let Some(ref session_id) = target.session_id {
        if let Some(session) = state.sessions.iter().find(|s| s.id == *session_id) {
            return Some(session.clone());
        }
    }
    if let Some(ref sessions_session_id) = target.sessions_session_id {
        if let Some(session) = state
            .sessions
            .iter()
            .find(|s| s.sessions_session_id.as_deref() == Some(sessions_session_id.as_str()))
        {
            return Some(session.clone());
        }
    }
    if let Some(window_index) = target.window_index {
        let id = Session::session_id_from_window(window_index);
        if let Some(session) = state.sessions.iter().find(|s| s.id == id) {
            return Some(session.clone());
        }
        if let Some(session) = state
            .sessions
            .iter()
            .find(|s| s.tab_index == window_index)
        {
            return Some(session.clone());
        }
    }
    None
}

fn kill_window_if_live(tmux_session: &str, window_index: u32) -> Result<()> {
    let Ok(windows) = tmux::list_windows(tmux_session) else {
        return Ok(());
    };
    if windows.len() <= 1 {
        anyhow::bail!("refusing to close the last remaining session window");
    }
    if windows.iter().any(|window| window.index == window_index) {
        tmux::close_window(tmux_session, window_index)?;
    }
    Ok(())
}

pub(crate) fn cleanup_closed_session_artifacts(
    config: &Config,
    window_index: u32,
    sessions_session_id: Option<&str>,
    agent_session_id: Option<&str>,
) -> Result<()> {
    if let Some(ssn) = sessions_session_id {
        crate::session::remove_managed_record(&config.home, ssn)?;
    }
    if let Some(sid) = agent_session_id {
        let _ = std::fs::remove_file(config.session_env_path(sid));
        let _ = std::fs::remove_file(config.session_title_path(sid));
    }
    clear_manual_title_files_for_tab(config, window_index)?;
    Ok(())
}

fn clear_manual_title_files_for_tab(config: &Config, tab_index: u32) -> std::io::Result<()> {
    let title_path = config.session_title_path_for_tab(tab_index);
    let manual_path = config.session_title_manual_path_for_tab(tab_index);
    if title_path.exists() {
        std::fs::remove_file(title_path)?;
    }
    if manual_path.exists() {
        std::fs::remove_file(manual_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentState;
    use crate::session::managed::{save_managed_record, ManagedLaunchRecord};
    use chrono::Utc;
    use std::collections::HashSet;
    use tempfile::TempDir;

    fn test_session(id: &str, tab_index: u32, ssn: &str, agent_sid: &str) -> Session {
        Session {
            id: id.into(),
            kitty_window_id: tab_index as u64,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index,
            tmux_session: "agents".into(),
            tmux_pane_id: format!("%{tab_index}"),
            pane_pid: 0,
            agent_session_id: Some(agent_sid.into()),
            title: "grok · test".into(),
            description: "test".into(),
            cwd: "/tmp".into(),
            cwd_label: "/tmp".into(),
            project: "grok".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            managed: true,
            sessions_session_id: Some(ssn.into()),
            managed_agent: Some("grok".into()),
        }
    }

    #[test]
    fn bootstrap_sessions_session_id_is_stable() {
        let id1 = bootstrap_sessions_session_id(2, "/abs/path", "grok --resume uuid");
        let id2 = bootstrap_sessions_session_id(2, "/abs/path", "grok --resume uuid");
        assert_eq!(id1, id2);
        assert!(id1.starts_with("ws:2:"));
        assert_eq!(id1.len(), "ws:2:".len() + 16);
    }

    #[test]
    fn bootstrap_sessions_session_id_differs_by_workspace_index() {
        let cwd = "/abs/path";
        let cmd = "grok";
        let a = bootstrap_sessions_session_id(0, cwd, cmd);
        let b = bootstrap_sessions_session_id(1, cwd, cmd);
        assert_ne!(a, b);
    }

    #[test]
    fn create_with_launch_command_round_trips_normalized_manifest_entry() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        let prompt = "fix the sidebar";
        let model_id = "grok-composer-2.5-fast";
        let cwd = "/tmp/new-chat-roundtrip";
        let launch_command =
            crate::agents::build_launch_command_with_prompt("grok", model_id, Some(prompt));

        let _ = create_with_launch_command(
            &config,
            cwd,
            &launch_command,
            ManifestSource::NewChat,
            false,
            Some(model_id),
            Some(prompt),
        );

        let manifest = crate::session::manifest::load_manifest(&config).unwrap();
        assert_eq!(manifest.entries.len(), 1);
        let entry = &manifest.entries[0];
        assert_eq!(entry.agent, "grok");
        assert_eq!(entry.cwd, cwd);
        assert_eq!(entry.launch_command, "grok --model grok-composer-2.5-fast");
        assert!(!entry.launch_command.contains(prompt));
        assert!(launch_command.contains(prompt));
    }

    #[test]
    fn new_chat_manifest_entry_omits_user_prompt_text() {
        let home = Path::new("/home/testuser");
        let prompt = "fix the sidebar";
        let spec = LaunchSpec {
            sessions_session_id: "ssn_prompt_strip".into(),
            source: ManifestSource::NewChat,
            cwd: "/home/testuser/projects/foo".into(),
            agent: "grok".into(),
            launch_command: crate::agents::build_launch_command_with_prompt("grok", "grok-composer-2.5-fast", Some(prompt)),
            workspace_index: None,
            focus: true,
            window_name: None,
            bootstrap_new_session: false,
            model_id: Some("grok-composer-2.5-fast".into()),
            user_prompt: Some(prompt.into()),
        };
        let entry = spec.to_manifest_entry(home);
        assert_eq!(entry.launch_command, "grok --model grok-composer-2.5-fast");
        assert!(!entry.launch_command.contains(prompt));
    }

    #[test]
    fn launch_spec_for_agent_uses_dynamic_id() {
        let spec = launch_spec_for_agent(
            "/tmp".into(),
            DEFAULT_AGENT_APP,
            None,
            ManifestSource::InstantKey,
            true,
        );
        assert!(spec.sessions_session_id.starts_with("ssn_"));
        assert_eq!(spec.agent, "grok");
        assert_eq!(spec.launch_command, "grok");
        assert_eq!(spec.source, ManifestSource::InstantKey);
    }

    #[test]
    fn to_manifest_entry_stamps_messaged_at_for_user_launches() {
        let home = Path::new("/home/testuser");
        for source in [
            ManifestSource::NewChat,
            ManifestSource::InstantKey,
            ManifestSource::Cli,
        ] {
            let spec = launch_spec_for_agent("/tmp".into(), "grok", None, source, true);
            let entry = spec.to_manifest_entry(home);
            assert!(
                entry.messaged_at.is_some(),
                "{source:?} launches should stamp sidebar order"
            );
        }
    }

    #[test]
    fn to_manifest_entry_omits_messaged_at_for_bootstrap_rows() {
        let home = Path::new("/home/testuser");
        let spec = LaunchSpec {
            sessions_session_id: "ssn_bootstrap".into(),
            source: ManifestSource::WorkspaceBootstrap,
            cwd: "/tmp".into(),
            agent: "grok".into(),
            launch_command: "grok".into(),
            workspace_index: Some(0),
            focus: false,
            window_name: None,
            bootstrap_new_session: true,
            model_id: None,
            user_prompt: None,
        };
        assert!(spec.to_manifest_entry(home).messaged_at.is_none());
    }

    #[test]
    fn end_all_group_survives_reboot() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        let cwd = "/tmp/end-all-group";
        let closed_slot = bootstrap_sessions_session_id(0, cwd, "grok");
        let open_slot = bootstrap_sessions_session_id(1, cwd, "codex");

        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: closed_slot.clone(),
                source: ManifestSource::WorkspaceBootstrap,
                workspace_index: Some(0),
                cwd: cwd.into(),
                cwd_label: cwd.into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: None,
                title: Some("ended grok".into()),
                messaged_at: None,
                closed: true,
            },
        )
        .unwrap();
        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: open_slot.clone(),
                source: ManifestSource::WorkspaceBootstrap,
                workspace_index: Some(1),
                cwd: cwd.into(),
                cwd_label: cwd.into(),
                agent: "codex".into(),
                launch_command: "codex".into(),
                agent_session_id: None,
                title: Some("surviving codex".into()),
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();
        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_other_group".into(),
                source: ManifestSource::NewChat,
                workspace_index: None,
                cwd: "/tmp/other".into(),
                cwd_label: "/tmp/other".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: None,
                title: Some("other group".into()),
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let manifest = crate::session::manifest::load_manifest(&config).unwrap();
        assert!(crate::session::restore::workspace_bootstrap_closed(
            &manifest, 0, cwd, "grok"
        ));
        assert!(!crate::session::restore::workspace_bootstrap_closed(
            &manifest, 1, cwd, "codex"
        ));

        let pending = crate::session::restore::entries_needing_restore(&manifest, &HashSet::new());
        assert_eq!(pending.len(), 2);
        assert!(pending
            .iter()
            .any(|entry| entry.sessions_session_id == open_slot));
        assert!(pending
            .iter()
            .any(|entry| entry.sessions_session_id == "ssn_other_group"));
        assert!(!pending
            .iter()
            .any(|entry| entry.sessions_session_id == closed_slot));
    }

    #[test]
    fn close_within_one_second_cleans_artifacts() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        let session = test_session("tmux:win:3", 3, "ssn_close_test", "agent-close-1");
        crate::daemon::persist::save_state(&config, &[session.clone()], 1).unwrap();

        let record = ManagedLaunchRecord {
            sessions_session_id: "ssn_close_test".into(),
            launch_id: "lch_close".into(),
            agent: "grok".into(),
            tmux_session: "agents-nonexistent".into(),
            window_index: 3,
            pane_id: Some("%3".into()),
            initial_cwd: "/tmp".into(),
            created_at: Utc::now().to_rfc3339(),
            agent_session_id: Some("agent-close-1".into()),
        };
        save_managed_record(home, &record).unwrap();
        std::fs::write(
            config.session_env_path("agent-close-1"),
            "GROK_SESSION_ID=agent-close-1\n",
        )
        .unwrap();
        std::fs::write(config.session_title_path("agent-close-1"), "grok · test\n").unwrap();
        std::fs::write(config.session_title_path_for_tab(3), "grok · test\n").unwrap();
        std::fs::write(config.session_title_manual_path_for_tab(3), "1\n").unwrap();
        append_entry(
            &config,
            LaunchSpec {
                sessions_session_id: "ssn_close_test".into(),
                source: ManifestSource::Cli,
                cwd: "/tmp".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                workspace_index: None,
                focus: false,
                window_name: None,
                bootstrap_new_session: false,
                model_id: None,
                user_prompt: None,
            }
            .to_manifest_entry(home),
        )
        .unwrap();

        let outcome = close_unified(
            &config,
            CloseTarget {
                session_id: Some("tmux:win:3".into()),
                sessions_session_id: Some("ssn_close_test".into()),
                window_index: Some(3),
            },
        )
        .unwrap();

        assert_eq!(outcome.session_id, "tmux:win:3");
        assert_eq!(outcome.sessions_session_id.as_deref(), Some("ssn_close_test"));
        assert!(!crate::session::managed::managed_record_path(home, "ssn_close_test").exists());
        assert!(!config.session_env_path("agent-close-1").exists());
        assert!(!config.session_title_path("agent-close-1").exists());
        assert!(!config.session_title_path_for_tab(3).exists());
        assert!(!config.session_title_manual_path_for_tab(3).exists());

        let manifest = crate::session::manifest::load_manifest(&config).unwrap();
        let entry = manifest
            .entries
            .iter()
            .find(|entry| entry.sessions_session_id == "ssn_close_test")
            .expect("manifest entry");
        assert!(entry.closed, "close_unified must tombstone manifest entry");
    }
}