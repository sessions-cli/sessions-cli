use crate::agents::{
    build_quick_launch_command, build_resume_command, is_workspace_script, looks_like_shell_command,
};
use crate::config::Config;
use crate::daemon::persist::load_state_or_empty;
use crate::daemon::tmux;
use crate::model::Session;
use crate::pty::format_tilde_path;
use crate::session::lifecycle::{agent_for_launch_command, bootstrap_sessions_session_id};
use crate::session::managed::{load_managed_index, managed_record_path, ManagedLaunchRecord};
use crate::session::workspace::WorkspaceCatalog;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestSource {
    WorkspaceBootstrap,
    NewChat,
    InstantKey,
    Cli,
    Discovered,
    /// Scheduled or manual automation fire.
    Automation,
}

impl ManifestSource {
    /// User-initiated launches stamp sidebar order at creation; bootstrap/restore rows do not.
    pub fn stamps_order_on_launch(self) -> bool {
        matches!(
            self,
            Self::NewChat | Self::InstantKey | Self::Cli | Self::Automation
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionManifest {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_sessions_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_from: Option<String>,
    #[serde(default)]
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub sessions_session_id: String,
    pub source: ManifestSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_index: Option<u32>,
    pub cwd: String,
    pub cwd_label: String,
    #[serde(default)]
    pub agent: String,
    pub launch_command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub closed: bool,
}

pub fn manifest_path(home: &Path) -> PathBuf {
    crate::paths::state_dir(home).join("session-manifest.json")
}

pub fn load_manifest(config: &Config) -> Result<SessionManifest> {
    let path = config.session_manifest_path();
    if path.exists() {
        return read_manifest(&path);
    }
    migrate_if_empty(config)
}

/// Read an existing manifest without running migration — for daemon poll paths.
pub fn try_load_manifest(home: &Path) -> Option<SessionManifest> {
    let path = manifest_path(home);
    if !path.exists() {
        return None;
    }
    read_manifest(&path).ok()
}

pub fn manifest_entry_for_ssn<'a>(
    manifest: &'a SessionManifest,
    sessions_session_id: &str,
) -> Option<&'a ManifestEntry> {
    manifest
        .entries
        .iter()
        .find(|entry| entry.sessions_session_id == sessions_session_id && !entry.closed)
}

pub fn manifest_has_open_entry(manifest: &SessionManifest, sessions_session_id: &str) -> bool {
    manifest_entry_for_ssn(manifest, sessions_session_id).is_some()
}

/// Prompt-free `launch_command` for durable manifest storage (agent + model only).
pub fn normalize_launch_command_for_manifest(agent: &str, model: &str) -> String {
    crate::agents::build_launch_command(agent, model)
}

/// Resolve the `launch_command` value written to session-manifest.json.
///
/// **Contract:** stripping keys off `LaunchSpec::user_prompt`, not by parsing `launch_command`.
/// Create paths that embed prompt text in the tmux `launch_command` must also set `user_prompt`
/// (and `model_id`) on the spec. When `user_prompt` is unset, `launch_command` is stored verbatim
/// (workspace bootstrap, resume commands, instant keys).
pub fn manifest_launch_command_for_spec(spec: &crate::session::lifecycle::LaunchSpec) -> String {
    let has_user_prompt = spec
        .user_prompt
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty());
    if has_user_prompt && crate::agents::agent_accepts_cli_prompt(&spec.agent) {
        let model = spec
            .model_id
            .as_deref()
            .unwrap_or_else(|| crate::agents::default_model_id(&spec.agent));
        return normalize_launch_command_for_manifest(&spec.agent, model);
    }
    spec.launch_command.clone()
}

/// Seed poll/refresh state from durable manifest fields after window-index recycle.
pub fn hydrate_session_from_manifest(home: &Path, session: &mut Session, entry: &ManifestEntry) {
    session.managed = true;
    session.sessions_session_id = Some(entry.sessions_session_id.clone());
    if !entry.agent.is_empty() {
        session.managed_agent = Some(entry.agent.clone());
    }
    let manifest_agent_sid = entry.agent_session_id.as_deref();
    let manifest_sid_is_subagent = manifest_agent_sid
        .and_then(|sid| crate::agents::parent_session_id_for_subagent(home, sid))
        .is_some();
    if session.agent_session_id.is_none() && !manifest_sid_is_subagent {
        session.agent_session_id = entry.agent_session_id.clone();
    }
    if session.messaged_at.is_none() {
        session.messaged_at = entry.messaged_at;
    }
    if manifest_sid_is_subagent {
        return;
    }
    let manifest_matches_session = match (manifest_agent_sid, session.agent_session_id.as_deref()) {
        (None, _) | (_, None) => true,
        (Some(manifest_sid), Some(session_sid)) => manifest_sid == session_sid,
    };
    if !manifest_matches_session {
        return;
    }
    if let Some(ref title) = entry.title {
        let thread = crate::pty::parse_description(title);
        if crate::pty::is_sticky_thread_title(&thread)
            && (session.title.is_empty()
                || !crate::pty::is_sticky_thread_title(&session.description))
        {
            session.title = title.clone();
            session.description = thread;
            if !entry.agent.is_empty() {
                session.project = entry.agent.clone();
            }
        }
    }
}

pub fn save_manifest(config: &Config, manifest: &SessionManifest) -> Result<()> {
    atomic_write_manifest(&config.session_manifest_path(), manifest)
}

pub fn mark_entry_closed(config: &Config, sessions_session_id: &str) -> Result<()> {
    mark_entries_closed(config, &[sessions_session_id.to_string()]).map(|_| ())
}

/// Tombstone multiple open entries in one load/save cycle.
/// Returns how many entries flipped from open → closed.
pub fn mark_entries_closed(config: &Config, sessions_session_ids: &[String]) -> Result<usize> {
    if sessions_session_ids.is_empty() {
        return Ok(0);
    }
    let wanted: HashSet<&str> = sessions_session_ids.iter().map(String::as_str).collect();
    let mut manifest = load_manifest(config)?;
    let mut closed = 0usize;
    for entry in &mut manifest.entries {
        if entry.closed || !wanted.contains(entry.sessions_session_id.as_str()) {
            continue;
        }
        entry.closed = true;
        closed += 1;
    }
    if closed > 0 {
        save_manifest(config, &manifest)?;
    }
    Ok(closed)
}

/// Close every open manifest row whose sessions id is not in `live`.
///
/// Used by `sessions down` so restore only brings back what was actually live
/// before shutdown — not stale orphans left open after windows died outside close.
pub fn tombstone_open_entries_absent_from(
    config: &Config,
    live: &std::collections::HashSet<String>,
) -> Result<usize> {
    let mut manifest = load_manifest(config)?;
    let mut closed = 0usize;
    for entry in &mut manifest.entries {
        if entry.closed || live.contains(&entry.sessions_session_id) {
            continue;
        }
        entry.closed = true;
        closed += 1;
    }
    if closed > 0 {
        save_manifest(config, &manifest)?;
    }
    Ok(closed)
}

/// Clear a tombstone so cold-boot workspace seeding can recreate the slot.
pub fn reopen_manifest_entry(config: &Config, sessions_session_id: &str) -> Result<()> {
    let mut manifest = load_manifest(config)?;
    let Some(entry) = manifest
        .entries
        .iter_mut()
        .find(|entry| entry.sessions_session_id == sessions_session_id)
    else {
        return Ok(());
    };
    if !entry.closed {
        return Ok(());
    }
    entry.closed = false;
    save_manifest(config, &manifest)
}

pub fn append_entry(config: &Config, entry: ManifestEntry) -> Result<()> {
    let mut manifest = load_manifest(config)?;
    if let Some(existing) = manifest
        .entries
        .iter()
        .find(|existing| existing.sessions_session_id == entry.sessions_session_id)
    {
        if existing.closed {
            return Ok(());
        }
        return Ok(());
    }
    manifest.entries.push(entry);
    save_manifest(config, &manifest)
}

/// Fields back-synced from daemon hook notify events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestSyncPatch {
    pub agent_session_id: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub messaged_at: Option<DateTime<Utc>>,
}

fn should_overwrite_manifest_agent(entry_agent: &str) -> bool {
    entry_agent.is_empty() || entry_agent == "console" || entry_agent == "session"
}

fn should_preserve_manifest_title(entry: &ManifestEntry, daemon_title: &str) -> bool {
    let Some(ref existing) = entry.title else {
        return false;
    };
    let existing_thread = crate::pty::parse_description(existing);
    crate::pty::is_sticky_thread_title(&existing_thread)
        && crate::pty::is_weak_session_title(daemon_title)
}

fn apply_sync_patch_to_entry(
    home: &Path,
    entry: &mut ManifestEntry,
    patch: &ManifestSyncPatch,
) -> bool {
    let mut changed = false;
    if let Some(ref agent_session_id) = patch.agent_session_id {
        if crate::agents::parent_session_id_for_subagent(home, agent_session_id).is_some() {
            // Subagent hooks must not overwrite the managed slot's parent binding.
        } else if entry.agent_session_id.as_deref() != Some(agent_session_id.as_str()) {
            entry.agent_session_id = Some(agent_session_id.clone());
            changed = true;
        }
    }
    if let Some(ref agent) = patch.agent {
        if !agent.is_empty()
            && agent != "console"
            && should_overwrite_manifest_agent(&entry.agent)
            && entry.agent != *agent
        {
            entry.agent = agent.clone();
            changed = true;
        }
    }
    if let Some(ref title) = patch.title {
        if should_preserve_manifest_title(entry, title) {
            // Keep durable sticky manifest title over daemon placeholder (e.g. grok · ?).
        } else if entry.title.as_deref() != Some(title.as_str()) {
            entry.title = Some(title.clone());
            changed = true;
        }
    }
    if let Some(messaged_at) = patch.messaged_at {
        if entry.messaged_at != Some(messaged_at) {
            entry.messaged_at = Some(messaged_at);
            changed = true;
        }
    }
    changed
}

/// Apply debounced manifest patches in a single load/save cycle.
pub fn apply_manifest_sync_patches(
    config: &Config,
    patches: &std::collections::HashMap<String, ManifestSyncPatch>,
) -> Result<()> {
    if patches.is_empty() {
        return Ok(());
    }
    let mut manifest = load_manifest(config)?;
    let mut changed = false;
    for (sessions_session_id, patch) in patches {
        let Some(entry) = manifest
            .entries
            .iter_mut()
            .find(|entry| entry.sessions_session_id == *sessions_session_id)
        else {
            continue;
        };
        changed |= apply_sync_patch_to_entry(&config.home, entry, patch);
    }
    if changed {
        save_manifest(config, &manifest)?;
    }
    Ok(())
}

/// Batch production drain path for agent_session_id back-sync.
///
/// Called from `ManifestSyncQueue::flush` (debounced `manifest_persist_loop` and
/// `ClientCommand::FlushManifest`). Applies all updates in one load/save cycle.
pub fn update_manifest_agent_session_ids(
    config: &Config,
    updates: &[(String, String)],
) -> Result<()> {
    let mut patches = std::collections::HashMap::new();
    for (sessions_session_id, agent_session_id) in updates {
        patches.insert(
            sessions_session_id.clone(),
            ManifestSyncPatch {
                agent_session_id: Some(agent_session_id.clone()),
                agent: None,
                title: None,
                messaged_at: None,
            },
        );
    }
    apply_manifest_sync_patches(config, &patches)
}

/// Single-entry wrapper around [`update_manifest_agent_session_ids`].
///
/// The daemon uses the batch API directly; this remains the public single-update API for
/// CLI and future call sites.
pub fn update_entry_agent_session_id(
    config: &Config,
    sessions_session_id: &str,
    agent_session_id: &str,
) -> Result<()> {
    update_manifest_agent_session_ids(
        config,
        &[(
            sessions_session_id.to_string(),
            agent_session_id.to_string(),
        )],
    )
}

/// Persist which managed session the user last focused.
///
/// Sidebar `focus_row` calls this directly (not debounced). That is acceptable:
/// focus is a rare user action, this updates a single manifest field, and the
/// call is a no-op when `last_active_sessions_session_id` is already set.
pub fn update_last_active(config: &Config, sessions_session_id: &str) -> Result<()> {
    let mut manifest = load_manifest(config)?;
    if manifest.last_active_sessions_session_id.as_deref() == Some(sessions_session_id) {
        return Ok(());
    }
    manifest.last_active_sessions_session_id = Some(sessions_session_id.to_string());
    save_manifest(config, &manifest)
}

/// Down-path flush: apply daemon session snapshot fields onto open manifest entries.
pub fn sync_manifest_from_daemon_snapshot(config: &Config, sessions: &[Session]) -> Result<()> {
    let mut patches = std::collections::HashMap::new();
    let mut last_active: Option<String> = None;
    for session in sessions {
        if !session.managed {
            continue;
        }
        let Some(ref sessions_session_id) = session.sessions_session_id else {
            continue;
        };
        patches.insert(
            sessions_session_id.clone(),
            ManifestSyncPatch {
                agent_session_id: session.agent_session_id.clone(),
                agent: session
                    .managed_agent
                    .clone()
                    .filter(|agent| agent != "console"),
                title: (!session.title.is_empty()).then(|| session.title.clone()),
                messaged_at: session.messaged_at,
            },
        );
        if session.is_active {
            last_active = Some(sessions_session_id.clone());
        }
    }
    if patches.is_empty() && last_active.is_none() {
        return Ok(());
    }
    let mut manifest = load_manifest(config)?;
    let mut changed = false;
    for (sessions_session_id, patch) in &patches {
        let Some(entry) = manifest
            .entries
            .iter_mut()
            .find(|entry| entry.sessions_session_id == *sessions_session_id && !entry.closed)
        else {
            continue;
        };
        changed |= apply_sync_patch_to_entry(&config.home, entry, patch);
    }
    if let Some(ref sessions_session_id) = last_active {
        if manifest.last_active_sessions_session_id.as_deref() != Some(sessions_session_id.as_str())
        {
            manifest.last_active_sessions_session_id = Some(sessions_session_id.clone());
            changed = true;
        }
    }
    if changed {
        save_manifest(config, &manifest)?;
    }
    Ok(())
}

fn read_manifest(path: &Path) -> Result<SessionManifest> {
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut manifest: SessionManifest =
        serde_json::from_str(&data).with_context(|| format!("parse {}", path.display()))?;
    if manifest.version == 0 {
        manifest.version = MANIFEST_VERSION;
    }
    Ok(manifest)
}

fn migrate_if_empty(config: &Config) -> Result<SessionManifest> {
    let state = load_state_or_empty(config);
    let workspaces = WorkspaceCatalog::load(&config.workspaces_path);
    let managed_index = load_managed_index(&config.home);
    let live_windows = tmux::list_windows(&config.tmux_session)
        .map(|windows| {
            windows
                .into_iter()
                .map(|window| window.index)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    gc_stale_managed_records(&config.home, &managed_index, &live_windows);

    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for session in &state.sessions {
        let Some(entry) = manifest_entry_from_session(
            session,
            &workspaces,
            &managed_index,
            &config.home,
            &config.tmux_session,
        ) else {
            continue;
        };
        if seen.insert(entry.sessions_session_id.clone()) {
            entries.push(entry);
        }
    }

    let manifest = SessionManifest {
        version: MANIFEST_VERSION,
        last_active_sessions_session_id: state
            .sessions
            .iter()
            .find(|session| session.is_active)
            .and_then(|session| session.sessions_session_id.clone()),
        migrated_from: if entries.is_empty() && state.sessions.is_empty() {
            None
        } else {
            Some("sessionsd".into())
        },
        entries,
    };
    save_manifest(config, &manifest)?;
    Ok(manifest)
}

fn manifest_entry_from_session(
    session: &Session,
    workspaces: &WorkspaceCatalog,
    managed_index: &crate::session::managed::ManagedLaunchIndex,
    home: &Path,
    tmux_session: &str,
) -> Option<ManifestEntry> {
    let sessions_session_id = session
        .sessions_session_id
        .clone()
        .or_else(|| infer_sessions_session_id(session, workspaces, managed_index, tmux_session))?;

    let managed = managed_index.for_window(tmux_session, session.tab_index);
    let workspace_index = sessions_session_id
        .strip_prefix("ws:")
        .and_then(|rest| rest.split(':').next())
        .and_then(|index| index.parse().ok())
        .or_else(|| {
            workspaces
                .entry_for_window_index(session.tab_index)
                .map(|_| session.tab_index.saturating_sub(1))
        });

    let launch_command = infer_launch_command(session, workspaces, managed);
    let agent = session
        .managed_agent
        .clone()
        .filter(|agent| !agent.is_empty())
        .or_else(|| managed.map(|record| record.agent.clone()))
        .unwrap_or_else(|| agent_for_launch_command(&launch_command));

    let source = if sessions_session_id.starts_with("ws:") {
        ManifestSource::WorkspaceBootstrap
    } else if session.managed {
        ManifestSource::Cli
    } else {
        ManifestSource::Discovered
    };

    Some(ManifestEntry {
        sessions_session_id,
        source,
        workspace_index,
        cwd: session.cwd.clone(),
        cwd_label: if session.cwd_label.is_empty() {
            format_tilde_path(&session.cwd, home)
        } else {
            session.cwd_label.clone()
        },
        agent,
        launch_command,
        agent_session_id: session
            .agent_session_id
            .clone()
            .or_else(|| managed.and_then(|record| record.agent_session_id.clone())),
        title: (!session.title.is_empty()).then_some(session.title.clone()),
        messaged_at: session.messaged_at,
        closed: false,
    })
}

fn infer_sessions_session_id(
    session: &Session,
    workspaces: &WorkspaceCatalog,
    managed_index: &crate::session::managed::ManagedLaunchIndex,
    tmux_session: &str,
) -> Option<String> {
    if let Some(record) = managed_index.for_window(tmux_session, session.tab_index) {
        return Some(record.sessions_session_id.clone());
    }
    if let Some(workspace) = workspaces.entry_for_window_index(session.tab_index) {
        if workspace.cwd.trim_end_matches('/') == session.cwd.trim_end_matches('/') {
            return Some(bootstrap_sessions_session_id(
                session.tab_index.saturating_sub(1),
                &workspace.cwd,
                &workspace.command,
            ));
        }
    }
    None
}

fn infer_launch_command(
    session: &Session,
    workspaces: &WorkspaceCatalog,
    managed: Option<&ManagedLaunchRecord>,
) -> String {
    if let Some(command) = workspaces.bootstrap_command_for_window(session.tab_index, &session.cwd)
    {
        return command.to_string();
    }
    let agent = managed.map(|record| record.agent.as_str()).or_else(|| {
        session
            .managed_agent
            .as_deref()
            .filter(|agent| !agent.is_empty())
    });
    match agent {
        None | Some("console") => String::new(),
        Some(agent) => build_quick_launch_command(agent),
    }
}

pub const STALE_ORPHAN_HOURS: i64 = 24;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ManifestRepairReport {
    pub tombstoned: Vec<String>,
    pub launch_commands_rewritten: Vec<String>,
    pub agent_session_ids_backfilled: Vec<String>,
}

/// Opt-in manifest repair: tombstone stale orphans and fix corrupted launch commands.
pub fn repair_manifest(config: &Config) -> Result<ManifestRepairReport> {
    if !config.session_manifest_path().exists() {
        return Ok(ManifestRepairReport::default());
    }

    let mut manifest = load_manifest(config)?;
    let managed_index = load_managed_index(&config.home);
    let sessionsd_agent_ids = agent_session_ids_from_sessionsd(config);
    let live_ssns = if tmux::session_exists(&config.tmux_session) {
        tmux::list_live_sessions_session_ids(&config.tmux_session).unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };
    let cutoff = Utc::now() - chrono::Duration::hours(STALE_ORPHAN_HOURS);
    let mut report = ManifestRepairReport::default();
    let mut changed = false;

    for entry in &mut manifest.entries {
        if entry.closed {
            continue;
        }

        if entry.agent_session_id.is_none() {
            if let Some(agent_session_id) = sessionsd_agent_ids.get(&entry.sessions_session_id) {
                entry.agent_session_id = Some(agent_session_id.clone());
                report
                    .agent_session_ids_backfilled
                    .push(entry.sessions_session_id.clone());
                changed = true;
            }
        }

        if launch_command_needs_shell(entry) && !looks_like_shell_command(&entry.launch_command) {
            entry.launch_command = if let Some(ref agent_session_id) = entry.agent_session_id {
                let hint = build_quick_launch_command(&entry.agent);
                build_resume_command(&entry.agent, &hint, agent_session_id)
            } else {
                build_quick_launch_command(&entry.agent)
            };
            report
                .launch_commands_rewritten
                .push(entry.sessions_session_id.clone());
            changed = true;
        }

        if live_ssns.contains_key(&entry.sessions_session_id) {
            continue;
        }
        let managed = managed_index.for_ssn(&entry.sessions_session_id);
        let anchor = entry_stale_anchor(entry, managed);
        if anchor < cutoff {
            entry.closed = true;
            report.tombstoned.push(entry.sessions_session_id.clone());
            changed = true;
        }
    }

    if changed {
        save_manifest(config, &manifest)?;
    }
    Ok(report)
}

fn agent_session_ids_from_sessionsd(config: &Config) -> std::collections::HashMap<String, String> {
    load_state_or_empty(config)
        .sessions
        .into_iter()
        .filter_map(|session| {
            let sessions_session_id = session.sessions_session_id?;
            let agent_session_id = session.agent_session_id?;
            Some((sessions_session_id, agent_session_id))
        })
        .collect()
}

pub fn launch_command_needs_shell(entry: &ManifestEntry) -> bool {
    if entry.agent == "console" {
        return false;
    }
    !is_workspace_script(&entry.launch_command)
}

fn entry_stale_anchor(
    entry: &ManifestEntry,
    managed: Option<&ManagedLaunchRecord>,
) -> DateTime<Utc> {
    if let Some(messaged_at) = entry.messaged_at {
        return messaged_at;
    }
    if let Some(record) = managed {
        if let Ok(parsed) = DateTime::parse_from_rfc3339(&record.created_at) {
            return parsed.with_timezone(&Utc);
        }
    }
    DateTime::from_timestamp(0, 0).unwrap_or_else(Utc::now)
}

fn gc_stale_managed_records(
    home: &Path,
    managed_index: &crate::session::managed::ManagedLaunchIndex,
    live_windows: &std::collections::HashSet<u32>,
) {
    let dir = crate::session::managed::managed_state_dir(home);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<ManagedLaunchRecord>(&data) else {
            continue;
        };
        if live_windows.is_empty() || live_windows.contains(&record.window_index) {
            continue;
        }
        let _ = fs::remove_file(managed_record_path(home, &record.sessions_session_id));
        let _ = managed_index; // index is read-only snapshot for migration
    }
}

fn atomic_write_manifest(path: &Path, manifest: &SessionManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let data = serde_json::to_string_pretty(manifest)?;
    {
        let mut file = File::create(&tmp)?;
        file.write_all(data.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentState;
    use crate::session::lifecycle::LaunchSpec;
    use chrono::Utc;
    use tempfile::TempDir;

    fn test_config(home: &Path) -> Config {
        let mut config = Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.workspaces_path = home.join("workspaces.toml");
        config.tmux_session = "agents-nonexistent".into();
        config
    }

    fn sample_entry(ssn: &str, workspace_index: Option<u32>, cwd: &str) -> ManifestEntry {
        ManifestEntry {
            sessions_session_id: ssn.into(),
            source: ManifestSource::WorkspaceBootstrap,
            workspace_index,
            cwd: cwd.into(),
            cwd_label: cwd.into(),
            agent: "grok".into(),
            launch_command: "grok".into(),
            agent_session_id: None,
            title: Some("grok · test".into()),
            messaged_at: None,
            closed: false,
        }
    }

    #[test]
    fn hydrate_session_from_manifest_preserves_sticky_title_and_messaged_at() {
        let dir = TempDir::new().unwrap();
        let mut session = Session::default();
        session.title = "grok · ?".into();
        session.description = "?".into();
        let messaged_at = Utc::now() - chrono::Duration::hours(3);
        let entry = ManifestEntry {
            sessions_session_id: "ssn_hydrate".into(),
            source: ManifestSource::NewChat,
            workspace_index: None,
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            agent: "grok".into(),
            launch_command: "grok".into(),
            agent_session_id: Some("agent-hydrate".into()),
            title: Some("grok · sticky title".into()),
            messaged_at: Some(messaged_at),
            closed: false,
        };

        hydrate_session_from_manifest(dir.path(), &mut session, &entry);

        assert_eq!(session.messaged_at, Some(messaged_at));
        assert_eq!(session.title, "grok · sticky title");
        assert_eq!(session.description, "sticky title");
        assert_eq!(session.agent_session_id.as_deref(), Some("agent-hydrate"));
        assert!(session.managed);
        assert_eq!(session.sessions_session_id.as_deref(), Some("ssn_hydrate"));
    }

    #[test]
    fn manifest_roundtrip_persists_entries() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let entry = sample_entry("ssn_roundtrip", None, "/tmp/project");
        append_entry(&config, entry.clone()).unwrap();

        let loaded = load_manifest(&config).unwrap();
        assert_eq!(loaded.version, MANIFEST_VERSION);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0], entry);
    }

    #[test]
    fn mark_entry_closed_sets_tombstone() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_close", None, "/tmp")).unwrap();

        mark_entry_closed(&config, "ssn_close").unwrap();

        let loaded = load_manifest(&config).unwrap();
        assert!(loaded.entries[0].closed);
    }

    #[test]
    fn tombstone_open_entries_absent_from_keeps_live_only() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_live", None, "/tmp/live")).unwrap();
        append_entry(&config, sample_entry("ssn_orphan", None, "/tmp/orphan")).unwrap();

        let live = HashSet::from(["ssn_live".to_string()]);
        let closed = tombstone_open_entries_absent_from(&config, &live).unwrap();
        assert_eq!(closed, 1);

        let loaded = load_manifest(&config).unwrap();
        let by_id: std::collections::HashMap<_, _> = loaded
            .entries
            .iter()
            .map(|entry| (entry.sessions_session_id.as_str(), entry.closed))
            .collect();
        assert_eq!(by_id.get("ssn_live"), Some(&false));
        assert_eq!(by_id.get("ssn_orphan"), Some(&true));
    }

    #[test]
    fn append_entry_skips_closed_existing_slot() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let mut entry = sample_entry("ws:0:abc", Some(0), "/shared");
        append_entry(&config, entry.clone()).unwrap();
        mark_entry_closed(&config, "ws:0:abc").unwrap();

        entry.title = Some("should not reopen".into());
        append_entry(&config, entry).unwrap();

        let loaded = load_manifest(&config).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.entries[0].closed);
        assert_eq!(loaded.entries[0].title.as_deref(), Some("grok · test"));
    }

    #[test]
    fn same_cwd_two_workspace_slots() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let cwd = "/home/testuser/projects/shared";
        let slot0 = bootstrap_sessions_session_id(0, cwd, "grok");
        let slot1 = bootstrap_sessions_session_id(1, cwd, "codex");

        append_entry(&config, sample_entry(&slot0, Some(0), cwd)).unwrap();
        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: slot1.clone(),
                source: ManifestSource::WorkspaceBootstrap,
                workspace_index: Some(1),
                cwd: cwd.into(),
                cwd_label: cwd.into(),
                agent: "codex".into(),
                launch_command: "codex".into(),
                ..sample_entry("unused", Some(1), cwd)
            },
        )
        .unwrap();

        mark_entry_closed(&config, &slot0).unwrap();

        let loaded = load_manifest(&config).unwrap();
        let slot0_entry = loaded
            .entries
            .iter()
            .find(|entry| entry.sessions_session_id == slot0)
            .unwrap();
        let slot1_entry = loaded
            .entries
            .iter()
            .find(|entry| entry.sessions_session_id == slot1)
            .unwrap();
        assert!(slot0_entry.closed);
        assert!(!slot1_entry.closed);
    }

    #[test]
    fn migrate_infer_launch_command_uses_quick_launch_not_description() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        fs::create_dir_all(crate::paths::state_dir(dir.path())).unwrap();
        let session = Session {
            id: "tmux:win:2".into(),
            kitty_window_id: 2,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 2,
            tmux_session: "agents".into(),
            tmux_pane_id: "%2".into(),
            pane_pid: 0,
            agent_session_id: Some("agent-title-drift".into()),
            title: "grok · sticky thread".into(),
            description: "sticky thread".into(),
            cwd: "/tmp/drift".into(),
            cwd_label: "~/tmp/drift".into(),
            project: "grok".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now()),
            prompt_submitted: false,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            managed: true,
            sessions_session_id: Some("ssn_title_drift".into()),
            managed_agent: Some("grok".into()),
        };
        crate::daemon::persist::save_state(&config, &[session], 1).unwrap();

        let manifest = load_manifest(&config).unwrap();
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].launch_command, "grok");
        assert_eq!(
            manifest.entries[0].title.as_deref(),
            Some("grok · sticky thread")
        );
    }

    #[test]
    fn migrate_from_sessionsd_seeds_entries() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        fs::create_dir_all(crate::paths::state_dir(dir.path())).unwrap();
        let session = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: "agents".into(),
            tmux_pane_id: "%1".into(),
            pane_pid: 0,
            agent_session_id: Some("agent-1".into()),
            title: "grok · seeded".into(),
            description: "grok".into(),
            cwd: "/tmp/project".into(),
            cwd_label: "~/tmp/project".into(),
            project: "grok".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now()),
            prompt_submitted: false,
            title_manual: false,
            is_active: true,
            last_event_at: Utc::now(),
            managed: true,
            sessions_session_id: Some("ssn_migrated".into()),
            managed_agent: Some("grok".into()),
        };
        crate::daemon::persist::save_state(&config, &[session], 1).unwrap();

        let manifest = load_manifest(&config).unwrap();
        assert_eq!(manifest.migrated_from.as_deref(), Some("sessionsd"));
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].sessions_session_id, "ssn_migrated");
        assert_eq!(
            manifest.last_active_sessions_session_id.as_deref(),
            Some("ssn_migrated")
        );
        assert!(manifest_path(dir.path()).exists());
    }

    #[test]
    fn update_entry_agent_session_id_no_op_when_unchanged() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let mut entry = sample_entry("ssn_noop", None, "/tmp");
        entry.agent_session_id = Some("agent-unchanged".into());
        append_entry(&config, entry).unwrap();
        update_entry_agent_session_id(&config, "ssn_noop", "agent-unchanged").unwrap();

        let before = fs::metadata(manifest_path(dir.path()))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        update_entry_agent_session_id(&config, "ssn_noop", "agent-unchanged").unwrap();
        let after = fs::metadata(manifest_path(dir.path()))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn repair_rewrites_corrupted_launch_command_to_quick_launch_without_agent_session_id() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let mut entry = sample_entry("ssn_repair_quick", None, "/tmp");
        entry.launch_command = "grok · sticky thread".into();
        append_entry(&config, entry).unwrap();

        let report = repair_manifest(&config).unwrap();
        assert_eq!(report.launch_commands_rewritten, vec!["ssn_repair_quick"]);
        assert_eq!(
            load_manifest(&config).unwrap().entries[0].launch_command,
            "grok"
        );
    }

    #[test]
    fn repair_backfills_agent_session_id_from_sessionsd() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        fs::create_dir_all(crate::paths::state_dir(dir.path())).unwrap();
        append_entry(&config, sample_entry("ssn_backfill", None, "/tmp")).unwrap();
        let session = Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: "agents-nonexistent".into(),
            tmux_pane_id: "%1".into(),
            pane_pid: 0,
            agent_session_id: Some("agent-backfill".into()),
            title: "grok · backfill".into(),
            description: "grok".into(),
            cwd: "/tmp".into(),
            cwd_label: "/tmp".into(),
            project: "grok".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: true,
            last_event_at: Utc::now(),
            managed: true,
            sessions_session_id: Some("ssn_backfill".into()),
            managed_agent: Some("grok".into()),
        };
        crate::daemon::persist::save_state(&config, &[session], 1).unwrap();

        let report = repair_manifest(&config).unwrap();
        assert_eq!(report.agent_session_ids_backfilled, vec!["ssn_backfill"]);
        assert_eq!(
            load_manifest(&config).unwrap().entries[0]
                .agent_session_id
                .as_deref(),
            Some("agent-backfill")
        );
    }

    #[test]
    fn repair_tombstones_orphan_without_timestamp_anchor() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_epoch_orphan", None, "/tmp")).unwrap();

        let report = repair_manifest(&config).unwrap();
        assert_eq!(report.tombstoned, vec!["ssn_epoch_orphan"]);
        assert!(load_manifest(&config).unwrap().entries[0].closed);
    }

    #[test]
    fn repair_rewrites_corrupted_launch_command_to_resume_form() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let mut entry = sample_entry("ssn_repair_cmd", None, "/tmp");
        entry.launch_command = "grok · sticky thread".into();
        entry.agent_session_id = Some("agent-repair-cmd".into());
        append_entry(&config, entry).unwrap();

        let report = repair_manifest(&config).unwrap();
        assert_eq!(report.launch_commands_rewritten, vec!["ssn_repair_cmd"]);
        assert_eq!(
            load_manifest(&config).unwrap().entries[0].launch_command,
            "grok --resume agent-repair-cmd"
        );
    }

    #[test]
    fn repair_tombstones_open_entry_without_window_for_24h() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let mut entry = sample_entry("ssn_stale_orphan", None, "/tmp");
        entry.messaged_at = Some(Utc::now() - chrono::Duration::hours(25));
        append_entry(&config, entry).unwrap();

        let report = repair_manifest(&config).unwrap();
        assert_eq!(report.tombstoned, vec!["ssn_stale_orphan"]);
        assert!(load_manifest(&config).unwrap().entries[0].closed);
    }

    #[test]
    fn repair_skips_recent_orphan_without_live_window() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let mut entry = sample_entry("ssn_recent_orphan", None, "/tmp");
        entry.messaged_at = Some(Utc::now() - chrono::Duration::hours(2));
        append_entry(&config, entry).unwrap();

        let report = repair_manifest(&config).unwrap();
        assert!(report.tombstoned.is_empty());
        assert!(!load_manifest(&config).unwrap().entries[0].closed);
    }

    #[test]
    fn launch_spec_to_manifest_entry_preserves_resume_commands() {
        let home = Path::new("/home/testuser");
        let spec = LaunchSpec {
            sessions_session_id: "ssn_test".into(),
            source: ManifestSource::InstantKey,
            cwd: "/home/testuser/tmp/foo".into(),
            agent: "grok".into(),
            launch_command: "grok --resume uuid".into(),
            workspace_index: None,
            focus: true,
            window_name: None,
            bootstrap_new_session: false,
            model_id: None,
            user_prompt: None,
        };
        let entry = spec.to_manifest_entry(home);
        assert_eq!(entry.launch_command, "grok --resume uuid");
        assert_eq!(entry.cwd_label, "~/tmp/foo");
        assert!(!entry.closed);
    }

    #[test]
    fn normalize_launch_command_for_manifest_omits_user_prompt() {
        assert_eq!(
            normalize_launch_command_for_manifest("grok", "grok-composer-2.5-fast"),
            "grok --model grok-composer-2.5-fast"
        );
        assert_eq!(
            normalize_launch_command_for_manifest("opencode", "default"),
            "opencode"
        );
    }

    #[test]
    fn manifest_launch_command_for_spec_requires_user_prompt_field() {
        let prompt = "fix the sidebar";
        let with_prompt_field = LaunchSpec {
            sessions_session_id: "ssn_contract".into(),
            source: ManifestSource::NewChat,
            cwd: "/tmp".into(),
            agent: "grok".into(),
            launch_command: format!("grok --model grok-composer-2.5-fast '{prompt}'"),
            workspace_index: None,
            focus: false,
            window_name: None,
            bootstrap_new_session: false,
            model_id: Some("grok-composer-2.5-fast".into()),
            user_prompt: Some(prompt.into()),
        };
        assert_eq!(
            manifest_launch_command_for_spec(&with_prompt_field),
            "grok --model grok-composer-2.5-fast"
        );

        let without_prompt_field = LaunchSpec {
            user_prompt: None,
            ..with_prompt_field
        };
        assert_eq!(
            manifest_launch_command_for_spec(&without_prompt_field),
            format!("grok --model grok-composer-2.5-fast '{prompt}'")
        );
    }

    #[test]
    fn launch_spec_to_manifest_entry_strips_prompt_for_new_chat() {
        let home = Path::new("/home/testuser");
        let prompt = "fix the sidebar";
        let spec = LaunchSpec {
            sessions_session_id: "ssn_new_chat".into(),
            source: ManifestSource::NewChat,
            cwd: "/home/testuser/tmp/foo".into(),
            agent: "grok".into(),
            launch_command: format!("grok --model grok-composer-2.5-fast '{prompt}'"),
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

    fn mock_daemon_session(
        ssn: &str,
        agent_session_id: Option<&str>,
        title: &str,
        managed: bool,
        is_active: bool,
        managed_agent: Option<&str>,
    ) -> Session {
        Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: "agents".into(),
            tmux_pane_id: "%1".into(),
            pane_pid: 0,
            agent_session_id: agent_session_id.map(str::to_string),
            title: title.into(),
            description: title.into(),
            cwd: "/tmp".into(),
            cwd_label: "/tmp".into(),
            project: "grok".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now()),
            prompt_submitted: false,
            title_manual: false,
            is_active,
            last_event_at: Utc::now(),
            managed,
            sessions_session_id: Some(ssn.into()),
            managed_agent: managed_agent.map(str::to_string),
        }
    }
    #[test]
    fn t3_down_sync_writes_agent_session_id() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_t3", None, "/tmp")).unwrap();
        sync_manifest_from_daemon_snapshot(
            &config,
            &[mock_daemon_session(
                "ssn_t3",
                Some("agent-down-sync"),
                "grok · down sync",
                true,
                false,
                Some("grok"),
            )],
        )
        .unwrap();
        let loaded = load_manifest(&config).unwrap();
        let entry = loaded
            .entries
            .iter()
            .find(|e| e.sessions_session_id == "ssn_t3")
            .unwrap();
        assert_eq!(entry.agent_session_id.as_deref(), Some("agent-down-sync"));
        assert_eq!(entry.title.as_deref(), Some("grok · down sync"));
        assert!(entry.messaged_at.is_some());
    }
    #[test]
    fn t3_down_sync_writes_agent_field() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let mut entry = sample_entry("ssn_t3_agent", None, "/tmp");
        entry.agent = "console".into();
        append_entry(&config, entry).unwrap();

        sync_manifest_from_daemon_snapshot(
            &config,
            &[mock_daemon_session(
                "ssn_t3_agent",
                Some("agent-codex"),
                "codex · down sync",
                true,
                false,
                Some("codex"),
            )],
        )
        .unwrap();

        let loaded = load_manifest(&config).unwrap();
        let row = loaded
            .entries
            .iter()
            .find(|e| e.sessions_session_id == "ssn_t3_agent")
            .unwrap();
        assert_eq!(row.agent, "codex");
        assert_eq!(row.agent_session_id.as_deref(), Some("agent-codex"));
    }

    #[test]
    fn t12_down_sync_sets_last_active_from_active_session() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_idle", None, "/tmp/a")).unwrap();
        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_active".into(),
                ..sample_entry("ssn_active", None, "/tmp/b")
            },
        )
        .unwrap();
        sync_manifest_from_daemon_snapshot(
            &config,
            &[
                mock_daemon_session(
                    "ssn_idle",
                    Some("agent-idle"),
                    "grok · idle",
                    true,
                    false,
                    Some("grok"),
                ),
                mock_daemon_session(
                    "ssn_active",
                    Some("agent-active"),
                    "grok · active",
                    true,
                    true,
                    Some("grok"),
                ),
            ],
        )
        .unwrap();
        assert_eq!(
            load_manifest(&config)
                .unwrap()
                .last_active_sessions_session_id
                .as_deref(),
            Some("ssn_active")
        );
    }
    #[test]
    fn sync_manifest_preserves_sticky_title_over_daemon_placeholder() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let mut entry = sample_entry("ssn_sticky", None, "/tmp");
        entry.title = Some("grok · fix sidebar".into());
        append_entry(&config, entry).unwrap();

        sync_manifest_from_daemon_snapshot(
            &config,
            &[mock_daemon_session(
                "ssn_sticky",
                Some("agent-sticky"),
                "grok · ?",
                true,
                false,
                Some("grok"),
            )],
        )
        .unwrap();

        let loaded = load_manifest(&config).unwrap();
        let row = loaded
            .entries
            .iter()
            .find(|e| e.sessions_session_id == "ssn_sticky")
            .unwrap();
        assert_eq!(row.title.as_deref(), Some("grok · fix sidebar"));
        assert_eq!(row.agent_session_id.as_deref(), Some("agent-sticky"));
    }

    #[test]
    fn sync_manifest_falls_back_to_sessionsd_when_daemon_offline() {
        let dir = TempDir::new().unwrap();
        let mut config = test_config(dir.path());
        config.socket_path = dir.path().join("offline.sock");
        config.tmux_session = "agents-nonexistent".into();

        append_entry(&config, sample_entry("ssn_offline", None, "/tmp")).unwrap();
        let session = mock_daemon_session(
            "ssn_offline",
            Some("agent-from-sessionsd"),
            "grok · offline",
            true,
            false,
            Some("grok"),
        );
        crate::daemon::persist::save_state(&config, &[session], 1).unwrap();

        // Offline path mirrors `daemon_sessions_for_down_sync` fallback to sessionsd.json.
        let sessions = load_state_or_empty(&config).sessions;
        sync_manifest_from_daemon_snapshot(&config, &sessions).unwrap();

        let loaded = load_manifest(&config).unwrap();
        let row = loaded
            .entries
            .iter()
            .find(|e| e.sessions_session_id == "ssn_offline")
            .unwrap();
        assert_eq!(
            row.agent_session_id.as_deref(),
            Some("agent-from-sessionsd")
        );
    }

    fn sync_manifest_from_daemon_snapshot_skips_unmanaged_and_closed() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        append_entry(&config, sample_entry("ssn_closed", None, "/tmp")).unwrap();
        mark_entry_closed(&config, "ssn_closed").unwrap();
        sync_manifest_from_daemon_snapshot(
            &config,
            &[
                mock_daemon_session(
                    "ssn_closed",
                    Some("agent-should-not-apply"),
                    "grok · closed",
                    true,
                    false,
                    Some("grok"),
                ),
                mock_daemon_session(
                    "ssn_discovered",
                    Some("agent-discovered"),
                    "grok · discovered",
                    false,
                    true,
                    Some("grok"),
                ),
            ],
        )
        .unwrap();
        let loaded = load_manifest(&config).unwrap();
        assert!(loaded
            .entries
            .iter()
            .find(|e| e.sessions_session_id == "ssn_closed")
            .unwrap()
            .agent_session_id
            .is_none());
        assert_eq!(loaded.last_active_sessions_session_id, None);
    }
}
