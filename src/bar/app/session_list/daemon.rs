use super::super::{
    is_fresh_unacknowledged_completion, AcknowledgedCompletion, App, AGENTS_WINDOW_PROBE_INTERVAL,
};
use crate::bar::client::{ClientEvent, EventReceiver};
use crate::model::{AgentState, Session};
use chrono::Utc;
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
    // Patches first: they carry one-shot signals (`ring_bell`) and higher-fidelity
    // hook state. Snapshots used to be applied first and then dropped older patches
    // via version gating — which silently killed completion bells whenever a poll
    // snapshot shared the same drain batch.
    for patch in patches {
        other.push(ClientEvent::Patch(patch));
    }
    if let Some(best) = pending_snapshots.into_iter().max_by_key(snapshot_version) {
        other.push(best);
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

/// Fields the sidebar row renderer reads from embedded [`RowKind::Session`] clones.
pub(super) fn session_row_render_eq(left: &Session, right: &Session) -> bool {
    left.tab_index == right.tab_index
        && left.title == right.title
        && left.description == right.description
        && left.messaged_at == right.messaged_at
        && left.is_active == right.is_active
        && left.state == right.state
        && left.last_event_at == right.last_event_at
        && left.completed_thread == right.completed_thread
        && left.completed_at == right.completed_at
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
                    // Only force a rebuild when the boot empty row ("Starting…") is
                    // currently painted — not merely because daemon_ready flips.
                    let leaving_boot_empty = self.is_boot_loading();
                    self.daemon_ready = true;
                    if self.reconnecting {
                        self.reconnecting = false;
                        // Drop sticky reconnect notice once the stream is healthy again.
                        if self
                            .clipboard_notice_text
                            .as_deref()
                            .is_some_and(|t| t.starts_with("reconnecting"))
                        {
                            self.clipboard_notice_text = None;
                            self.clipboard_notice_until = None;
                        }
                    }
                    let prev_active_id = self
                        .sessions
                        .iter()
                        .find(|s| s.is_active)
                        .map(|s| s.id.clone());
                    // Capture whether the user was tracking focus *before* we
                    // apply the new is_active flags from the daemon.
                    let was_tracking_active = self.selection_tracks_active();
                    let sessions = self.merge_incoming_snapshot(sessions);
                    let daemon_active_tab = sessions
                        .iter()
                        .find(|session| session.is_active)
                        .map(|session| session.tab_index);
                    // Membership *or* sort order change (messaged_at) — both flip
                    // snapshot_order so rows re-sort via rebuild.
                    let structure_changed =
                        snapshot_needs_structure_rebuild(&self.sessions, &sessions)
                            || leaving_boot_empty;
                    self.sessions = sessions;
                    self.reconcile_pending_focus(daemon_active_tab);
                    self.apply_completion_acknowledgments();
                    self.version = version;
                    // Full post-reconcile snapshots are complete (partial restore is
                    // suppressed while the daemon is booting). Persist PWD order so
                    // cold boot restores the same sidebar group layout.
                    let group_order_changed = self.persist_group_order_from_sessions();
                    if structure_changed || group_order_changed {
                        self.rebuild_rows();
                    } else {
                        // Always realign row clones after post-processing (ack/focus) so
                        // completion badges and spinners never stick on stale session data.
                        self.sync_row_sessions();
                    }
                    // External focus (`sessions focus N`, ⌘1–0 via Cursor tasks, tmux
                    // window change after poll) often arrives as a same-structure
                    // snapshot with only is_active flipped. Follow that only when the
                    // user was already highlighting the previous active session —
                    // arrow-key browse onto another row must not be stolen.
                    let new_active_id = self
                        .sessions
                        .iter()
                        .find(|s| s.is_active)
                        .map(|s| s.id.clone());
                    if prev_active_id != new_active_id {
                        if let Some(id) = &new_active_id {
                            self.acknowledge_completion_for_session(id);
                        }
                        if self.pending_focus_tab_index.is_none() && was_tracking_active {
                            self.sync_selection_to_active(true);
                        }
                        self.pointer_hover_refresh_pending = true;
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
                    // Daemon owns the audible alert (spawned on notify). The bar only
                    // uses `ring_bell` for a force redraw so the green/amber row paints
                    // immediately — do not play sound here (would double-ring).
                    let force_redraw_for_bell = ring_bell.unwrap_or(false);
                    if version < self.version {
                        if force_redraw_for_bell {
                            self.force_redraw();
                        }
                        return;
                    }
                    self.version = version;
                    let mut should_ring = force_redraw_for_bell;
                    let mut clear_hover_after_patch = false;
                    let mut acknowledge_on_active = false;
                    // Only messaged_at / cwd_label / group moves need a full row
                    // rebuild. State/title/spinner patches used to rebuild every
                    // time and re-sort, which made the highlight thrash.
                    let mut needs_structure_rebuild = false;
                    let was_tracking_active = self.selection_tracks_active();
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
                            if s.messaged_at != Some(at) {
                                needs_structure_rebuild = true;
                            }
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
                            if s.cwd_label != cwd_label {
                                needs_structure_rebuild = true;
                            }
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
                    if needs_structure_rebuild {
                        self.rebuild_rows();
                    } else {
                        self.sync_row_sessions();
                    }
                    // Focus patches: follow only when the user was already on the
                    // previous active row (not mid browse-with-arrows).
                    if acknowledge_on_active
                        && self.pending_focus_tab_index.is_none()
                        && was_tracking_active
                    {
                        self.sync_selection_to_active(true);
                    }
                    if should_ring {
                        self.force_redraw();
                    }
                }
            }
            ClientEvent::Disconnected(_) => {
                if !self.reconnecting {
                    self.reconnecting = true;
                    if self.sessions.is_empty() {
                        self.rebuild_rows();
                    } else {
                        // Sticky title notice while sessions remain on screen.
                        self.clipboard_notice_text = Some("reconnecting…".into());
                        self.clipboard_notice_until = None;
                        self.force_redraw();
                    }
                }
            }
        }
    }

    pub(crate) fn boot_empty_label(&self) -> Option<&'static str> {
        if self.reconnecting {
            Some("Reconnecting…")
        } else if !self.daemon_ready {
            Some("Starting…")
        } else {
            None
        }
    }

    pub(crate) fn is_boot_loading(&self) -> bool {
        self.sessions.is_empty() && self.boot_empty_label().is_some()
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
        if self.last_agents_window_probe.elapsed()
            < self.effective_probe_interval(AGENTS_WINDOW_PROBE_INTERVAL)
        {
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
        }
        // Rebuild first — select_session_by_tab_index reads `rows`/`selectable`,
        // which do not include this session until rebuild. Selecting before rebuild
        // silently fails and leaves the previous row highlighted (warm-pool claim
        // and cold create both hit this path).
        self.rebuild_rows();
        if claim_focus {
            let _ = self.select_session_by_tab_index(window_index);
        }
        self.rows_version = self.rows_version.wrapping_add(1);
        self.force_redraw();
    }
}

#[cfg(test)]
mod tests {
    use crate::bar::app::{
        test_fixtures::{completed_session, sample_session},
        AcknowledgedCompletion, App,
    };
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
    fn same_structure_focus_snapshot_moves_sidebar_selection() {
        // `sessions focus N` / Cursor ⌘N broadcasts a snapshot that only flips
        // is_active — no add/remove/reorder. Selection must still follow.
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();
        let first = app.session_row_index("tmux:win:1").unwrap();
        app.set_selected(first);
        assert_eq!(
            app.session_at(app.selected).map(|s| s.id.as_str()),
            Some("tmux:win:1")
        );

        app.apply_event(ClientEvent::Snapshot {
            sessions: vec![
                sample_session("tmux:win:1", 1, "one", false),
                sample_session("tmux:win:2", 2, "two", true),
            ],
            version: 2,
        });

        assert_eq!(
            app.session_at(app.selected).map(|s| s.id.as_str()),
            Some("tmux:win:2"),
            "sidebar selection must follow external focus when only is_active changes"
        );
        assert!(app
            .sessions
            .iter()
            .find(|s| s.id == "tmux:win:2")
            .is_some_and(|s| s.is_active));
    }

    #[test]
    fn focus_snapshot_does_not_steal_browse_selection() {
        // Arrow-key browse onto a non-active row must survive a later is_active
        // flip for a different session (external focus / poll).
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
            sample_session("tmux:win:3", 3, "three", false),
        ];
        app.rebuild_rows();
        let browse = app.session_row_index("tmux:win:2").unwrap();
        app.set_selected(browse);
        assert!(!app.session_at(app.selected).unwrap().is_active);

        app.apply_event(ClientEvent::Snapshot {
            sessions: vec![
                sample_session("tmux:win:1", 1, "one", false),
                sample_session("tmux:win:2", 2, "two", false),
                sample_session("tmux:win:3", 3, "three", true),
            ],
            version: 2,
        });

        assert_eq!(
            app.session_at(app.selected).map(|s| s.id.as_str()),
            Some("tmux:win:2"),
            "browse selection must not jump to the newly active session"
        );
    }

    #[test]
    fn state_only_patch_does_not_reorder_rows() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        let older = Utc::now() - chrono::Duration::minutes(10);
        let newer = Utc::now() - chrono::Duration::minutes(1);
        let mut session_a = sample_session("tmux:win:1", 1, "older", false);
        session_a.messaged_at = Some(older);
        let mut session_b = sample_session("tmux:win:2", 2, "newer", true);
        session_b.messaged_at = Some(newer);
        app.sessions = vec![session_a, session_b];
        app.rebuild_rows();
        let top_before = app.rows.iter().find_map(|row| match row {
            crate::bar::ui::RowKind::Session { session } => Some(session.id.clone()),
            _ => None,
        });
        let selected_before = app.session_row_index("tmux:win:2").unwrap();
        app.set_selected(selected_before);

        app.apply_event(ClientEvent::Patch(ServerEvent::Patch {
            session_id: "tmux:win:1".into(),
            state: Some(AgentState::Working),
            title: None,
            description: None,
            cwd: None,
            cwd_label: None,
            project: None,
            is_active: None,
            last_event_at: Some(Utc::now()),
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: None,
            title_manual: None,
            ring_bell: None,
            version: 2,
        }));

        let top_after = app.rows.iter().find_map(|row| match row {
            crate::bar::ui::RowKind::Session { session } => Some(session.id.clone()),
            _ => None,
        });
        assert_eq!(top_before, top_after, "state-only patch must not re-sort");
        assert_eq!(
            app.session_at(app.selected).map(|s| s.id.as_str()),
            Some("tmux:win:2")
        );
    }

    #[test]
    fn optimistic_create_with_focus_selects_new_session() {
        // Warm-pool claim and cold create both use claim_focus=true. Selection
        // must land on the new row after rebuild (not the previous active).
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        app.sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.rebuild_rows();
        let first = app.session_row_index("tmux:win:1").unwrap();
        app.set_selected(first);

        app.push_optimistic_new_session(
            22,
            Some("grok"),
            "/tmp/claimed".into(),
            "~/tmp/claimed".into(),
            "grok · ?",
            true,
        );

        assert_eq!(
            app.session_at(app.selected).map(|s| s.tab_index),
            Some(22),
            "sidebar highlight must follow the claimed/created window"
        );
        assert!(
            app.sessions
                .iter()
                .find(|s| s.tab_index == 22)
                .is_some_and(|s| s.is_active),
            "new session must be the active focus"
        );
        assert_eq!(app.pending_focus_tab_index, Some(22));
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
        // Patches apply before the highest-version snapshot so ring_bell is not
        // version-gated away.
        assert!(matches!(&coalesced[0], ClientEvent::Patch(_)));
        assert!(matches!(
            &coalesced[1],
            ClientEvent::Snapshot { version: 3, .. }
        ));
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
        // Isolated home: live group_order on the developer machine must not
        // force a structure rebuild and break this invariant.
        let dir = tempfile::TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        let sessions = vec![
            sample_session("tmux:win:1", 1, "one", true),
            sample_session("tmux:win:2", 2, "two", false),
        ];
        app.sessions = sessions.clone();
        app.rebuild_rows();
        // Stabilize group order after first rebuild so the next snapshot does not
        // report group_order_changed from reconcile_and_save.
        let _ = app.persist_group_order_from_sessions();
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
    fn snapshot_acknowledgment_syncs_row_completion_state() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        let mut done = sample_session("tmux:win:1", 1, "ship api", false);
        done.state = AgentState::Done;
        done.completed_thread = Some("thread-1".into());
        done.completed_at = Some(Utc::now());
        app.sessions = vec![done.clone()];
        app.rebuild_rows();
        app.acknowledged_completions.insert(
            "tmux:win:1".into(),
            AcknowledgedCompletion {
                thread: "thread-1".into(),
                at: Utc::now(),
            },
        );

        app.apply_event(ClientEvent::Snapshot {
            sessions: vec![done],
            version: 2,
        });

        let row_session = app
            .rows
            .iter()
            .find_map(|row| match row {
                crate::bar::ui::RowKind::Session { session } => Some(session),
                _ => None,
            })
            .expect("expected session row");
        assert_eq!(row_session.state, AgentState::Idle);
        assert!(!row_session.thread_is_complete());
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

    #[test]
    fn patch_messaged_at_bubbles_session_to_top() {
        let config = Config::default();
        let mut app = App::new(&config).unwrap();
        app.selection_initialized = true;
        let older = Utc::now() - chrono::Duration::minutes(10);
        let newer = Utc::now() - chrono::Duration::minutes(1);
        let mut session_a = sample_session("tmux:win:1", 1, "older", false);
        session_a.messaged_at = Some(older);
        let mut session_b = sample_session("tmux:win:2", 2, "newer", false);
        session_b.messaged_at = Some(newer);
        app.sessions = vec![session_a, session_b];
        app.rebuild_rows();

        let top_before = app.rows.iter().find_map(|row| match row {
            crate::bar::ui::RowKind::Session { session } => Some(session.description.as_str()),
            _ => None,
        });
        assert_eq!(top_before, Some("newer"));

        let bumped = Utc::now();
        app.apply_event(ClientEvent::Patch(ServerEvent::Patch {
            session_id: "tmux:win:1".into(),
            state: None,
            title: None,
            description: None,
            cwd: None,
            cwd_label: None,
            project: None,
            is_active: None,
            last_event_at: None,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(bumped),
            prompt_submitted: None,
            title_manual: None,
            ring_bell: None,
            version: 2,
        }));

        let top_after = app.rows.iter().find_map(|row| match row {
            crate::bar::ui::RowKind::Session { session } => Some(session.description.as_str()),
            _ => None,
        });
        assert_eq!(top_after, Some("older"));
    }
}
