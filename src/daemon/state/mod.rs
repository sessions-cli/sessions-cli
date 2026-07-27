use crate::config::Config;
use crate::daemon::manifest_sync::{ManifestPatch, ManifestSyncQueue};
use crate::daemon::tmux::{
    bootstrap_session, pane_to_window_index, play_alert_sound, poll_tmux, rename_window,
    save_pane_state, select_window, session_exists, write_session_env_tmux,
};
use crate::model::{AgentState, NotifyMessage, ServerEvent, Session};
use crate::session::load_session_env;
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

mod disk_sync;
mod identity;
mod merge;
mod notify;
mod sort;
mod store;
mod titles;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use {disk_sync::*, identity::*, merge::*, notify::*, sort::*, store::*, titles::*};

use disk_sync::hydrate_messaged_at;
use identity::agent_session_suppressions;
use merge::{compute_refresh, RefreshComputation};
use notify::{
    apply_thread_hook, hook_targets_session, initial_session_names, is_console_session,
    resolve_notify_cwd, resolve_window_index, update_session_cwd,
};
use sort::{resolve_focus_target, sorted_sessions};
use store::{closed_markers_from_manifest, sessions_changed, sessions_session_ids_to_tombstone};
use titles::{
    clear_manual_title_files_for_tab, persist_agent_session_title, resolve_renamed_title,
    write_manual_session_title_files,
};

use crate::notify::events::{
    event_to_state, marks_thread_complete, normalize_hook_event, rings_alert,
};
use crate::notify::payload::read_prompt_from_payload;
use crate::pty::{
    is_bootstrap_sidebar_thread, is_sticky_thread_title, is_weak_session_title,
    normalize_agent_label, resolve_session_names,
};
use crate::session::WorkspaceCatalog;
use notify::cwd_label_for_path;

#[derive(Debug)]
pub struct DaemonState {
    inner: Arc<RwLock<StateInner>>,
    config: Config,
    manifest_sync: Arc<ManifestSyncQueue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ClosedSessionMarker {
    /// Stable managed id — primary key for poll filtering after restart.
    sessions_session_id: Option<String>,
    /// Agent thread id — used only to ignore late hooks after close, not to
    /// hide unrelated live windows that happen to share a binding history.
    agent_session_id: Option<String>,
    tmux_pane_id: String,
}

impl ClosedSessionMarker {
    pub(crate) fn from_session(session: &Session) -> Self {
        Self {
            sessions_session_id: session.sessions_session_id.clone(),
            agent_session_id: session.agent_session_id.clone(),
            tmux_pane_id: session.tmux_pane_id.clone(),
        }
    }

    /// Whether a polled/live row should be suppressed as closed.
    ///
    /// Match only by stable `sessions_session_id` or the closed pane — never by
    /// `agent_session_id` alone. Agent SIDs can be rebound or collide across
    /// rows; matching them permanently hid live windows after false tombstones.
    pub(crate) fn matches(&self, session: &Session) -> bool {
        if let Some(ref ssn) = self.sessions_session_id {
            if session.sessions_session_id.as_deref() == Some(ssn.as_str()) {
                return true;
            }
        }
        if !self.tmux_pane_id.is_empty() && self.tmux_pane_id == session.tmux_pane_id {
            return true;
        }
        false
    }

    pub(crate) fn matches_notify(&self, agent_session_id: Option<&str>, pane_id: &str) -> bool {
        if !self.tmux_pane_id.is_empty() && !pane_id.is_empty() && self.tmux_pane_id == pane_id {
            return true;
        }
        if let Some(sid) = agent_session_id {
            if self.agent_session_id.as_deref() == Some(sid) {
                return true;
            }
        }
        false
    }
}

#[derive(Debug)]
struct StateInner {
    sessions: HashMap<String, Session>,
    closed_sessions: HashSet<ClosedSessionMarker>,
    /// Session ids whose agent binding is hidden from snapshots (stale pane
    /// binding or lost a same-session-id collision). Recomputed at poll time
    /// off the lock-holding path so `sorted_sessions` stays I/O-free.
    agent_suppressions: HashSet<String>,
    /// Blocks poll prune until `sessions up` finishes manifest restore.
    booting: bool,
    version: u64,
    last_poll_at: Option<chrono::DateTime<Utc>>,
    dirty: bool,
}

#[derive(Debug, Default)]
struct NotifySideEffects {
    pane_state: Option<(String, AgentState)>,
    session_env: Option<SessionEnvWrite>,
    session_title: Option<(String, String)>,
    rename_window: Option<(String, u32, String)>,
    /// One-shot system sound — played off the state lock on the daemon so the
    /// bell is not lost when the sidebar coalesces/drops a `ring_bell` patch.
    ring_alert: bool,
}

#[derive(Debug)]
struct SessionEnvWrite {
    agent_session_id: String,
    tmux_pane_id: Option<String>,
    window_index: u32,
    tmux_session: String,
    sessions_session_id: Option<String>,
    managed_agent: Option<String>,
}

impl DaemonState {
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn new(
        config: Config,
        restored: Vec<Session>,
        manifest_sync: Arc<ManifestSyncQueue>,
    ) -> Self {
        // Always start booting until an explicit reconcile (RestoreComplete or
        // auto-reconcile when agents is already live). Prevents partial poll
        // snapshots from streaming into the sidebar during restore/reload.
        let booting = true;
        let closed_sessions = closed_markers_from_manifest(&config);
        let mut map = HashMap::new();
        for mut s in restored {
            if s.state == AgentState::Done && s.completed_thread.is_none() {
                s.completed_thread = Some(s.description.clone());
            }
            hydrate_messaged_at(&mut s, &config);
            map.insert(s.id.clone(), s);
        }
        // Seed suppressions once at startup so the first snapshot (served before
        // the initial poll) is already de-duplicated.
        let restored: Vec<Session> = map.values().cloned().collect();
        let agent_suppressions = agent_session_suppressions(&config, &restored);
        Self {
            inner: Arc::new(RwLock::new(StateInner {
                sessions: map,
                closed_sessions,
                agent_suppressions,
                booting,
                version: 0,
                last_poll_at: None,
                dirty: false,
            })),
            config,
            manifest_sync,
        }
    }

    pub fn manifest_sync(&self) -> &Arc<ManifestSyncQueue> {
        &self.manifest_sync
    }

    pub async fn restore_complete(&self) {
        self.inner.write().await.booting = false;
    }

    pub async fn set_booting(&self, value: bool) {
        self.inner.write().await.booting = value;
    }

    pub async fn is_booting(&self) -> bool {
        self.inner.read().await.booting
    }

    pub async fn session_list(&self) -> Vec<Session> {
        let inner = self.inner.read().await;
        sorted_sessions(inner.sessions.values(), &inner.agent_suppressions)
    }

    /// Unique cwd paths for warm-pool maintain (one spare per active group).
    pub async fn live_cwd_labels(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        let mut cwds: Vec<String> = inner
            .sessions
            .values()
            .map(|s| s.cwd.clone())
            .filter(|c| !c.is_empty())
            .collect();
        cwds.sort();
        cwds.dedup();
        cwds
    }

    pub async fn version(&self) -> u64 {
        self.inner.read().await.version
    }

    pub async fn session_count(&self) -> usize {
        self.inner.read().await.sessions.len()
    }

    pub async fn last_poll_at(&self) -> Option<chrono::DateTime<Utc>> {
        self.inner.read().await.last_poll_at
    }

    pub async fn is_dirty(&self) -> bool {
        self.inner.read().await.dirty
    }

    pub async fn clear_dirty(&self) {
        self.inner.write().await.dirty = false;
    }

    pub async fn handle_notify(&self, msg: &NotifyMessage) -> Option<ServerEvent> {
        let started = std::time::Instant::now();
        let result = self.handle_notify_inner(msg).await;
        crate::daemon::metrics::record_hook_apply(started.elapsed().as_micros() as u64);
        if result.is_some() {
            crate::daemon::metrics::record_notify_applied();
        }
        result
    }

    async fn handle_notify_inner(&self, msg: &NotifyMessage) -> Option<ServerEvent> {
        let event = normalize_hook_event(&msg.event);
        let window_index = resolve_window_index(msg, &self.config)?;
        let pane_id = msg.tmux_pane_id.clone().unwrap_or_default();
        let id = Session::session_id_from_window(window_index);
        let state = event_to_state(event)?;
        if event == "session_start" {
            if let Some(ref sid) = msg.session_id {
                crate::agents::invalidate_agent_id_cache(sid);
            }
        }
        let workspaces = WorkspaceCatalog::load(&self.config.workspaces_path);
        let tmux_session = msg
            .tmux_session
            .clone()
            .unwrap_or_else(|| self.config.tmux_session.clone());
        let env_window_index = if pane_id.is_empty() {
            window_index
        } else if let Some(idx) = msg
            .kitty_window_id
            .and_then(|v| u32::try_from(v).ok())
            .filter(|&v| v > 0)
        {
            idx
        } else if let Some(ref sid) = msg.session_id {
            load_session_env(&self.config.session_env_path(sid))
                .window_index
                .unwrap_or(window_index)
        } else {
            pane_to_window_index(&tmux_session, &pane_id).unwrap_or(window_index)
        };

        let (patch, side_effects, manifest_patch) = {
            let mut inner = self.inner.write().await;
            let existing = inner.sessions.get(&id).cloned();
            if inner
                .closed_sessions
                .iter()
                .any(|marker| marker.matches_notify(msg.session_id.as_deref(), &pane_id))
            {
                debug!("notify ignored for {event} on {id}: session was closed");
                return None;
            }
            if let Some(ref entry) = existing {
                if !hook_targets_session(entry, event, msg, &pane_id, &self.config.home) {
                    debug!(
                        "notify ignored for {} on {}: hook target mismatch",
                        event, id
                    );
                    return None;
                }
            } else if matches!(
                event,
                "stop"
                    | "turn_complete"
                    | "pre_tool"
                    | "post_tool"
                    | "tool_fail"
                    | "approval_required"
            ) {
                return None;
            }

            let entry = inner.sessions.entry(id.clone()).or_insert_with(|| {
                let cwd = resolve_notify_cwd(
                    msg.cwd.as_deref(),
                    &pane_id,
                    &tmux_session,
                    &self.config.home,
                );
                let cwd_label = cwd_label_for_path(&cwd, &self.config.home);
                let (title, description, project) =
                    initial_session_names(&workspaces, window_index, &cwd, &self.config.home);
                Session {
                    id: id.clone(),
                    kitty_window_id: window_index as u64,
                    kitty_tab_id: 0,
                    kitty_os_window_id: 0,
                    tab_index: window_index,
                    tmux_session: self.config.tmux_session.clone(),
                    tmux_pane_id: pane_id.clone(),
                    pane_pid: 0,
                    agent_session_id: msg.session_id.clone(),
                    title,
                    description,
                    cwd: cwd.clone(),
                    cwd_label,
                    project,
                    state: AgentState::Idle,
                    completed_thread: None,
                    completed_at: None,
                    messaged_at: Some(Utc::now()),
                    prompt_submitted: false,
                    title_manual: false,
                    is_active: false,
                    last_event_at: Utc::now(),
                    managed: false,
                    sessions_session_id: None,
                    managed_agent: None,
                }
            });
            let was_complete = entry.thread_is_complete();
            let mut side_effects = NotifySideEffects::default();

            if !pane_id.is_empty() {
                entry.tmux_pane_id = pane_id.clone();
            }
            if let Some(ref ts) = msg.tmux_session {
                entry.tmux_session = ts.clone();
            } else if entry.tmux_session.is_empty() {
                entry.tmux_session = self.config.tmux_session.clone();
            }
            if let Some(ref cwd) = msg.cwd {
                update_session_cwd(entry, cwd, &self.config.home);
            }

            let prior_agent_sid = entry.agent_session_id.clone();

            if let Some(ref sid) = msg.session_id {
                let can_bind_prompt = event == "prompt"
                    && !is_console_session(entry)
                    && (entry.agent_session_id.is_none()
                        // Replace a cross-agent poison pill (e.g. Grok UUID on
                        // an OpenCode managed row) so the real thread binds.
                        || entry.managed_agent.as_deref().is_some_and(|agent| {
                            entry.agent_session_id.as_deref().is_some_and(|bound| {
                                !crate::agents::agent_session_matches_expected_agent(
                                    &self.config.home,
                                    bound,
                                    agent,
                                )
                            })
                        })
                        // Same-agent rebind (new OpenCode/Claude thread in pane).
                        || (msg.agent.as_deref().is_some_and(|a| {
                            entry
                                .managed_agent
                                .as_deref()
                                .is_some_and(|m| m.eq_ignore_ascii_case(a))
                        })));
                if matches!(event, "session_start" | "stop" | "turn_complete") || can_bind_prompt {
                    entry.agent_session_id = Some(sid.clone());
                }
            }

            if let Some(ref ssn_id) = msg.sessions_session_id {
                entry.managed = true;
                entry.sessions_session_id = Some(ssn_id.clone());
                if let Some(agent) = msg.agent.as_ref().or(entry.managed_agent.as_ref()) {
                    entry.managed_agent = Some(agent.clone());
                }
                if let Some(ref sid) = msg.session_id {
                    let _ = crate::session::update_managed_agent_session_id(
                        &self.config.home,
                        ssn_id,
                        sid,
                    );
                }
            }

            // After binding, recompute group key from the agent thread project
            // (or fall back to pane cwd when unbound) so a repaired SID does not
            // leave the row under a hijacked project's group.
            if let Some(ref sid) = entry.agent_session_id {
                entry.cwd_label =
                    crate::agents::group_cwd_for_session(&self.config.home, &entry.cwd, Some(sid));
            } else if !entry.cwd.is_empty() {
                entry.cwd_label = crate::pty::format_tilde_path(&entry.cwd, &self.config.home);
            }

            let agent_session_rotated = event == "session_start"
                && prior_agent_sid
                    .as_deref()
                    .zip(entry.agent_session_id.as_deref())
                    .is_some_and(|(old, new)| old != new);

            if (event == "session_start" || event == "prompt") && !entry.title_manual {
                let prompt = read_prompt_from_payload(&msg.payload);
                let workspace = workspaces.workspace_ref_for_window(window_index, &entry.cwd);
                let grok_sid = entry
                    .agent_session_id
                    .clone()
                    .or_else(|| msg.session_id.clone());
                let prior_title = if agent_session_rotated {
                    ""
                } else {
                    entry.title.as_str()
                };
                let prior_description = if agent_session_rotated {
                    ""
                } else {
                    entry.description.as_str()
                };
                let (title, description, project) = resolve_session_names(
                    &self.config.home,
                    &entry.cwd,
                    msg.agent.as_deref(),
                    grok_sid.as_deref(),
                    prior_title,
                    prior_description,
                    &prompt,
                    workspace,
                    event == "prompt",
                );
                if !is_weak_session_title(&title)
                    && is_sticky_thread_title(&description)
                    && !is_bootstrap_sidebar_thread(&description)
                {
                    entry.title = title.clone();
                    entry.description = description;
                    entry.project = project;
                    normalize_agent_label(
                        entry,
                        workspace,
                        msg.agent.as_deref(),
                        grok_sid.as_deref(),
                    );
                    if let Some(ref sid) = entry.agent_session_id {
                        side_effects.session_title = Some((sid.clone(), entry.title.clone()));
                    }
                    persist_agent_session_title(&self.config, entry);
                }
            }

            let applied = match apply_thread_hook(entry, event, state) {
                Some(state) => state,
                None => {
                    crate::daemon::metrics::record_notify_duplicate();
                    return None;
                }
            };

            if let Some(ref sid) = msg.session_id {
                side_effects.session_env = Some(SessionEnvWrite {
                    agent_session_id: sid.clone(),
                    tmux_pane_id: (!pane_id.is_empty()).then_some(pane_id.clone()),
                    window_index: env_window_index,
                    tmux_session: entry.tmux_session.clone(),
                    sessions_session_id: entry.sessions_session_id.clone(),
                    managed_agent: entry.managed_agent.clone(),
                });
            }
            let display_state = entry.display_state();
            let title = entry.title.clone();
            let last_event_at = entry.last_event_at;
            let description = entry.description.clone();
            let cwd = entry.cwd.clone();
            let cwd_label = entry.cwd_label.clone();
            let project = entry.project.clone();
            let completed_thread = entry.completed_thread.clone();
            let completed_at = entry.completed_at;
            let prompt_submitted = entry.prompt_submitted;
            let messaged_at = entry.messaged_at;
            let title_manual = entry.title_manual;
            let managed_agent = entry.managed_agent.clone();
            // Completion: only the first visible done state rings (deduped).
            // Assistance: Grok `approval_required` (not every pre_tool flash).
            let should_ring = rings_alert(event)
                && (event == "approval_required" || (entry.thread_is_complete() && !was_complete));
            side_effects.ring_alert = should_ring;
            let entry_tmux_session = entry.tmux_session.clone();
            let rename_window = (!entry.title_manual
                && (event == "prompt" || event == "session_start"))
                .then_some((entry_tmux_session.clone(), window_index, title.clone()));
            if !pane_id.is_empty() {
                let pane_state = if marks_thread_complete(event) {
                    // Completion is hook-owned (`completed_thread`); pane files must not
                    // retain `working`/`approval` or stale `done` across polls.
                    AgentState::Idle
                } else {
                    applied
                };
                side_effects.pane_state = Some((pane_id.clone(), pane_state));
            }
            side_effects.rename_window = rename_window;
            inner.version += 1;
            inner.dirty = true;
            let version = inner.version;

            let manifest_patch = match (&msg.sessions_session_id, &msg.session_id) {
                (Some(ssn), Some(agent_session_id)) => {
                    let mut patch = ManifestPatch {
                        agent_session_id: Some(agent_session_id.clone()),
                        agent: msg
                            .agent
                            .clone()
                            .or(managed_agent)
                            .filter(|agent| agent != "console"),
                        title: None,
                        messaged_at: None,
                    };
                    if matches!(event, "prompt" | "session_start") {
                        patch.messaged_at = messaged_at;
                        if is_sticky_thread_title(&description)
                            && !is_weak_session_title(&title)
                            && !is_bootstrap_sidebar_thread(&description)
                        {
                            patch.title = Some(title.clone());
                        }
                    }
                    Some((ssn.clone(), patch))
                }
                _ => None,
            };

            (
                ServerEvent::Patch {
                    session_id: id.clone(),
                    state: Some(display_state),
                    title: Some(title),
                    description: Some(description),
                    cwd: Some(cwd),
                    cwd_label: Some(cwd_label),
                    project: Some(project),
                    is_active: None,
                    last_event_at: matches!(
                        event,
                        "prompt" | "session_start" | "pre_tool" | "post_tool" | "tool_fail"
                    )
                    .then_some(last_event_at),
                    completed_thread,
                    completed_at: marks_thread_complete(event)
                        .then_some(completed_at)
                        .flatten(),
                    messaged_at: matches!(event, "prompt" | "session_start")
                        .then(|| messaged_at)
                        .flatten(),
                    prompt_submitted: matches!(event, "prompt" | "session_start")
                        .then_some(prompt_submitted),
                    title_manual: Some(title_manual),
                    ring_bell: should_ring.then_some(true),
                    version,
                },
                side_effects,
                manifest_patch,
            )
        };

        // The RwLock write scope ends before this point. Side effects and manifest enqueue
        // run without holding the lock — never call save_manifest / fsync on the hook path.
        self.apply_notify_side_effects(side_effects).await;
        if let Some((ssn, patch)) = manifest_patch {
            self.manifest_sync.enqueue(ssn, patch);
        }
        Some(patch)
    }

    async fn apply_notify_side_effects(&self, side_effects: NotifySideEffects) {
        if side_effects.ring_alert {
            // Spawn only — never block the hook path waiting on afplay.
            if let Err(err) = play_alert_sound() {
                warn!("play alert sound failed: {err}");
            }
        }

        if let Some((pane_id, state)) = side_effects.pane_state {
            if let Err(err) = save_pane_state(&self.config.tmux_state_dir, &pane_id, state) {
                warn!("save pane state failed for {pane_id}: {err}");
            }
        }

        if let Some(env) = side_effects.session_env {
            if let Err(err) = write_session_env_tmux(
                &self.config,
                &env.agent_session_id,
                env.tmux_pane_id.as_deref(),
                Some(env.window_index),
                &env.tmux_session,
                env.sessions_session_id.as_deref(),
                env.managed_agent.as_deref(),
            ) {
                warn!(
                    "write session env failed for {}: {err}",
                    env.agent_session_id
                );
            }
        }

        if let Some((agent_session_id, title)) = side_effects.session_title {
            let path = self.config.session_title_path(&agent_session_id);
            let result = path
                .parent()
                .map(std::fs::create_dir_all)
                .transpose()
                .and_then(|_| std::fs::write(&path, format!("{title}\n")));
            if let Err(err) = result {
                warn!("write session title failed for {agent_session_id}: {err}");
            }
        }

        if let Some((tmux_session, window_index, title)) = side_effects.rename_window {
            if let Err(err) = rename_window(&tmux_session, window_index, &title) {
                warn!("rename window failed for {window_index}: {err}");
            }
        }
    }

    pub async fn refresh_from_tmux(&self) -> Option<ServerEvent> {
        let refresh_started = std::time::Instant::now();
        let event = self.refresh_from_tmux_inner().await;
        crate::daemon::metrics::record_refresh(refresh_started.elapsed().as_micros() as u64);
        if let Some(ServerEvent::Snapshot { ref sessions, .. }) = event {
            if let Ok(bytes) = serde_json::to_string(sessions).map(|s| s.len() as u64) {
                crate::daemon::metrics::record_snapshot_bytes(bytes);
            }
        }
        event
    }

    async fn refresh_from_tmux_inner(&self) -> Option<ServerEvent> {
        // Agents tmux session gone (e.g. `sessions down`) — re-enter booting so
        // empty polls do not prune persisted rows before the next restore.
        if !session_exists(&self.config.tmux_session) {
            self.set_booting(true).await;
        }

        // Snapshot the current state under a brief read lock. The merge below
        // performs disk I/O (summary reads, env files, title restoration) and
        // must not run while the write lock is held — that would block every
        // hook event and focus request for the duration of the I/O.
        let (previous_sessions, closed_snapshot, booting) = {
            let inner = self.inner.read().await;
            (
                Arc::new(inner.sessions.clone()),
                inner.closed_sessions.clone(),
                inner.booting,
            )
        };

        let session = self.config.tmux_session.clone();
        let home = self.config.home.clone();
        let state_dir = self.config.tmux_state_dir.clone();
        let workspaces_path = self.config.workspaces_path.clone();
        let config = self.config.clone();
        let prior_for_merge = previous_sessions.clone();
        // Poll tmux and compute the fully-merged next state off the lock-holding
        // path, on a blocking thread. All I/O happens here against the snapshot.
        let computed = tokio::task::spawn_blocking(move || {
            let (polled, workspaces) =
                poll_tmux(&session, &home, &state_dir, &workspaces_path).ok()?;
            Some(compute_refresh(
                &config,
                polled,
                &prior_for_merge,
                &closed_snapshot,
                &workspaces,
            ))
        })
        .await
        .ok()
        .flatten()?;

        // Swap the computed state in under the write lock. This section is pure
        // in-memory work — no disk or tmux access — so it holds the lock only
        // for microseconds.
        let mut inner = self.inner.write().await;
        inner.last_poll_at = Some(Utc::now());

        let RefreshComputation {
            merged,
            polled_ids,
            polled_panes,
            polled_sids,
            polled_ssns,
            agent_suppressions,
            healed_closed_ssns,
        } = computed;
        inner.agent_suppressions = agent_suppressions;

        // Drop close markers that no longer apply:
        // - SSN healed because the window is still live (false tombstone / failed kill)
        // - pane/ssn no longer present in the raw poll (window actually gone)
        let healed: std::collections::HashSet<String> =
            healed_closed_ssns.iter().cloned().collect();
        inner.closed_sessions.retain(|marker| {
            if marker
                .sessions_session_id
                .as_ref()
                .is_some_and(|ssn| healed.contains(ssn))
            {
                return false;
            }
            marker
                .sessions_session_id
                .as_ref()
                .is_some_and(|ssn| polled_ssns.contains(ssn))
                || marker
                    .agent_session_id
                    .as_ref()
                    .is_some_and(|sid| polled_sids.contains(sid))
                || (!marker.tmux_pane_id.is_empty() && polled_panes.contains(&marker.tmux_pane_id))
        });

        for fresh in merged {
            // A hook may have closed this session while we were merging.
            if inner.closed_sessions.iter().any(|m| m.matches(&fresh)) {
                continue;
            }
            // Reconcile against concurrent hook updates: only apply the merged
            // result if the live session is still identical to the snapshot we
            // merged from. If a hook landed in the meantime, its (real-time)
            // update wins and the next poll reconciles disk state.
            if inner.sessions.get(&fresh.id) == previous_sessions.get(&fresh.id) {
                inner.sessions.insert(fresh.id.clone(), fresh);
            }
        }

        if !booting {
            for session in previous_sessions.values() {
                if !polled_ids.contains(&session.id) {
                    let _ = clear_manual_title_files_for_tab(&self.config, session.tab_index);
                }
            }
            // Remove sessions that vanished from tmux, but preserve any created
            // concurrently by a hook (present live, absent from our snapshot) — the
            // next poll will pick them up.
            inner
                .sessions
                .retain(|id, _| polled_ids.contains(id) || !previous_sessions.contains_key(id));
        }

        // Tombstone only stable SSNs that are truly gone after merge — never
        // when the same managed session reattached under a new tmux:win:N id.
        let vanished_ssns = if !booting {
            sessions_session_ids_to_tombstone(&previous_sessions, &inner.sessions)
        } else {
            Vec::new()
        };

        let changed = sessions_changed(&previous_sessions, &inner.sessions);
        let snapshot = if changed {
            inner.version += 1;
            inner.dirty = true;
            let version = inner.version;
            let sessions = sorted_sessions(inner.sessions.values(), &inner.agent_suppressions);
            Some(ServerEvent::Snapshot { sessions, version })
        } else {
            None
        };

        // Persist restore allowlist from the healthy post-merge set (not mid-boot).
        let live_snapshot_ssns = if !booting {
            Some(
                inner
                    .sessions
                    .values()
                    .filter_map(|session| session.sessions_session_id.clone())
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        drop(inner);

        // Live windows that were false-closed must re-open so restore/hooks work.
        for ssn in &healed_closed_ssns {
            let _ = crate::session::reopen_manifest_entry(&self.config, ssn);
        }
        if !vanished_ssns.is_empty() {
            // Windows that died without going through CloseSession must not
            // reappear as phantom PWDs on the next cold boot.
            let _ = crate::session::mark_entries_closed(&self.config, &vanished_ssns);
        }
        if let Some(ssns) = live_snapshot_ssns {
            let _ = crate::session::live_snapshot::save(&self.config, ssns);
        }

        snapshot
    }

    pub async fn snapshot_event(&self) -> ServerEvent {
        let inner = self.inner.read().await;
        let sessions = sorted_sessions(inner.sessions.values(), &inner.agent_suppressions);
        ServerEvent::Snapshot {
            sessions,
            version: inner.version,
        }
    }

    /// Resolve sidebar ordinal / tab_index to a live tmux window index.
    pub async fn resolve_focus_window(
        &self,
        window_index: u32,
        tab_index: Option<u32>,
    ) -> anyhow::Result<u32> {
        let inner = self.inner.read().await;
        resolve_focus_target(
            &self.config,
            &inner.sessions,
            &inner.agent_suppressions,
            window_index,
            tab_index,
        )
    }

    /// Fast focus path: resolves target and calls `select_window` only.
    /// Does NOT acquire the write lock — `is_active` is updated on the next poll
    /// or via [`Self::commit_focus_state`].
    /// Use this in the subscriber connection to avoid blocking the event loop.
    pub async fn focus_exec(
        &self,
        window_index: u32,
        tab_index: Option<u32>,
    ) -> anyhow::Result<()> {
        let target = self.resolve_focus_window(window_index, tab_index).await?;
        // Switch the workspace pane first — this is the latency-sensitive part
        // (matches sidebar click: select-window immediately).
        select_window(&self.config.tmux_session, target)?;
        Ok(())
    }

    /// Mark the focused window active and return a snapshot for subscribers.
    /// Call after [`Self::focus_exec`] so CLI/UI can ack the switch before this work.
    pub async fn commit_focus_state(&self, target_tab_index: u32) -> ServerEvent {
        let mut inner = self.inner.write().await;
        for session in inner.sessions.values_mut() {
            session.is_active = session.tab_index == target_tab_index;
            if session.tab_index == target_tab_index
                && session.acknowledge_if_done()
                && !session.tmux_pane_id.is_empty()
            {
                let _ = save_pane_state(
                    &self.config.tmux_state_dir,
                    &session.tmux_pane_id,
                    AgentState::Idle,
                );
            }
        }
        inner.version += 1;
        inner.dirty = true;
        let version = inner.version;
        let sessions = sorted_sessions(inner.sessions.values(), &inner.agent_suppressions);
        ServerEvent::Snapshot { sessions, version }
    }

    /// Full focus path: `select_window` then state commit. Prefer the split
    /// `focus_exec` + `commit_focus_state` in CLI handlers so the client can
    /// return as soon as the window has switched.
    pub async fn focus(
        &self,
        window_index: u32,
        tab_index: Option<u32>,
    ) -> anyhow::Result<ServerEvent> {
        let target = self.resolve_focus_window(window_index, tab_index).await?;
        select_window(&self.config.tmux_session, target)?;
        Ok(self.commit_focus_state(target).await)
    }

    pub async fn acknowledge_completion(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<ServerEvent>> {
        let mut inner = self.inner.write().await;
        let entry = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| anyhow::anyhow!("session {session_id} not found"))?;
        if !entry.acknowledge_if_done() {
            return Ok(None);
        }
        let pane_id = entry.tmux_pane_id.clone();
        let display_state = entry.display_state();
        let completed_thread = entry.completed_thread.clone();
        let completed_at = entry.completed_at;
        if !pane_id.is_empty() {
            let _ = save_pane_state(&self.config.tmux_state_dir, &pane_id, AgentState::Idle);
        }
        inner.version += 1;
        inner.dirty = true;
        let version = inner.version;
        let patch = ServerEvent::Patch {
            session_id: session_id.to_string(),
            state: Some(display_state),
            title: None,
            description: None,
            cwd: None,
            cwd_label: None,
            project: None,
            is_active: None,
            last_event_at: None,
            completed_thread,
            completed_at,
            messaged_at: None,
            prompt_submitted: None,
            title_manual: None,
            ring_bell: None,
            version,
        };
        Ok(Some(patch))
    }

    pub async fn rename_session(
        &self,
        session_id: &str,
        title_input: String,
    ) -> anyhow::Result<ServerEvent> {
        let title_input = title_input.trim();
        if title_input.is_empty() {
            anyhow::bail!("rename title cannot be empty");
        }

        let mut side_effects = NotifySideEffects::default();
        let (patch, tab_index, title) = {
            let mut inner = self.inner.write().await;
            let entry = inner
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| anyhow::anyhow!("session {session_id} not found"))?;
            let tab_index = entry.tab_index;
            let (title, description, project) = resolve_renamed_title(entry, title_input);
            entry.title = title.clone();
            entry.description = description.clone();
            entry.project = project.clone();
            entry.title_manual = true;

            let tmux_session = if entry.tmux_session.is_empty() {
                self.config.tmux_session.clone()
            } else {
                entry.tmux_session.clone()
            };
            side_effects.rename_window = Some((tmux_session, tab_index, title.clone()));

            inner.version += 1;
            inner.dirty = true;
            let version = inner.version;

            let patch = ServerEvent::Patch {
                session_id: session_id.to_string(),
                state: None,
                title: Some(title.clone()),
                description: Some(description),
                project: Some(project),
                cwd: None,
                cwd_label: None,
                is_active: None,
                last_event_at: None,
                completed_thread: None,
                completed_at: None,
                messaged_at: None,
                prompt_submitted: None,
                title_manual: Some(true),
                ring_bell: None,
                version,
            };
            (patch, tab_index, title)
        };

        if let Err(err) = write_manual_session_title_files(&self.config, tab_index, &title) {
            warn!("write session title failed for tab {tab_index}: {err}");
        }

        self.apply_notify_side_effects(side_effects).await;
        Ok(patch)
    }

    /// Kill the tmux window, tombstone the session, and remove it from state.
    /// Tombstones block concurrent polls and late hook events from re-adding the row.
    pub async fn close_session(&self, session_id: &str) -> anyhow::Result<ServerEvent> {
        let to_close = {
            let inner = self.inner.read().await;
            inner.sessions.get(session_id).cloned()
        };
        let Some(removed) = to_close else {
            anyhow::bail!("session {session_id} not found");
        };

        let target = crate::session::lifecycle::CloseTarget {
            session_id: Some(session_id.to_string()),
            sessions_session_id: removed.sessions_session_id.clone(),
            window_index: Some(removed.tab_index),
        };
        crate::session::lifecycle::close_unified(&self.config, target)?;

        let marker = ClosedSessionMarker::from_session(&removed);

        let event = {
            let mut inner = self.inner.write().await;
            inner.closed_sessions.insert(marker);
            if inner.sessions.remove(session_id).is_some() {
                inner.version += 1;
                inner.dirty = true;
            }
            let version = inner.version;
            let sessions = sorted_sessions(inner.sessions.values(), &inner.agent_suppressions);
            ServerEvent::Snapshot { sessions, version }
        };

        Ok(event)
    }

    pub async fn bootstrap(&self) -> anyhow::Result<()> {
        bootstrap_session(&self.config)
    }
}
