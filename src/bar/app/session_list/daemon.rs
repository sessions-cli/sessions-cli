use super::super::{is_fresh_unacknowledged_completion, AcknowledgedCompletion, App, AGENTS_WINDOW_PROBE_INTERVAL};
use crate::bar::client::{ClientEvent, EventReceiver};
use crate::model::{AgentState, ServerEvent, Session};
use chrono::Utc;
use std::collections::HashSet;
use std::time::Instant;

pub(crate) fn coalesce_client_events(batch: Vec<ClientEvent>) -> Vec<ClientEvent> {
    let mut pending_snapshots = Vec::new();
    let mut patches = Vec::new();
    let mut other = Vec::new();
    for ev in batch {
        match ev {
            ClientEvent::Snapshot { .. } => pending_snapshots.push(ev),
            ClientEvent::Patch(patch) => patches.push(patch),
            other_ev => other.push(other_ev),
        }
    }
    if let Some(best) = pending_snapshots.into_iter().max_by_key(snapshot_version) {
        other.push(best);
    }
    for patch in patches {
        other.push(ClientEvent::Patch(patch));
    }
    other
}

fn snapshot_version(ev: &ClientEvent) -> u64 {
    match ev {
        ClientEvent::Snapshot { version, .. } => *version,
        _ => 0,
    }
}

fn snapshot_order(sessions: &[Session]) -> Vec<&str> {
    sessions.iter().map(|session| session.id.as_str()).collect()
}

fn snapshot_needs_structure_rebuild(current: &[Session], incoming: &[Session]) -> bool {
    current.len() != incoming.len() || snapshot_order(current) != snapshot_order(incoming)
}

/// Row paint reads from [`RowKind::Session`] clones — keep them aligned with `self.sessions`
/// when poll snapshots update agent state without reordering the list.
fn snapshot_session_visual_unchanged(current: &[Session], incoming: &[Session]) -> bool {
    if snapshot_needs_structure_rebuild(current, incoming) {
        return false;
    }
    current.iter().zip(incoming.iter()).all(|(left, right)| {
        left.tab_index == right.tab_index
            && left.title == right.title
            && left.description == right.description
            && left.messaged_at == right.messaged_at
            && left.is_active == right.is_active
            && left.state == right.state
            && left.last_event_at == right.last_event_at
            && left.completed_thread == right.completed_thread
            && left.completed_at == right.completed_at
    })
}

impl App {
    pub(crate) fn drain_client_events(&mut self, events: &EventReceiver) {
        let mut batch = Vec::new();
        while let Some(ev) = events.try_recv() {
            batch.push(ev);
        }
        for ev in coalesce_client_events(batch) {
            self.apply_event(ev);
        }
    }

    pub(crate) fn apply_event(&mut self, ev: ClientEvent) {
        match ev {
            ClientEvent::Snapshot { sessions, version } => {
                if version >= self.version {
                    let prev_active_id = self
                        .sessions
                        .iter()
                        .find(|s| s.is_active)
                        .map(|s| s.id.clone());
                    let sessions = self.merge_incoming_snapshot(sessions);
                    let daemon_active_tab = sessions
                        .iter()
                        .find(|session| session.is_active)
                        .map(|session| session.tab_index);
                    let structure_changed =
                        snapshot_needs_structure_rebuild(&self.sessions, &sessions);
                    let visual_unchanged =
                        snapshot_session_visual_unchanged(&self.sessions, &sessions);
                    self.sessions = sessions;
                    self.reconcile_pending_focus(daemon_active_tab);
                    self.apply_completion_acknowledgments();
                    self.version = version;
                    if structure_changed {
                        self.rebuild_rows();
                        let new_active_id = self
                            .sessions
                            .iter()
                            .find(|s| s.is_active)
                            .map(|s| s.id.clone());
                        if prev_active_id != new_active_id {
                            if let Some(id) = &new_active_id {
                                self.acknowledge_completion_for_session(id);
                            }
                            if self.pending_focus_tab_index.is_none() {
                                self.sync_selection_to_active(true);
                            }
                            self.pointer_hover_refresh_pending = true;
                        }
                    } else if !visual_unchanged {
                        self.sync_row_sessions();
                    }
                }
            }
            ClientEvent::Patch(patch) => {
                if let crate::model::ServerEvent::Patch {
                    session_id,
                    state,
                    title,
                    description,
                    cwd,
                    cwd_label,
                    project,
                    is_active,
                    last_event_at,
                    completed_thread,
                    completed_at,
                    messaged_at,
                    prompt_submitted,
                    title_manual,
                    ring_bell,
                    version,
                } = patch
                {
                    if version < self.version {
                        return;
                    }
                    self.version = version;
                    let mut should_ring = ring_bell.unwrap_or(false);
                    let mut clear_hover_after_patch = false;
                    let mut acknowledge_on_active = false;
                    let acknowledged = self.acknowledged_completions.clone();
                    let mut was_complete = false;
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        was_complete = s.thread_is_complete();
                        if let Some(st) = state {
                            s.state = st;
                            if matches!(
                                st,
                                AgentState::Working | AgentState::Approval | AgentState::Error
                            ) {
                                s.completed_thread = None;
                                s.completed_at = None;
                            }
                        }
                        if let Some(ct) = completed_thread {
                            s.completed_thread = Some(ct);
                        }
                        if let Some(at) = completed_at {
                            s.completed_at = Some(at);
                        }
                        if let Some(submitted) = prompt_submitted {
                            s.prompt_submitted = submitted;
                        }
                        if let Some(at) = messaged_at {
                            s.messaged_at = Some(at);
                        }
                        if !was_complete
                            && is_fresh_unacknowledged_completion(s, &acknowledged)
                            && !should_ring
                        {
                            should_ring = true;
                        }
                        if let Some(t) = title {
                            s.title = t;
                        }
                        if let Some(d) = description {
                            s.description = d;
                        }
                        if let Some(cwd) = cwd {
                            s.cwd = cwd;
                        }
                        if let Some(cwd_label) = cwd_label {
                            s.cwd_label = cwd_label;
                        }
                        if let Some(project) = project {
                            s.project = project;
                        }
                        if let Some(manual) = title_manual {
                            s.title_manual = manual;
                        }
                        if let Some(a) = is_active {
                            let became_active = a && !s.is_active;
                            s.is_active = a;
                            acknowledge_on_active = became_active;
                        }
                        if let Some(at) = last_event_at {
                            s.last_event_at = at;
                        }
                    } else {
                        let _ = self.client.refresh();
                    }
                    if !was_complete
                        && self
                            .sessions
                            .iter()
                            .find(|s| s.id == session_id)
                            .is_some_and(|s| is_fresh_unacknowledged_completion(s, &acknowledged))
                    {
                        self.acknowledged_completions.remove(&session_id);
                    }
                    if acknowledge_on_active && self.acknowledge_session_completion(&session_id) {
                        clear_hover_after_patch = true;
                    }
                    if clear_hover_after_patch {
                        self.pointer_hover_refresh_pending = true;
                    }
                    self.reapply_pending_focus_after_patch();
                    self.apply_completion_acknowledgments();
                    self.rebuild_rows();
                    if should_ring {
                        self.force_redraw();
                        if let Err(err) = crate::daemon::tmux::play_alert_sound() {
                            tracing::warn!("play alert sound failed: {err}");
                        }
                    }
                }
            }
            ClientEvent::Disconnected(_) => {}
        }
    }
    pub(crate) fn apply_completion_acknowledgments(&mut self) {
        let acknowledged = self.acknowledged_completions.clone();
        for session in &mut self.sessions {
            let Some(ack) = acknowledged.get(&session.id) else {
                continue;
            };
            if session.completed_thread.as_deref() != Some(ack.thread.as_str()) {
                continue;
            }
            let seen_already = session
                .completed_at
                .is_none_or(|completed_at| completed_at <= ack.at);
            if seen_already && session.state == AgentState::Done {
                session.state = AgentState::Idle;
            }
        }
    }
    pub(crate) fn acknowledge_session_completion(&mut self, session_id: &str) -> bool {
        let snapshot = self
            .sessions
            .iter()
            .find(|s| s.id == session_id && s.thread_is_complete())
            .map(|session| {
                (
                    session.completed_thread.clone(),
                    session.completed_at,
                    session.tmux_pane_id.clone(),
                )
            });
        let Some((Some(thread), completed_at, pane_id)) = snapshot else {
            return false;
        };
        let at = completed_at.unwrap_or_else(Utc::now);
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            session.state = AgentState::Idle;
        }
        self.acknowledged_completions.insert(
            session_id.to_string(),
            AcknowledgedCompletion { thread, at },
        );
        if !pane_id.is_empty() {
            let _ = crate::daemon::tmux::save_pane_state(
                &self.config.tmux_state_dir,
                &pane_id,
                AgentState::Idle,
            );
        }
        let _ = self.client.acknowledge_completion(session_id);
        true
    }
    pub(crate) fn acknowledge_completion_for_session(&mut self, session_id: &str) {
        if self.acknowledge_session_completion(session_id) {
            self.rebuild_rows();
            self.force_redraw();
        }
    }
    pub(crate) fn merge_incoming_snapshot(&self, mut incoming: Vec<Session>) -> Vec<Session> {
        let incoming_ids: std::collections::HashSet<_> =
            incoming.iter().map(|session| session.id.clone()).collect();
        for session in &mut incoming {
            if session.messaged_at.is_some() {
                continue;
            }
            let Some(local) = self.sessions.iter().find(|local| local.id == session.id) else {
                continue;
            };
            if let Some(at) = local.messaged_at {
                session.messaged_at = Some(at);
            }
        }
        for local in &self.sessions {
            if incoming_ids.contains(&local.id) {
                continue;
            }
            if local.tmux_pane_id.is_empty() {
                incoming.push(local.clone());
            }
        }
        incoming
    }
    pub(crate) fn reconcile_pending_focus(&mut self, daemon_active_tab: Option<u32>) {
        let Some(pending) = self.pending_focus_tab_index else {
            return;
        };
        self.apply_exclusive_focus(pending);
        if daemon_active_tab == Some(pending) {
            self.pending_focus_tab_index = None;
        }
    }
    pub(crate) fn reapply_pending_focus_after_patch(&mut self) {
        if let Some(tab_index) = self.pending_focus_tab_index {
            self.apply_exclusive_focus(tab_index);
        }
    }
    pub(crate) fn apply_exclusive_focus(&mut self, tab_index: u32) {
        for session in &mut self.sessions {
            session.is_active = session.tab_index == tab_index;
        }
    }
    pub(crate) fn apply_refresh_snapshot(&mut self) {
        if let Ok(Some(crate::model::ServerEvent::Snapshot { sessions, version })) =
            self.client.refresh_snapshot()
        {
            self.apply_event(ClientEvent::Snapshot { sessions, version });
        }
    }
    pub(crate) fn sync_external_active_window(&mut self) {
        if self.last_agents_window_probe.elapsed() < AGENTS_WINDOW_PROBE_INTERVAL {
            return;
        }
        self.last_agents_window_probe = Instant::now();
        let Ok((index, name, cwd)) =
            crate::daemon::tmux::active_window_summary(&self.config.tmux_session)
        else {
            return;
        };
        let known = self
            .sessions
            .iter()
            .any(|session| session.tab_index == index);
        if known {
            self.last_tracked_agents_window = Some(index);
            return;
        }
        if self.last_tracked_agents_window == Some(index) {
            return;
        }
        self.last_tracked_agents_window = Some(index);
        let cwd_label = crate::pty::format_tilde_path(&cwd, &self.config.home);
        let agent_id = crate::pty::parse_app(&name)
            .filter(|app| crate::pty::is_agent_app(app))
            .map(|app| app.to_string());
        self.push_optimistic_new_session(index, agent_id.as_deref(), cwd, cwd_label, &name, false);
        self.client.refresh_async();
    }
    pub(crate) fn push_optimistic_new_session(
        &mut self,
        window_index: u32,
        agent_id: Option<&str>,
        cwd: String,
        cwd_label: String,
        window_name: &str,
        claim_focus: bool,
    ) {
        let now = Utc::now();
        let (title, description, project) = if let Some(agent) = agent_id {
            let title = crate::pty::format_session_title(agent, "?");
            (title, "?".to_string(), agent.to_string())
        } else if crate::pty::is_console_label(window_name)
            || crate::pty::parse_app(window_name).is_none()
        {
            (
                crate::pty::CONSOLE_LABEL.to_string(),
                crate::pty::CONSOLE_LABEL.to_string(),
                String::new(),
            )
        } else {
            let title = window_name.to_string();
            let description = crate::pty::parse_description(window_name);
            let project = crate::pty::parse_app(window_name).unwrap_or_default();
            (title, description, project)
        };
        if claim_focus {
            for session in &mut self.sessions {
                session.is_active = false;
            }
        }
        self.sessions.push(Session {
            id: Session::session_id_from_window(window_index),
            kitty_window_id: window_index as u64,
            kitty_tab_id: 0,
            kitty_os_window_id: 0,
            tab_index: window_index,
            tmux_session: self.config.tmux_session.clone(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title,
            description,
            cwd,
            cwd_label,
            project,
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(now),
            prompt_submitted: false,
            title_manual: false,
            is_active: claim_focus,
            last_event_at: now,
            ..Default::default()
        });
        if claim_focus {
            self.pending_focus_tab_index = Some(window_index);
            self.apply_exclusive_focus(window_index);
            self.select_session_by_tab_index(window_index);
        }
        self.rebuild_rows();
        self.rows_version = self.rows_version.wrapping_add(1);
        self.force_redraw();
    }
}

#[cfg(test)]
mod tests {
    use crate::bar::app::{App, test_fixtures::{sample_session, completed_session}};
    use crate::bar::client::ClientEvent;
    use crate::config::Config;
    use crate::model::{AgentState, ServerEvent};
    use chrono::Utc;

    #[test]
    fn snapshot_with_pending_focus_overrides_stale_daemon_active() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.pending_focus_tab_index = Some(2);
        app.apply_event(ClientEvent::Snapshot {
            sessions: vec![
                sample_session("tmux:win:1", 1, "one", true),
                sample_session("tmux:win:2", 2, "two", false),
            ],
            version: 1,
        });

        assert_eq!(app.pending_focus_tab_index, Some(2));
        assert_eq!(
            app.sessions
                .iter()
                .filter(|session| session.is_active)
                .map(|session| session.tab_index)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn snapshot_clears_pending_focus_when_daemon_agrees() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.pending_focus_tab_index = Some(2);
        app.apply_event(ClientEvent::Snapshot {
            sessions: vec![
                sample_session("tmux:win:1", 1, "one", false),
                sample_session("tmux:win:2", 2, "two", true),
            ],
            version: 1,
        });

        assert_eq!(app.pending_focus_tab_index, None);
        assert_eq!(
            app.sessions
                .iter()
                .find(|session| session.is_active)
                .map(|session| session.tab_index),
            Some(2)
        );
    }

    #[test]
    fn stale_snapshot_preserves_optimistic_new_session() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![sample_session("tmux:win:1", 1, "one", true)];
        app.push_optimistic_new_session(
            9,
            Some("grok"),
            "/tmp/foo".into(),
            "~/tmp/foo".into(),
            "grok · ?",
            false,
        );

        app.apply_event(ClientEvent::Snapshot {
            sessions: vec![sample_session("tmux:win:1", 1, "one", true)],
            version: 1,
        });

        assert!(
            app.sessions
                .iter()
                .any(|session| session.id == "tmux:win:9"),
            "optimistic row should survive until daemon poll catches the new window"
        );
        assert_eq!(
            app.sessions
                .iter()
                .find(|session| session.is_active)
                .map(|session| session.tab_index),
            Some(1),
            "background detection must not steal active focus"
        );
    }

    #[test]
    fn acknowledged_completion_survives_stale_daemon_snapshot() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        let completed_at = Utc::now();
        let mut completed = completed_session("tmux:win:1", 1, "ship api");
        completed.completed_at = Some(completed_at);
        let stale_snapshot = completed.clone();
        app.sessions = vec![completed, sample_session("tmux:win:2", 2, "other", false)];
        assert!(app.acknowledge_session_completion("tmux:win:1"));
        app.apply_event(ClientEvent::Snapshot {
            sessions: vec![
                stale_snapshot,
                sample_session("tmux:win:2", 2, "other", false),
            ],
            version: 1,
        });

        let completed = app.sessions.iter().find(|s| s.id == "tmux:win:1").unwrap();
        assert!(!completed.thread_is_complete());
        assert_eq!(
            app.acknowledged_completions
                .get("tmux:win:1")
                .map(|ack| &ack.thread),
            Some(&"ship api".to_string())
        );
    }

    #[test]
    fn patch_does_not_restore_green_after_acknowledged_completion() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![completed_session("tmux:win:1", 1, "ship api")];
        assert!(app.acknowledge_session_completion("tmux:win:1"));

        app.apply_event(ClientEvent::Patch(ServerEvent::Patch {
            session_id: "tmux:win:1".into(),
            state: Some(AgentState::Done),
            title: None,
            description: None,
            cwd: None,
            cwd_label: None,
            project: None,
            is_active: None,
            last_event_at: None,
            completed_thread: Some("ship api".into()),
            completed_at: app.sessions[0].completed_at,
            messaged_at: None,
            prompt_submitted: None,
            title_manual: None,
            ring_bell: None,
            version: 2,
        }));

        assert!(!app.sessions[0].thread_is_complete());
    }

    #[test]
    fn coalesce_client_events_keeps_highest_version_snapshot() {
        let events = vec![
            ClientEvent::Snapshot {
                sessions: vec![sample_session("tmux:win:1", 1, "one", true)],
                version: 1,
            },
            ClientEvent::Patch(ServerEvent::Patch {
                session_id: "tmux:win:1".into(),
                state: Some(AgentState::Working),
                title: None,
                description: None,
                cwd: None,
                cwd_label: None,
                project: None,
                is_active: None,
                last_event_at: None,
                completed_thread: None,
                completed_at: None,
                messaged_at: None,
                prompt_submitted: None,
                title_manual: None,
                ring_bell: None,
                version: 2,
            }),
            ClientEvent::Snapshot {
                sessions: vec![sample_session("tmux:win:1", 1, "one", true)],
                version: 3,
            },
            ClientEvent::Snapshot {
                sessions: vec![sample_session("tmux:win:1", 1, "one", true)],
                version: 2,
            },
        ];

        let coalesced = super::coalesce_client_events(events);
        assert_eq!(coalesced.len(), 2);
        assert!(matches!(
            &coalesced[0],
            ClientEvent::Snapshot { version: 3, .. }
        ));
        assert!(matches!(&coalesced[1], ClientEvent::Patch(_)));
    }

    #[test]
    fn merge_incoming_snapshot_preserves_local_messaged_at() {
        let config = Config::default();
        let app = App::new(&config).unwrap();
        let at = Utc::now();
        let mut local = sample_session("tmux:win:3", 3, "fresh", false);
        local.messaged_at = Some(at);
        let mut app = app;
        app.sessions = vec![local];

        let mut incoming = sample_session("tmux:win:3", 3, "fresh", false);
        incoming.messaged_at = None;
        incoming.tmux_pane_id = "%3".into();

        let merged = app.merge_incoming_snapshot(vec![incoming]);
        assert_eq!(merged[0].messaged_at, Some(at));
    }

    #[test]
    fn snapshot_with_unchanged_sidebar_skips_rebuild() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        let sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.sessions = sessions.clone();
        app.rebuild_rows();
        let rows_before = app.rows_version;

        let mut enriched = sessions;
        enriched[0].agent_session_id = Some("agent-1".into());
        enriched[1].tmux_pane_id = "%1".into();
        app.apply_event(ClientEvent::Snapshot {
            sessions: enriched,
            version: 2,
        });

        assert_eq!(app.rows_version, rows_before);
        assert_eq!(app.version, 2);
        assert_eq!(app.sessions[0].agent_session_id.as_deref(), Some("agent-1"));
    }

    #[test]
    fn snapshot_state_change_syncs_row_sessions_without_structure_rebuild() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        let mut idle = sample_session("tmux:win:1", 1, "ship api", false);
        idle.state = AgentState::Idle;
        app.sessions = vec![idle.clone()];
        app.rebuild_rows();
        let rows_before = app.rows.len();

        let mut working = idle;
        working.state = AgentState::Working;
        working.last_event_at = Utc::now();
        app.apply_event(ClientEvent::Snapshot {
            sessions: vec![working],
            version: 2,
        });

        assert_eq!(app.rows.len(), rows_before);
        let row_session = app
            .rows
            .iter()
            .find_map(|row| match row {
                crate::bar::ui::RowKind::Session { session } => Some(session),
                _ => None,
            })
            .expect("expected session row");
        assert_eq!(row_session.state, AgentState::Working);
        assert!(row_session.shows_run_spinner());
    }

    #[test]
    fn snapshot_without_pending_trusts_daemon_active() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.apply_event(ClientEvent::Snapshot {
            sessions: vec![
                sample_session("tmux:win:1", 1, "one", false),
                sample_session("tmux:win:2", 2, "two", true),
            ],
            version: 1,
        });

        assert_eq!(app.pending_focus_tab_index, None);
        assert_eq!(
            app.sessions
                .iter()
                .find(|session| session.is_active)
                .map(|session| session.tab_index),
            Some(2)
        );
    }

}
