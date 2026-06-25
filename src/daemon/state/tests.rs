// WS-08: co-located state tests
use super::*;
    use crate::config::Config;
    use crate::daemon::manifest_sync::ManifestSyncQueue;
    use crate::agents::grok::{grok_events_path, grok_session_dir};
    use crate::pty::{format_tilde_path, is_weak_session_title};
    use crate::session::WorkspaceCatalog;
    use crate::model::{AgentState, NotifyMessage, Session};
    use crate::session::group_order;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_daemon_state(config: Config, sessions: Vec<Session>) -> DaemonState {
        DaemonState::new(config, sessions, Arc::new(ManifestSyncQueue::new()))
    }

    fn write_grok_turn_started(config: &Config, cwd: &str, sid: &str) {
        let session_dir = grok_session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("events.jsonl"),
            r#"{"ts":"2026-06-11T12:00:00.000Z","type":"turn_started"}"#,
        )
        .unwrap();
    }

    fn sample_entry(description: &str) -> Session {
        Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 0,
            kitty_os_window_id: 0,
            tab_index: 1,
            tmux_session: "agents".into(),
            tmux_pane_id: "%1".into(),
            pane_pid: 0,
            agent_session_id: None,
            title: format!("app · {description}"),
            description: description.into(),
            cwd: "/tmp".into(),
            cwd_label: "~/tmp".into(),
            project: "app".into(),
            state: AgentState::Working,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now()),
            prompt_submitted: true,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            managed: false,
            sessions_session_id: None,
            managed_agent: None,
        }
    }

    #[test]
    fn managed_identity_wins_over_inferred_codex_assignment() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let cwd = env!("CARGO_MANIFEST_DIR");
        let bound_sid = "managed-codex-thread";
        let record = crate::session::ManagedLaunchRecord {
            sessions_session_id: "ssn_test".into(),
            launch_id: "lch_test".into(),
            agent: "codex".into(),
            tmux_session: "agents".into(),
            window_index: 4,
            pane_id: Some("%44".into()),
            initial_cwd: cwd.into(),
            created_at: Utc::now().to_rfc3339(),
            agent_session_id: Some(bound_sid.into()),
        };
        crate::session::save_managed_record(&config.home, &record).unwrap();

        let existing = Session {
            managed: true,
            sessions_session_id: Some("ssn_test".into()),
            managed_agent: Some("codex".into()),
            agent_session_id: Some(bound_sid.into()),
            tmux_pane_id: "%44".into(),
            tab_index: 4,
            cwd: cwd.into(),
            ..sample_entry("managed codex")
        };
        let mut fresh = Session {
            managed: true,
            sessions_session_id: Some("ssn_test".into()),
            managed_agent: Some("codex".into()),
            agent_session_id: None,
            tmux_pane_id: "%44".into(),
            tab_index: 4,
            cwd: cwd.into(),
            ..sample_entry("managed codex")
        };

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::from([("%44".to_string(), 4)]),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.agent_session_id.as_deref(), Some(bound_sid));
        assert!(fresh.managed);
    }

    /// Reproduce the suppression set the daemon would cache at poll time, so
    /// tests exercise `sorted_sessions`/`resolve_focus_target` with the same
    /// inputs the production read paths receive.
    fn suppressions_for(config: &Config, sessions: &HashMap<String, Session>) -> HashSet<String> {
        let list: Vec<Session> = sessions.values().cloned().collect();
        agent_session_suppressions(config, &list)
    }

    #[test]
    fn merge_preserves_manual_title_when_agent_session_id_drops() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        write_manual_session_title_files(&config, 3, "grok · my custom name").unwrap();

        let existing = Session {
            tab_index: 3,
            agent_session_id: Some("session-abc".into()),
            title: "grok · my custom name".into(),
            description: "my custom name".into(),
            project: "grok".into(),
            title_manual: true,
            ..sample_entry("ship api")
        };
        let mut fresh = Session {
            tab_index: 3,
            agent_session_id: None,
            title: "grok · auto summary title".into(),
            description: "auto summary title".into(),
            project: "grok".into(),
            title_manual: false,
            ..sample_entry("ship api")
        };

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.title, "grok · my custom name");
        assert_eq!(fresh.description, "my custom name");
        assert!(fresh.title_manual);
    }

    #[test]
    fn prompt_hook_skips_title_refresh_when_manual() {
        let mut entry = sample_entry("ship api");
        entry.title = "grok · custom name".into();
        entry.description = "custom name".into();
        entry.title_manual = true;

        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let workspaces = WorkspaceCatalog::load(&config.workspaces_path);
        let workspace = workspaces.workspace_ref_for_window(entry.tab_index, &entry.cwd);
        let (title, description, project) = resolve_session_names(
            &config.home,
            &entry.cwd,
            Some("grok"),
            entry.agent_session_id.as_deref(),
            &entry.title,
            &entry.description,
            "completely different prompt title",
            workspace,
            true,
        );
        assert!(!is_weak_session_title(&title));
        assert_ne!(title, entry.title);

        // handle_notify gate — manual titles must not be replaced by auto-naming.
        if !entry.title_manual {
            entry.title = title;
            entry.description = description;
            entry.project = project;
        }
        assert_eq!(entry.title, "grok · custom name");
        assert_eq!(entry.description, "custom name");
    }

    #[test]
    fn resolve_renamed_title_keeps_agent_prefix_for_thread_only_input() {
        let session = Session {
            title: "grok · fix sidebar".into(),
            description: "fix sidebar".into(),
            project: "grok".into(),
            ..sample_entry("fix sidebar")
        };
        let (title, description, project) = resolve_renamed_title(&session, "new name");
        assert_eq!(title, "grok · new name");
        assert_eq!(description, "new name");
        assert_eq!(project, "grok");
    }

    #[test]
    fn resolve_renamed_title_accepts_full_display_title() {
        let session = sample_entry("old");
        let (title, description, project) =
            resolve_renamed_title(&session, "codex · refreshed task");
        assert_eq!(title, "codex · refreshed task");
        assert_eq!(description, "refreshed task");
        assert_eq!(project, "codex");
    }

    #[test]
    fn turn_complete_after_acknowledged_completion_with_same_title_is_deduped() {
        let mut entry = sample_entry("ship api");
        entry.state = AgentState::Idle;
        entry.completed_thread = Some("ship api".into());
        entry.completed_at = Some(Utc::now());
        assert!(entry.completion_acknowledged());
        assert!(!entry.thread_is_complete());

        assert_eq!(
            apply_thread_hook(&mut entry, "turn_complete", AgentState::Done),
            None
        );
        assert!(!entry.thread_is_complete());
    }

    #[test]
    fn turn_complete_after_acknowledged_completion_follows_new_prompt() {
        let mut entry = sample_entry("ship api");
        entry.state = AgentState::Idle;
        entry.completed_thread = Some("ship api".into());
        entry.completed_at = Some(Utc::now());
        assert!(entry.completion_acknowledged());

        assert_eq!(
            apply_thread_hook(&mut entry, "prompt", AgentState::Working),
            Some(AgentState::Working)
        );
        assert_eq!(
            apply_thread_hook(&mut entry, "turn_complete", AgentState::Done),
            Some(AgentState::Done)
        );
        assert!(entry.thread_is_complete());
    }

    #[test]
    fn turn_complete_completes_from_approval_state() {
        let mut entry = sample_entry("ship api");
        entry.state = AgentState::Approval;
        assert_eq!(
            apply_thread_hook(&mut entry, "turn_complete", AgentState::Done),
            Some(AgentState::Done)
        );
        assert!(entry.thread_is_complete());
    }

    #[test]
    fn turn_complete_rings_only_when_completion_becomes_visible() {
        let mut entry = sample_entry("ship api");
        entry.state = AgentState::Working;
        assert!(!entry.thread_is_complete());
        assert_eq!(
            apply_thread_hook(&mut entry, "turn_complete", AgentState::Done),
            Some(AgentState::Done)
        );
        assert!(entry.thread_is_complete());
        assert_eq!(
            apply_thread_hook(&mut entry, "turn_complete", AgentState::Done),
            None
        );
    }

    #[test]
    fn stop_marks_thread_done_once() {
        let mut entry = sample_entry("ship api");
        assert_eq!(
            apply_thread_hook(&mut entry, "stop", AgentState::Done),
            Some(AgentState::Done)
        );
        assert!(entry.thread_is_complete());
        assert_eq!(
            apply_thread_hook(&mut entry, "stop", AgentState::Done),
            None
        );
    }

    #[test]
    fn turn_complete_does_not_bump_last_message_timestamp() {
        let mut entry = sample_entry("ship api");
        let messaged_at = Utc::now() - chrono::Duration::minutes(10);
        entry.last_event_at = messaged_at;
        assert_eq!(
            apply_thread_hook(&mut entry, "turn_complete", AgentState::Done),
            Some(AgentState::Done)
        );
        assert_eq!(entry.last_event_at, messaged_at);
    }

    #[test]
    fn turn_complete_sets_completed_at() {
        let mut entry = sample_entry("ship api");
        assert_eq!(
            apply_thread_hook(&mut entry, "turn_complete", AgentState::Done),
            Some(AgentState::Done)
        );
        assert!(entry.completed_at.is_some());
    }

    #[test]
    fn session_start_clears_prompt_submitted() {
        let mut entry = sample_entry("ship api");
        entry.prompt_submitted = true;
        assert_eq!(
            apply_thread_hook(&mut entry, "session_start", AgentState::Idle),
            Some(AgentState::Idle)
        );
        assert!(!entry.prompt_submitted);
    }

    #[test]
    fn prompt_marks_session_as_messaged() {
        let mut entry = sample_entry("ship api");
        entry.prompt_submitted = false;
        entry.messaged_at = None;
        assert!(!entry.prompt_submitted);
        assert_eq!(
            apply_thread_hook(&mut entry, "prompt", AgentState::Working),
            Some(AgentState::Working)
        );
        assert!(entry.prompt_submitted);
        assert!(entry.messaged_at.is_some());
    }

    #[test]
    fn session_start_sets_messaged_at_for_group_order() {
        let mut entry = sample_entry("ship api");
        entry.messaged_at = Some(Utc::now() - chrono::Duration::hours(1));
        assert_eq!(
            apply_thread_hook(&mut entry, "session_start", AgentState::Idle),
            Some(AgentState::Idle)
        );
        assert!(entry.messaged_at.is_some());
        assert!(entry.messaged_at.unwrap() > Utc::now() - chrono::Duration::seconds(2));
    }

    #[test]
    fn tool_hooks_bump_last_event_at() {
        let mut entry = sample_entry("ship api");
        let messaged_at = Utc::now() - chrono::Duration::minutes(8);
        entry.last_event_at = messaged_at;
        assert_eq!(
            apply_thread_hook(&mut entry, "post_tool", AgentState::Working),
            Some(AgentState::Working)
        );
        assert!(entry.last_event_at > messaged_at);
        assert!(entry.shows_run_spinner());
    }

    fn seed_grok_events(home: &Path, session_id: &str, lines: &[&str]) {
        use std::fs;
        let dir = home.join(".grok/sessions").join("proj").join(session_id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("events.jsonl"), lines.join("\n")).unwrap();
    }

    #[test]
    fn sync_messaged_at_from_agent_disk_does_not_bump_hook_ordering() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "550e8400-e29b-41d4-a716-446655440000";
        let prompt_ts = "2026-06-11T10:00:00Z";
        let tool_ts = "2026-06-11T10:05:00Z";
        seed_grok_events(
            &config.home,
            sid,
            &[
                &format!(r#"{{"type":"turn_started","ts":"{prompt_ts}"}}"#),
                &format!(r#"{{"type":"phase_changed","phase":"tool_execution","ts":"{tool_ts}"}}"#),
            ],
        );

        let mut session = sample_entry("ship api");
        session.agent_session_id = Some(sid.into());
        let hook_messaged = chrono::DateTime::parse_from_rfc3339(prompt_ts)
            .unwrap()
            .with_timezone(&Utc);
        session.messaged_at = Some(hook_messaged);
        session.prompt_submitted = true;
        session.last_event_at = hook_messaged;

        sync_messaged_at_from_agent_disk(&config, &mut session);

        assert_eq!(session.messaged_at, Some(hook_messaged));
        assert!(session.last_event_at > hook_messaged);
    }

    #[test]
    fn sync_messaged_at_from_agent_disk_hydrates_from_turn_started_not_tool_hooks() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "550e8400-e29b-41d4-a716-446655440001";
        let prompt_ts = "2026-06-11T10:00:00Z";
        let tool_ts = "2026-06-11T10:05:00Z";
        seed_grok_events(
            &config.home,
            sid,
            &[
                &format!(r#"{{"type":"turn_started","ts":"{prompt_ts}"}}"#),
                &format!(r#"{{"type":"phase_changed","phase":"tool_execution","ts":"{tool_ts}"}}"#),
            ],
        );

        let mut session = sample_entry("ship api");
        session.agent_session_id = Some(sid.into());
        session.messaged_at = None;
        session.prompt_submitted = false;
        session.last_event_at = Utc::now() - chrono::Duration::hours(1);

        sync_messaged_at_from_agent_disk(&config, &mut session);

        let expected = chrono::DateTime::parse_from_rfc3339(prompt_ts)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(session.messaged_at, Some(expected));
    }

    #[test]
    fn post_tool_ignored_after_thread_complete() {
        let mut entry = sample_entry("ship api");
        let _ = apply_thread_hook(&mut entry, "stop", AgentState::Done);
        assert_eq!(
            apply_thread_hook(&mut entry, "post_tool", AgentState::Working),
            None
        );
        assert_eq!(entry.display_state(), AgentState::Done);
    }

    #[test]
    fn tool_hooks_resume_after_acknowledged_completion() {
        let mut entry = sample_entry("ship api");
        let _ = apply_thread_hook(&mut entry, "stop", AgentState::Done);
        assert!(entry.acknowledge_if_done());
        assert_eq!(
            apply_thread_hook(&mut entry, "pre_tool", AgentState::Approval),
            Some(AgentState::Approval)
        );
        assert_eq!(entry.state, AgentState::Approval);
        assert_eq!(entry.completed_thread, None);
        assert!(entry.shows_run_spinner());
    }

    #[test]
    fn refresh_merge_promotes_idle_to_working_from_grok_phase() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019ea057-3abe-74e2-b130-2f01c3dd1988";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let events_dir = grok_session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&events_dir).unwrap();
        std::fs::write(
            grok_events_path(&config.home, cwd, sid),
            r#"{"ts":"2026-06-07T04:38:45.548Z","type":"turn_started"}
{"ts":"2026-06-07T04:40:09.787Z","type":"phase_changed","phase":"streaming_reasoning"}
"#,
        )
        .unwrap();

        let mut existing = sample_entry("ship api");
        existing.agent_session_id = Some(sid.into());
        existing.cwd = cwd.into();
        existing.state = AgentState::Idle;
        existing.completed_thread = Some("ship api".into());
        let mut fresh = existing.clone();
        fresh.state = AgentState::Idle;
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.state, AgentState::Working);
        assert_eq!(fresh.completed_thread, None);
        assert!(fresh.shows_run_spinner());
    }

    #[test]
    fn prompt_starts_new_thread() {
        let mut entry = sample_entry("ship api");
        let _ = apply_thread_hook(&mut entry, "stop", AgentState::Done);
        entry.description = "fix tests".into();
        assert_eq!(
            apply_thread_hook(&mut entry, "prompt", AgentState::Working),
            Some(AgentState::Working)
        );
        assert_eq!(entry.completed_thread, None);
        assert_eq!(entry.display_state(), AgentState::Working);
    }

    #[test]
    fn coalesce_agent_state_prefers_live_pane_state() {
        assert_eq!(
            coalesce_agent_state(AgentState::Idle, AgentState::Approval, false),
            AgentState::Approval
        );
        assert_eq!(
            coalesce_agent_state(AgentState::Working, AgentState::Idle, false),
            AgentState::Working
        );
        assert_eq!(
            coalesce_agent_state(AgentState::Done, AgentState::Working, false),
            AgentState::Done
        );
        assert_eq!(
            coalesce_agent_state(AgentState::Working, AgentState::Done, false),
            AgentState::Working
        );
        assert_eq!(
            coalesce_agent_state(AgentState::Working, AgentState::Done, true),
            AgentState::Done
        );
    }

    #[test]
    fn refresh_merge_accepts_lifecycle_completion_from_poll() {
        let config = Config::default();
        let existing = sample_entry("refactor auth");
        let mut fresh = sample_entry("refactor auth");
        fresh.state = AgentState::Done;
        fresh.completed_thread = Some("refactor auth".into());
        fresh.completed_at = Some(Utc::now());
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert!(fresh.thread_is_complete());
        assert_eq!(fresh.completed_thread.as_deref(), Some("refactor auth"));
    }

    #[test]
    fn refresh_merge_ignores_stale_done_from_pane_poll() {
        let config = Config::default();
        let existing = sample_entry("live thread");
        let mut fresh = sample_entry("live thread");
        fresh.state = AgentState::Done;
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.state, AgentState::Working);
        assert_eq!(fresh.completed_thread, None);
    }

    #[test]
    fn refresh_merge_marks_complete_from_grok_turn_ended() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019ea009-b0b4-7e41-b767-43993c604b7f";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let events_dir = grok_session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&events_dir).unwrap();
        std::fs::write(
            grok_events_path(&config.home, cwd, sid),
            r#"{"ts":"2026-06-07T03:22:20.361Z","type":"turn_started"}
{"ts":"2026-06-07T03:22:46.584Z","type":"turn_ended","outcome":"completed"}
"#,
        )
        .unwrap();

        let mut existing = sample_entry("ship api");
        existing.agent_session_id = Some(sid.into());
        existing.cwd = cwd.into();
        existing.state = AgentState::Approval;
        let mut fresh = existing.clone();
        fresh.state = AgentState::Approval;
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert!(fresh.thread_is_complete());
        assert!(!fresh.shows_run_spinner());
        assert_eq!(fresh.completed_thread.as_deref(), Some("ship api"));
    }

    #[test]
    fn sync_live_activity_ignores_stale_disk_after_acknowledged_completion() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019ea057-3abe-74e2-b130-2f01c3dd1988";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let events_dir = crate::agents::grok::session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&events_dir).unwrap();
        std::fs::write(
            events_dir.join("events.jsonl"),
            r#"{"ts":"2026-06-07T04:38:45.548Z","type":"turn_started"}
{"ts":"2026-06-07T04:40:09.787Z","type":"phase_changed","phase":"streaming_reasoning"}
"#,
        )
        .unwrap();

        let mut session = sample_entry("ship api");
        session.agent_session_id = Some(sid.into());
        session.cwd = cwd.into();
        session.state = AgentState::Idle;
        session.completed_thread = Some("ship api".into());
        session.completed_at = Some(
            chrono::DateTime::parse_from_rfc3339("2026-06-07T04:45:00.000Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        sync_live_activity_from_disk(&config, &mut session);
        assert_eq!(session.state, AgentState::Idle);
        assert_eq!(session.completed_thread.as_deref(), Some("ship api"));
        assert!(!session.shows_run_spinner());
    }

    #[test]
    fn refresh_merge_clears_acknowledged_completion_when_pane_is_working() {
        let config = Config::default();
        let mut existing = sample_entry("done thread");
        existing.state = AgentState::Idle;
        existing.completed_thread = Some("done thread".into());
        let mut fresh = sample_entry("done thread");
        fresh.state = AgentState::Working;
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.state, AgentState::Working);
        assert_eq!(fresh.completed_thread, None);
    }

    #[test]
    fn refresh_merge_preserves_acknowledged_completion() {
        let config = Config::default();
        let mut existing = sample_entry("done thread");
        existing.state = AgentState::Idle;
        existing.completed_thread = Some("done thread".into());
        let mut fresh = sample_entry("done thread");
        fresh.state = AgentState::Done;
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.state, AgentState::Idle);
        assert_eq!(fresh.completed_thread, Some("done thread".into()));
    }

    #[test]
    fn acknowledge_completion_on_focus_only_when_session_becomes_active() {
        let mut completed = sample_entry("ship api");
        completed.state = AgentState::Done;
        completed.completed_thread = Some("ship api".into());
        completed.is_active = true;

        let mut still_active = completed.clone();
        let prior_active = completed.clone();
        assert!(!acknowledge_completion_on_focus(
            Some(&prior_active),
            &mut still_active
        ));
        assert!(still_active.thread_is_complete());

        let mut newly_active = completed;
        newly_active.is_active = true;
        assert!(acknowledge_completion_on_focus(None, &mut newly_active));
        assert!(!newly_active.thread_is_complete());
        assert!(newly_active.completion_acknowledged());
    }

    #[test]
    fn refresh_merge_preserves_completed_thread_timestamp() {
        let config = Config::default();
        let mut existing = sample_entry("done thread");
        existing.state = AgentState::Done;
        existing.completed_thread = Some("done thread".into());
        let completed_at = Utc::now() - chrono::Duration::minutes(5);
        existing.last_event_at = completed_at;
        let mut fresh = sample_entry("other title");
        fresh.state = AgentState::Done;
        fresh.last_event_at = Utc::now();
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert!(existing.thread_is_complete());
        assert_eq!(fresh.last_event_at, completed_at);
        assert_eq!(fresh.description, "done thread");
        assert!(fresh.thread_is_complete());
    }

    #[test]
    fn refresh_merge_relabels_workspace_when_foreground_tool_changes() {
        let config = Config::default();
        let mut existing = sample_entry("frontend dev");
        existing.title = "superflip · frontend dev".into();
        existing.project = "superflip".into();
        existing.cwd = "/home/testuser/projects/superflip/superflip-frontend".into();
        let mut fresh = sample_entry("htop");
        fresh.title = "htop".into();
        fresh.project = "superflip".into();
        fresh.cwd = existing.cwd.clone();
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.title, "htop");
        assert_eq!(fresh.description, "htop");
    }

    #[test]
    fn refresh_merge_relabels_ad_hoc_terminal_to_console() {
        let config = Config::default();
        let mut existing = sample_entry("frontend dev");
        existing.title = "superflip · frontend dev".into();
        existing.project = "superflip".into();
        existing.cwd = "/home/testuser/projects/superflip/superflip-frontend".into();
        let mut fresh = sample_entry("console");
        fresh.title = "console".into();
        fresh.project = "".into();
        fresh.cwd = existing.cwd.clone();
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.title, "console");
        assert_eq!(fresh.description, "console");
    }

    #[test]
    fn refresh_merge_relabels_false_grok_tool_prefix() {
        let config = Config::default();
        let mut existing = sample_entry("htop");
        existing.title = "grok · htop".into();
        existing.project = "grok".into();
        let mut fresh = sample_entry("htop");
        fresh.title = "htop".into();
        fresh.project = "sessions-cli".into();
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.title, "htop");
        assert_eq!(fresh.project, "sessions-cli");
    }

    #[test]
    fn refresh_merge_keeps_poll_tool_title_when_agent_session_clears() {
        let config = Config::default();
        let mut existing = sample_entry("ship api");
        existing.agent_session_id = Some("sid-grok".into());
        existing.title = "grok · ship api".into();
        existing.description = "ship api".into();
        existing.project = "grok".into();

        let mut fresh = sample_entry("cargo run --release");
        fresh.agent_session_id = None;
        fresh.title = "cargo · cargo run --release".into();
        fresh.description = "cargo run --release".into();
        fresh.project = "cargo".into();

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.agent_session_id, None);
        assert_eq!(fresh.title, "cargo · cargo run --release");
        assert_eq!(fresh.description, "cargo run --release");
        assert_eq!(fresh.project, "cargo");
    }

    #[test]
    fn refresh_merge_ignores_stale_tab_manual_tool_title_on_recycled_window() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        write_manual_session_title_files(&config, 33, "opencode").unwrap();

        let mut existing = sample_entry("opencode");
        existing.tab_index = 33;
        existing.tmux_pane_id = "%509".into();
        existing.title = "opencode".into();
        existing.project = "grok".into();
        existing.title_manual = true;
        existing.state = AgentState::Idle;

        let mut fresh = sample_entry("console");
        fresh.tab_index = 33;
        fresh.tmux_pane_id = "%509".into();
        fresh.title = "console".into();
        fresh.project = "".into();
        fresh.state = AgentState::Idle;

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.title, "console");
        assert_eq!(fresh.description, "console");
        assert!(!fresh.title_manual);
        assert!(!manual_title_marker_exists(&config, 33));
    }

    #[test]
    fn refresh_merge_upgrades_grok_binary_placeholder_title() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019eb049-9fdc-77d2-bd4a-faac4cfbb0c1";
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path(sid),
            "TMUX_PANE=%505\nSESSIONS_WINDOW_INDEX=22\nTMUX_SESSION=agents\n",
        )
        .unwrap();
        let mut existing = sample_entry("grok-0239-mac");
        existing.tab_index = 22;
        existing.tmux_pane_id = "%505".into();
        existing.title = "grok · grok-0239-mac".into();
        existing.agent_session_id = Some(sid.into());
        let mut fresh = sample_entry("Fix Cmd+Num Shortcuts After Collapsing PWD Headers");
        fresh.tab_index = 22;
        fresh.tmux_pane_id = "%505".into();
        fresh.title = "grok · Fix Cmd+Num Shortcuts After Collapsing PWD Headers".into();
        fresh.agent_session_id = Some(sid.into());
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(
            fresh.description,
            "Fix Cmd+Num Shortcuts After Collapsing PWD Headers"
        );
        assert!(!fresh.description.contains("grok-0239"));
    }

    #[test]
    fn refresh_merge_prefers_summary_over_machine_derived_hook_title() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019eb049-9fdc-77d2-bd4a-faac4cfbb0c1";
        let cwd = env!("CARGO_MANIFEST_DIR");
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(config.session_title_path(sid), "grok · grok-0239-mac\n").unwrap();
        let summary_dir = grok_session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            r#"{"generated_title":"Fix Cmd+Num Shortcuts After Collapsing PWD Headers in Sessions"}"#,
        )
        .unwrap();
        write_grok_turn_started(&config, cwd, sid);

        let mut existing = sample_entry("grok-0239-mac");
        existing.cwd = cwd.into();
        existing.title = "grok · grok-0239-mac".into();
        existing.agent_session_id = Some(sid.into());
        let mut fresh = sample_entry("grok-0239-mac");
        fresh.cwd = cwd.into();
        fresh.title = "grok · grok-0239-mac".into();
        fresh.agent_session_id = Some(sid.into());

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(
            fresh.description,
            "Fix Cmd+Num Shortcuts After Collapsing PWD Headers in Sessions"
        );
        assert!(!fresh.description.contains("grok-0239"));
    }

    #[test]
    fn refresh_merge_upgrades_completed_grok_placeholder_title() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019eb049-9fdc-77d2-bd4a-faac4cfbb0c1";
        let cwd = env!("CARGO_MANIFEST_DIR");
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(config.session_title_path(sid), "grok · grok-0239-mac\n").unwrap();
        let summary_dir = grok_session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            r#"{"generated_title":"Fix Cmd+Num Shortcuts After Collapsing PWD Headers in Sessions"}"#,
        )
        .unwrap();
        write_grok_turn_started(&config, cwd, sid);

        let mut existing = sample_entry("grok-0239-mac");
        existing.cwd = cwd.into();
        existing.title = "grok · grok-0239-mac".into();
        existing.agent_session_id = Some(sid.into());
        existing.state = AgentState::Done;
        existing.completed_thread = Some("grok-0239-mac".into());
        let mut fresh = sample_entry("grok-0239-mac");
        fresh.cwd = cwd.into();
        fresh.title = "grok · grok-0239-mac".into();
        fresh.agent_session_id = Some(sid.into());
        fresh.state = AgentState::Done;
        fresh.completed_thread = Some("grok-0239-mac".into());

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert!(fresh.thread_is_complete());
        assert_eq!(
            fresh.description,
            "Fix Cmd+Num Shortcuts After Collapsing PWD Headers in Sessions"
        );
        assert_eq!(
            fresh.completed_thread.as_deref(),
            Some("Fix Cmd+Num Shortcuts After Collapsing PWD Headers in Sessions")
        );
    }

    #[test]
    fn refresh_merge_preserves_running_title_over_poll_rewrite() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut existing = sample_entry("live thread");
        existing.title = "grok · live thread".into();
        existing.agent_session_id = Some("sid-live".into());
        let mut fresh = sample_entry("stale snippet");
        fresh.title = "grok · stale snippet".into();
        fresh.agent_session_id = Some("sid-live".into());
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.description, "live thread");
        assert_eq!(fresh.title, "grok · live thread");
    }

    #[test]
    fn refresh_merge_restores_grok_summary_for_placeholder_managed_session() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let sid = "019ebb73-88d4-7083-9cd8-74c948855d84";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let session_dir = grok_session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("summary.json"),
            r#"{
  "info": {"id": "019ebb73-88d4-7083-9cd8-74c948855d84", "cwd": "/Users/ethan/projects/sessions-cli"},
  "session_summary": "Control N New Section Failure in Active Project Background Process"
}"#,
        )
        .unwrap();
        std::fs::write(
            session_dir.join("events.jsonl"),
            r#"{"ts":"2026-06-12T10:49:59.194Z","type":"turn_started","session_id":"019ebb73-88d4-7083-9cd8-74c948855d84"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path(sid),
            "TMUX_PANE=%683\nSESSIONS_WINDOW_INDEX=9\nTMUX_SESSION=agents\n",
        )
        .unwrap();

        let existing = Session {
            agent_session_id: Some(sid.into()),
            title: "grok · ?".into(),
            description: "?".into(),
            project: "grok".into(),
            managed: true,
            sessions_session_id: Some("ssn_test".into()),
            cwd: cwd.into(),
            tab_index: 9,
            tmux_pane_id: "%683".into(),
            prompt_submitted: true,
            messaged_at: Some(Utc::now()),
            ..sample_entry("sessions-cli")
        };
        let mut fresh = existing.clone();

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(
            fresh.description,
            "Control N New Section Failure in Active Project Background Process"
        );
        assert_eq!(
            fresh.title,
            "grok · Control N New Section Failure in Active Project Background Process"
        );
        assert!(config.session_title_path(sid).exists());
    }

    #[test]
    fn refresh_merge_preserves_sticky_title_over_machine_derived_poll() {
        let config = Config::default();
        let mut existing = sample_entry("fix sidebar");
        existing.title = "grok · fix sidebar".into();
        existing.agent_session_id = Some("sid-live".into());
        let mut fresh = sample_entry("grok");
        fresh.title = "grok".into();
        fresh.description = "grok".into();
        fresh.agent_session_id = Some("sid-live".into());
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.description, "fix sidebar");
        assert_eq!(fresh.title, "grok · fix sidebar");
    }

    #[test]
    fn bootstrap_console_upgrades_to_commenced_summary() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019ec89f-fa0c-7a13-8acc-8c837c62d193";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let summary_dir = grok_session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            r#"{"generated_title":"Fix new sessions disappearing from sidebar after background send"}"#,
        )
        .unwrap();
        write_grok_turn_started(&config, cwd, sid);

        let mut existing = sample_entry("console");
        existing.cwd = cwd.into();
        existing.title = "grok · console".into();
        existing.description = "console".into();
        existing.project = "grok".into();
        existing.agent_session_id = Some(sid.into());
        existing.state = AgentState::Working;
        let mut fresh = sample_entry("console");
        fresh.cwd = cwd.into();
        fresh.title = "grok · console".into();
        fresh.description = "console".into();
        fresh.project = "grok".into();
        fresh.agent_session_id = Some(sid.into());
        fresh.state = AgentState::Working;

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(
            fresh.description,
            "Fix new sessions disappearing from sidebar after background send"
        );
        let title_file = std::fs::read_to_string(config.session_title_path(sid)).unwrap();
        assert!(title_file.contains("Fix new sessions disappearing"));
        assert!(!title_file.contains("console"));
    }

    #[tokio::test]
    async fn bootstrap_console_not_persisted_on_hook() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019ec89f-fa0c-7a13-8acc-8c837c62d193";
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path(sid),
            "TMUX_PANE=%49\nSESSIONS_WINDOW_INDEX=6\nTMUX_SESSION=agents\n",
        )
        .unwrap();

        let mut session = sample_entry("console");
        session.id = "tmux:win:6".into();
        session.tab_index = 6;
        session.kitty_window_id = 6;
        session.tmux_pane_id = "%49".into();
        session.title = "grok · console".into();
        session.description = "console".into();
        session.project = "grok".into();
        session.agent_session_id = Some(sid.into());
        session.state = AgentState::Working;

        let state = test_daemon_state(config.clone(), vec![session]);
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: Some("grok".into()),
            session_id: Some(sid.into()),
            kitty_window_id: None,
            tmux_pane_id: Some("%49".into()),
            tmux_session: Some("agents".into()),
            event: "prompt".into(),
            ts: 1,
            payload: serde_json::json!({}),
            cwd: Some(env!("CARGO_MANIFEST_DIR").into()),
            kitty_pid: None,
            kitty_listen_on: None,
            ..Default::default()
        };

        assert!(state.handle_notify(&msg).await.is_some());
        assert!(!config.session_title_path(sid).exists());
    }

    #[test]
    fn stale_title_file_bootstrap_loses_to_summary() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019ec89f-fa0c-7a13-8acc-8c837c62d193";
        let cwd = env!("CARGO_MANIFEST_DIR");
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(config.session_title_path(sid), "grok · console\n").unwrap();
        let summary_dir = grok_session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            r#"{"generated_title":"Fix new sessions disappearing from sidebar after background send"}"#,
        )
        .unwrap();
        write_grok_turn_started(&config, cwd, sid);

        let mut existing = sample_entry("console");
        existing.cwd = cwd.into();
        existing.title = "grok · console".into();
        existing.description = "console".into();
        existing.project = "grok".into();
        existing.agent_session_id = Some(sid.into());
        let mut fresh = sample_entry("console");
        fresh.cwd = cwd.into();
        fresh.title = "grok · console".into();
        fresh.description = "console".into();
        fresh.project = "grok".into();
        fresh.agent_session_id = Some(sid.into());

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(
            fresh.description,
            "Fix new sessions disappearing from sidebar after background send"
        );
        let title_file = std::fs::read_to_string(config.session_title_path(sid)).unwrap();
        assert!(!title_file.contains("console"));
    }

    #[test]
    fn non_bootstrap_short_prompt_preserved() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut existing = sample_entry("refactor");
        existing.title = "grok · refactor".into();
        existing.agent_session_id = Some("sid-live".into());
        let mut fresh = sample_entry("grok");
        fresh.title = "grok".into();
        fresh.description = "grok".into();
        fresh.agent_session_id = Some("sid-live".into());
        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );
        assert_eq!(fresh.description, "refactor");
        assert_eq!(fresh.title, "grok · refactor");
    }

    #[test]
    fn ensure_agent_session_title_upgrades_console() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "019ec89f-fa0c-7a13-8acc-8c837c62d193";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let summary_dir = grok_session_dir(&config.home, cwd, sid);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            r#"{"generated_title":"Fix new sessions disappearing from sidebar after background send"}"#,
        )
        .unwrap();
        write_grok_turn_started(&config, cwd, sid);

        let existing = Session {
            agent_session_id: Some(sid.into()),
            title: "grok · console".into(),
            description: "console".into(),
            project: "grok".into(),
            cwd: cwd.into(),
            messaged_at: Some(Utc::now()),
            ..sample_entry("console")
        };
        let mut session = existing.clone();

        ensure_agent_session_title(&config, &existing, &mut session);

        assert_eq!(
            session.description,
            "Fix new sessions disappearing from sidebar after background send"
        );
    }

    #[test]
    fn session_start_rotation_does_not_carry_prior_thread_name() {
        let config = Config::default();
        let mut entry = Session {
            agent_session_id: Some("sid-old".into()),
            title: "grok · old task".into(),
            description: "old task".into(),
            project: "grok".into(),
            ..sample_entry("old task")
        };
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: Some("grok".into()),
            session_id: Some("sid-new".into()),
            tmux_pane_id: Some("%1".into()),
            tmux_session: Some("agents".into()),
            event: "session_start".into(),
            ts: 0,
            payload: serde_json::json!({}),
            ..Default::default()
        };
        let workspace = WorkspaceCatalog::default();
        let prior_agent_sid = entry.agent_session_id.clone();
        let event = "session_start";
        let agent_session_rotated = event == "session_start"
            && prior_agent_sid
                .as_deref()
                .zip(msg.session_id.as_deref())
                .is_some_and(|(old, new)| old != new);
        entry.agent_session_id = msg.session_id.clone();
        let (title, description, _) = resolve_session_names(
            &config.home,
            &entry.cwd,
            msg.agent.as_deref(),
            entry.agent_session_id.as_deref(),
            if agent_session_rotated {
                ""
            } else {
                entry.title.as_str()
            },
            if agent_session_rotated {
                ""
            } else {
                entry.description.as_str()
            },
            "",
            workspace.workspace_ref_for_window(entry.tab_index, &entry.cwd),
            false,
        );
        assert!(agent_session_rotated);
        assert_eq!(description, "?");
        assert_eq!(title, "grok · ?");
    }

    #[test]
    fn compute_refresh_merges_by_sessions_session_id_preserves_order() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let older = Utc::now() - chrono::Duration::hours(2);
        let newer = Utc::now() - chrono::Duration::hours(1);

        let mut prior_a = sample_entry("thread a");
        prior_a.id = "tmux:win:1".into();
        prior_a.tab_index = 1;
        prior_a.tmux_pane_id = "%1".into();
        prior_a.managed = true;
        prior_a.sessions_session_id = Some("ssn_restore_a".into());
        prior_a.messaged_at = Some(older);

        let mut prior_b = sample_entry("thread b");
        prior_b.id = "tmux:win:2".into();
        prior_b.tab_index = 2;
        prior_b.tmux_pane_id = "%2".into();
        prior_b.managed = true;
        prior_b.sessions_session_id = Some("ssn_restore_b".into());
        prior_b.messaged_at = Some(newer);

        let previous = HashMap::from([
            (prior_a.id.clone(), prior_a),
            (prior_b.id.clone(), prior_b),
        ]);

        let mut fresh_a = sample_entry("thread a");
        fresh_a.id = "tmux:win:5".into();
        fresh_a.tab_index = 5;
        fresh_a.tmux_pane_id = "%5".into();
        fresh_a.managed = true;
        fresh_a.sessions_session_id = Some("ssn_restore_a".into());
        fresh_a.messaged_at = None;

        let mut fresh_b = sample_entry("thread b");
        fresh_b.id = "tmux:win:6".into();
        fresh_b.tab_index = 6;
        fresh_b.tmux_pane_id = "%6".into();
        fresh_b.managed = true;
        fresh_b.sessions_session_id = Some("ssn_restore_b".into());
        fresh_b.messaged_at = None;

        let out = compute_refresh(
            &config,
            vec![fresh_a, fresh_b],
            &previous,
            &std::collections::HashSet::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(out.merged.len(), 2);
        let merged_a = out
            .merged
            .iter()
            .find(|session| session.sessions_session_id.as_deref() == Some("ssn_restore_a"))
            .unwrap();
        let merged_b = out
            .merged
            .iter()
            .find(|session| session.sessions_session_id.as_deref() == Some("ssn_restore_b"))
            .unwrap();
        assert_eq!(merged_a.messaged_at, Some(older));
        assert_eq!(merged_b.messaged_at, Some(newer));
        assert!(merged_a.messaged_at.unwrap() < merged_b.messaged_at.unwrap());
    }

    #[test]
    fn compute_refresh_rejects_direct_index_match_when_sessions_session_id_changes() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let mut stale_at_index = sample_entry("stale at index");
        stale_at_index.id = "tmux:win:3".into();
        stale_at_index.tab_index = 3;
        stale_at_index.tmux_pane_id = "%old".into();
        stale_at_index.managed = true;
        stale_at_index.sessions_session_id = Some("ssn_moved".into());
        stale_at_index.title = "grok · moved thread".into();

        let previous = HashMap::from([(stale_at_index.id.clone(), stale_at_index)]);

        let mut fresh_workspace = sample_entry("workspace");
        fresh_workspace.id = "tmux:win:3".into();
        fresh_workspace.tab_index = 3;
        fresh_workspace.tmux_pane_id = "%new".into();
        fresh_workspace.managed = true;
        fresh_workspace.sessions_session_id = Some("ws:5:abc".into());
        fresh_workspace.title = "aeo · copy optimiser".into();

        let mut fresh_moved = sample_entry("moved thread");
        fresh_moved.id = "tmux:win:17".into();
        fresh_moved.tab_index = 17;
        fresh_moved.tmux_pane_id = "%moved".into();
        fresh_moved.managed = true;
        fresh_moved.sessions_session_id = Some("ssn_moved".into());
        fresh_moved.title = "grok · moved thread".into();

        let out = compute_refresh(
            &config,
            vec![fresh_workspace, fresh_moved],
            &previous,
            &std::collections::HashSet::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(out.merged.len(), 2);
        let workspace = out
            .merged
            .iter()
            .find(|session| session.sessions_session_id.as_deref() == Some("ws:5:abc"))
            .expect("workspace row");
        assert_eq!(workspace.tab_index, 3);
        assert_eq!(workspace.title, "aeo · copy optimiser");

        let moved = out
            .merged
            .iter()
            .find(|session| session.sessions_session_id.as_deref() == Some("ssn_moved"))
            .expect("moved row");
        assert_eq!(moved.tab_index, 17);
        assert_eq!(out.merged.iter().filter(|session| {
            session.sessions_session_id.as_deref() == Some("ssn_moved")
        }).count(), 1);
    }

    #[test]
    fn compute_refresh_hydrates_manifest_messaged_at_without_stamp_new_order() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let manifest_at = Utc::now() - chrono::Duration::days(1);
        crate::session::manifest::append_entry(
            &config,
            crate::session::manifest::ManifestEntry {
                sessions_session_id: "ssn_manifest".into(),
                source: crate::session::manifest::ManifestSource::NewChat,
                workspace_index: None,
                cwd: "/tmp/project".into(),
                cwd_label: "~/tmp/project".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: Some("agent-manifest".into()),
                title: Some("grok · restored title".into()),
                messaged_at: Some(manifest_at),
                closed: false,
            },
        )
        .unwrap();

        let mut fresh = sample_entry("?");
        fresh.id = "tmux:win:9".into();
        fresh.tab_index = 9;
        fresh.tmux_pane_id = "%9".into();
        fresh.managed = true;
        fresh.sessions_session_id = Some("ssn_manifest".into());
        fresh.messaged_at = None;

        let out = compute_refresh(
            &config,
            vec![fresh],
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(out.merged.len(), 1);
        let merged = &out.merged[0];
        assert_eq!(merged.messaged_at, Some(manifest_at));
        assert_eq!(merged.title, "grok · restored title");
        assert_eq!(merged.description, "restored title");
        assert_eq!(
            merged.agent_session_id.as_deref(),
            Some("agent-manifest")
        );
    }

    #[test]
    fn compute_refresh_restored_managed_skips_stamp_when_manifest_lacks_messaged_at() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        crate::session::manifest::append_entry(
            &config,
            crate::session::manifest::ManifestEntry {
                sessions_session_id: "ssn_new".into(),
                source: crate::session::manifest::ManifestSource::WorkspaceBootstrap,
                workspace_index: Some(0),
                cwd: "/tmp/project".into(),
                cwd_label: "~/tmp/project".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: None,
                title: None,
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let mut fresh = sample_entry("?");
        fresh.id = "tmux:win:3".into();
        fresh.tab_index = 3;
        fresh.tmux_pane_id = "%3".into();
        fresh.managed = true;
        fresh.sessions_session_id = Some("ssn_new".into());
        fresh.messaged_at = None;

        let out = compute_refresh(
            &config,
            vec![fresh],
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(out.merged.len(), 1);
        let merged = &out.merged[0];
        assert!(
            merged.messaged_at.is_none(),
            "restored bootstrap rows must not get a poll-time stamp"
        );
    }

    #[test]
    fn compute_refresh_hydrates_launch_messaged_at_from_manifest() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let launch_at = Utc::now() - chrono::Duration::minutes(1);
        crate::session::manifest::append_entry(
            &config,
            crate::session::manifest::ManifestEntry {
                sessions_session_id: "ssn_fresh".into(),
                source: crate::session::manifest::ManifestSource::Cli,
                workspace_index: None,
                cwd: "/tmp/project".into(),
                cwd_label: "~/tmp/project".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: None,
                title: None,
                messaged_at: Some(launch_at),
                closed: false,
            },
        )
        .unwrap();

        let mut fresh = sample_entry("?");
        fresh.id = "tmux:win:3".into();
        fresh.tab_index = 3;
        fresh.tmux_pane_id = "%3".into();
        fresh.managed = true;
        fresh.sessions_session_id = Some("ssn_fresh".into());
        fresh.messaged_at = None;

        let out = compute_refresh(
            &config,
            vec![fresh],
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(out.merged[0].messaged_at, Some(launch_at));
    }

    #[test]
    fn compute_refresh_manifest_messaged_at_overrides_stale_prior_stamp() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let manifest_at = Utc::now() - chrono::Duration::hours(4);
        crate::session::manifest::append_entry(
            &config,
            crate::session::manifest::ManifestEntry {
                sessions_session_id: "ssn_stale_stamp".into(),
                source: crate::session::manifest::ManifestSource::NewChat,
                workspace_index: None,
                cwd: "/tmp/project".into(),
                cwd_label: "~/tmp/project".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: Some("agent-stale".into()),
                title: Some("grok · restored".into()),
                messaged_at: Some(manifest_at),
                closed: false,
            },
        )
        .unwrap();

        let mut prior = sample_entry("?");
        prior.id = "tmux:win:3".into();
        prior.tab_index = 3;
        prior.tmux_pane_id = "%3".into();
        prior.managed = true;
        prior.sessions_session_id = Some("ssn_stale_stamp".into());
        prior.messaged_at = Some(Utc::now());
        let previous = HashMap::from([(prior.id.clone(), prior)]);

        let mut fresh = sample_entry("?");
        fresh.id = "tmux:win:3".into();
        fresh.tab_index = 3;
        fresh.tmux_pane_id = "%3".into();
        fresh.managed = true;
        fresh.sessions_session_id = Some("ssn_stale_stamp".into());
        fresh.messaged_at = None;

        let out = compute_refresh(
            &config,
            vec![fresh],
            &previous,
            &std::collections::HashSet::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(out.merged[0].messaged_at, Some(manifest_at));
    }

    #[test]
    fn compute_refresh_stamps_unmanaged_session_without_manifest_entry() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let mut fresh = sample_entry("?");
        fresh.id = "tmux:win:3".into();
        fresh.tab_index = 3;
        fresh.tmux_pane_id = "%3".into();
        fresh.managed = false;
        fresh.messaged_at = None;

        let out = compute_refresh(
            &config,
            vec![fresh],
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &WorkspaceCatalog::default(),
        );

        assert!(out.merged[0].messaged_at.is_some());
        assert!(
            out.merged[0].messaged_at.unwrap() > Utc::now() - chrono::Duration::seconds(2)
        );
    }

    #[test]
    fn compute_refresh_filters_closed_but_reports_all_polled_identities() {
        let config = Config::default();
        let previous = HashMap::new();

        let mut win1 = sample_entry("one");
        win1.id = "tmux:win:1".into();
        win1.tab_index = 1;
        win1.tmux_pane_id = "%1".into();
        win1.agent_session_id = Some("sid-1".into());

        let mut win2 = sample_entry("two");
        win2.id = "tmux:win:2".into();
        win2.tab_index = 2;
        win2.tmux_pane_id = "%2".into();
        win2.agent_session_id = Some("sid-2".into());

        // win2 is marked closed — it must be merged out, but its identity must
        // still be reported so the close marker can be expired once the pane
        // actually disappears from tmux.
        let mut closed = std::collections::HashSet::new();
        closed.insert(ClosedSessionMarker::from_session(&win2));

        let out = compute_refresh(
            &config,
            vec![win1, win2],
            &previous,
            &closed,
            &WorkspaceCatalog::default(),
        );

        assert_eq!(out.merged.len(), 1, "closed session is filtered from merge");
        assert_eq!(out.merged[0].id, "tmux:win:1");
        // polled_ids reflects only survivors (drives vanished-session removal)...
        assert!(out.polled_ids.contains("tmux:win:1"));
        assert!(!out.polled_ids.contains("tmux:win:2"));
        // ...but pane/sid identity sets cover every polled window, so a live
        // close marker for the still-present pane is not prematurely dropped.
        assert!(out.polled_panes.contains("%1"));
        assert!(out.polled_panes.contains("%2"));
        assert!(out.polled_sids.contains("sid-1"));
        assert!(out.polled_sids.contains("sid-2"));
    }

    #[test]
    fn hook_targets_session_allows_stop_for_same_pane_with_rotated_session_id() {
        let entry = Session {
            agent_session_id: Some("sid-a".into()),
            tmux_pane_id: "%1".into(),
            ..sample_entry("one")
        };
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: None,
            session_id: Some("sid-b".into()),
            kitty_window_id: None,
            tmux_pane_id: Some("%1".into()),
            tmux_session: Some("agents".into()),
            event: "stop".into(),
            ts: 0,
            payload: serde_json::json!({}),
            cwd: None,
            kitty_pid: None,
            kitty_listen_on: None,
            ..Default::default()
        };
        let home = std::path::Path::new("/tmp");
        assert!(hook_targets_session(&entry, "stop", &msg, "%1", home));
        assert!(!hook_targets_session(&entry, "post_tool", &msg, "%2", home));
    }

    #[test]
    fn hook_targets_session_rejects_stop_without_bound_grok_session() {
        let entry = Session {
            agent_session_id: None,
            tmux_pane_id: "%9".into(),
            ..sample_entry("console")
        };
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: None,
            session_id: Some("sid-live".into()),
            kitty_window_id: None,
            tmux_pane_id: Some("%9".into()),
            tmux_session: Some("agents".into()),
            event: "stop".into(),
            ts: 0,
            payload: serde_json::json!({}),
            cwd: None,
            kitty_pid: None,
            kitty_listen_on: None,
            ..Default::default()
        };
        let home = std::path::Path::new("/tmp");
        assert!(!hook_targets_session(&entry, "stop", &msg, "%9", home));
    }

    #[test]
    fn hook_targets_session_allows_session_start_to_replace_grok_session() {
        let entry = Session {
            agent_session_id: Some("sid-old".into()),
            tmux_pane_id: "%3".into(),
            ..sample_entry("old thread")
        };
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: None,
            session_id: Some("sid-new".into()),
            kitty_window_id: None,
            tmux_pane_id: Some("%3".into()),
            tmux_session: Some("agents".into()),
            event: "session_start".into(),
            ts: 0,
            payload: serde_json::json!({}),
            cwd: None,
            kitty_pid: None,
            kitty_listen_on: None,
            ..Default::default()
        };
        let home = std::path::Path::new("/tmp");
        assert!(hook_targets_session(&entry, "session_start", &msg, "%3", home));
    }

    #[test]
    fn hook_targets_session_rejects_subagent_session_start_for_parent_pane() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let parent = "019efbed-parent-session-id";
        let subagent = "019efbee-subagent-session";
        let encoded = crate::session::env::encode_session_cwd("/tmp/project");
        let subagent_dir = crate::paths::provider_sessions_dir(home, "grok")
            .join(&encoded)
            .join(parent)
            .join("subagents")
            .join(subagent);
        std::fs::create_dir_all(&subagent_dir).unwrap();
        std::fs::write(
            subagent_dir.join("meta.json"),
            format!(
                r#"{{"parent_session_id":"{parent}","child_session_id":"{subagent}"}}"#
            ),
        )
        .unwrap();

        let entry = Session {
            agent_session_id: Some(parent.into()),
            tmux_pane_id: "%6".into(),
            ..sample_entry("parent thread")
        };
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: Some("grok".into()),
            session_id: Some(subagent.into()),
            tmux_pane_id: Some("%6".into()),
            tmux_session: Some("agents".into()),
            event: "session_start".into(),
            ts: 0,
            payload: serde_json::json!({}),
            ..Default::default()
        };
        assert!(!hook_targets_session(&entry, "session_start", &msg, "%6", home));
        assert!(!hook_targets_session(&entry, "turn_complete", &msg, "%6", home));
    }

    #[test]
    fn dedupe_grok_sessions_keeps_active_owner() {
        // Hermetic: a temp home (no agent records on disk) plus a sid unique to
        // this test, so the global agent-id cache cannot be polluted by other
        // tests running in parallel and the ownership check is deterministic.
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sid = "sid-dedupe-active-owner";
        crate::agents::invalidate_agent_id_cache(sid);

        let mut sessions = vec![
            Session {
                id: "tmux:win:1".into(),
                tab_index: 1,
                tmux_pane_id: "%11".into(),
                tmux_session: "agents".into(),
                agent_session_id: Some(sid.into()),
                title: "grok · duplicate".into(),
                description: "duplicate".into(),
                cwd: "/tmp/a".into(),
                cwd_label: "~/tmp/a".into(),
                project: "grok".into(),
                state: AgentState::Approval,
                is_active: false,
                ..sample_entry("duplicate")
            },
            Session {
                id: "tmux:win:3".into(),
                tab_index: 3,
                tmux_pane_id: "%11".into(),
                tmux_session: "agents".into(),
                agent_session_id: Some(sid.into()),
                title: "grok · duplicate".into(),
                description: "duplicate".into(),
                cwd: "/tmp/a".into(),
                cwd_label: "~/tmp/a".into(),
                project: "grok".into(),
                state: AgentState::Idle,
                is_active: true,
                ..sample_entry("duplicate")
            },
        ];
        dedupe_agent_sessions(&config, &mut sessions);
        assert_eq!(sessions.len(), 2);
        let owner = sessions
            .iter()
            .find(|session| session.agent_session_id.as_deref() == Some(sid))
            .unwrap();
        assert_eq!(owner.tab_index, 3);
        assert_eq!(owner.id, "tmux:win:3");
        let cleared = sessions
            .iter()
            .find(|session| session.id == "tmux:win:1")
            .unwrap();
        assert_eq!(cleared.agent_session_id, None);
        assert_eq!(cleared.completed_thread, None);
        assert_eq!(cleared.state, AgentState::Idle);
    }

    #[tokio::test]
    async fn handle_notify_stop_accepts_rotated_agent_session_id_on_same_pane() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let old_sid = "019e9fe3-7bff-7511-b965-4be35d6dc36e";
        let new_sid = "019e9faa-aaaa-bbbb-cccc-ddddeeeeffff";
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path(new_sid),
            "TMUX_PANE=%49\nSESSIONS_WINDOW_INDEX=6\nTMUX_SESSION=agents\n",
        )
        .unwrap();

        let mut session = sample_entry("copy optimiser");
        session.id = "tmux:win:6".into();
        session.tab_index = 6;
        session.kitty_window_id = 6;
        session.tmux_pane_id = "%49".into();
        session.agent_session_id = Some(old_sid.into());
        session.state = AgentState::Approval;

        let state = test_daemon_state(config, vec![session]);
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: Some("grok".into()),
            session_id: Some(new_sid.into()),
            kitty_window_id: None,
            tmux_pane_id: Some("%49".into()),
            tmux_session: Some("agents".into()),
            event: "stop".into(),
            ts: 1,
            payload: serde_json::json!({}),
            cwd: None,
            kitty_pid: None,
            kitty_listen_on: None,
            ..Default::default()
        };

        assert!(state.handle_notify(&msg).await.is_some());
        let updated = state
            .session_list()
            .await
            .into_iter()
            .find(|session| session.tab_index == 6)
            .unwrap();
        assert_eq!(updated.agent_session_id.as_deref(), Some(new_sid));
        assert!(updated.thread_is_complete());
    }

    #[tokio::test]
    async fn handle_notify_turn_complete_marks_thread_from_approval() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let sid = "019e9fe3-7bff-7511-b965-4be35d6dc36e";
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path(sid),
            "TMUX_PANE=%49\nSESSIONS_WINDOW_INDEX=6\nTMUX_SESSION=agents\n",
        )
        .unwrap();

        let mut session = sample_entry("copy optimiser");
        session.id = "tmux:win:6".into();
        session.tab_index = 6;
        session.kitty_window_id = 6;
        session.tmux_pane_id = "%49".into();
        session.agent_session_id = Some(sid.into());
        session.state = AgentState::Approval;

        let state = test_daemon_state(config, vec![session]);
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: Some("grok".into()),
            session_id: Some(sid.into()),
            kitty_window_id: None,
            tmux_pane_id: Some("%49".into()),
            tmux_session: Some("agents".into()),
            event: "turn_complete".into(),
            ts: 1,
            payload: serde_json::json!({}),
            cwd: None,
            kitty_pid: None,
            kitty_listen_on: None,
            ..Default::default()
        };

        let patch = state.handle_notify(&msg).await;
        assert!(patch.is_some(), "turn_complete should produce a patch");
        let updated = state
            .session_list()
            .await
            .into_iter()
            .find(|session| session.tab_index == 6)
            .unwrap();
        assert!(updated.thread_is_complete());
        assert!(!updated.shows_run_spinner());
    }

    #[tokio::test]
    async fn handle_notify_stop_marks_thread_complete() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let sid = "019e9fe3-7bff-7511-b965-4be35d6dc36e";
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path(sid),
            "TMUX_PANE=%49\nSESSIONS_WINDOW_INDEX=6\nTMUX_SESSION=agents\n",
        )
        .unwrap();

        let mut session = sample_entry("copy optimiser");
        session.id = "tmux:win:6".into();
        session.tab_index = 6;
        session.kitty_window_id = 6;
        session.tmux_pane_id = "%49".into();
        session.agent_session_id = Some(sid.into());
        session.state = AgentState::Approval;

        let state = test_daemon_state(config, vec![session]);
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: Some("grok".into()),
            session_id: Some(sid.into()),
            kitty_window_id: None,
            tmux_pane_id: Some("%49".into()),
            tmux_session: Some("agents".into()),
            event: "stop".into(),
            ts: 1,
            payload: serde_json::json!({}),
            cwd: None,
            kitty_pid: None,
            kitty_listen_on: None,
            ..Default::default()
        };

        let patch = state.handle_notify(&msg).await;
        assert!(patch.is_some(), "stop should produce a patch");
        let updated = state
            .session_list()
            .await
            .into_iter()
            .find(|session| session.tab_index == 6)
            .unwrap();
        assert_eq!(updated.state, AgentState::Done);
        assert!(updated.thread_is_complete());
    }

    #[test]
    fn sessions_changed_detects_title_updates_only() {
        let mut previous = HashMap::new();
        let mut current = HashMap::new();
        let session = sample_entry("ship api");
        previous.insert(session.id.clone(), session.clone());
        current.insert(session.id.clone(), session);
        assert!(!sessions_changed(&previous, &current));

        let mut updated = sample_entry("ship api");
        updated.title = "codex · ship api".into();
        current.insert(updated.id.clone(), updated);
        assert!(sessions_changed(&previous, &current));
    }

    #[test]
    fn resolve_agent_session_id_prefers_fresh_when_pane_session_rotates() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path("sid-old"),
            "TMUX_PANE=%42\nSESSIONS_WINDOW_INDEX=3\nTMUX_SESSION=agents\n",
        )
        .unwrap();
        std::fs::write(
            config.session_env_path("sid-new"),
            "TMUX_PANE=%42\nSESSIONS_WINDOW_INDEX=3\nTMUX_SESSION=agents\n",
        )
        .unwrap();

        let existing = Session {
            agent_session_id: Some("sid-old".into()),
            tmux_pane_id: "%42".into(),
            tab_index: 3,
            ..sample_entry("old thread")
        };
        let fresh = Session {
            agent_session_id: Some("sid-new".into()),
            tmux_pane_id: "%42".into(),
            tab_index: 3,
            ..sample_entry("new thread")
        };

        let pane_index = HashMap::from([("%42".to_string(), 3)]);
        assert_eq!(
            resolve_agent_session_id(&config, &existing, &fresh, &pane_index).as_deref(),
            Some("sid-new")
        );
    }

    #[test]
    fn refresh_merge_prefers_polled_session_id_and_restores_title() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let session_id = "session-123";
        let state_dir = config.grok_state_dir();
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            config.session_title_path(session_id),
            "codex · fix sidebar titles\n",
        )
        .unwrap();
        write_grok_turn_started(&config, "/tmp", session_id);

        let existing = Session {
            agent_session_id: None,
            title: "console".into(),
            description: "console".into(),
            project: String::new(),
            ..sample_entry("ship api")
        };
        let mut fresh = Session {
            agent_session_id: Some(session_id.into()),
            title: "console".into(),
            description: "console".into(),
            project: String::new(),
            ..sample_entry("ship api")
        };

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.agent_session_id.as_deref(), Some(session_id));
        assert_eq!(fresh.title, "codex · fix sidebar titles");
        assert_eq!(fresh.description, "fix sidebar titles");
        assert_eq!(fresh.project, "codex");
    }

    #[test]
    fn resolve_focus_target_prefers_tab_index() {
        let config = Config::default();
        let mut sessions = HashMap::new();
        for tab_index in 1..=8 {
            sessions.insert(
                format!("tmux:win:{tab_index}"),
                Session {
                    tab_index,
                    ..sample_entry(&format!("thread-{tab_index}"))
                },
            );
        }

        let target = resolve_focus_target(
            &config,
            &sessions,
            &suppressions_for(&config, &sessions),
            3,
            Some(7),
        )
        .unwrap();
        assert_eq!(target, 7);
    }

    #[test]
    fn resolve_focus_target_ordinal_differs_from_collapsed_sidebar_order() {
        let config = Config::default();
        let mut sessions = HashMap::new();
        for tab_index in 1..=6 {
            sessions.insert(format!("tmux:win:{tab_index}"), {
                let at = Utc::now() - chrono::Duration::minutes(tab_index as i64);
                let mut session = sample_entry(&format!("thread-{tab_index}"));
                session.tab_index = tab_index;
                session.cwd_label = "~/tmp/a".into();
                session.description = format!("thread-{tab_index}");
                session.messaged_at = Some(at);
                session.last_event_at = at;
                session
            });
        }
        sessions.insert("tmux:win:15".into(), {
            let mut session = sample_entry("other-group");
            session.tab_index = 15;
            session.cwd_label = "~/tmp/b".into();
            session.description = "other-group".into();
            let at = Utc::now() - chrono::Duration::minutes(20);
            session.messaged_at = Some(at);
            session.last_event_at = at;
            session
        });

        let sorted = sorted_sessions(sessions.values(), &suppressions_for(&config, &sessions));
        assert_eq!(sorted.len(), 7);
        assert_eq!(sorted[0].tab_index, 1);
        assert_eq!(sorted[5].tab_index, 6);
        assert_eq!(sorted[6].tab_index, 15);

        // Collapsed ~/tmp/a shows MAX_THREADS_PER_GROUP rows; ordinal 7 is ~/tmp/b.
        let supp = suppressions_for(&config, &sessions);
        let ordinal_target = resolve_focus_target(&config, &sessions, &supp, 7, None).unwrap();
        let tab_target = resolve_focus_target(&config, &sessions, &supp, 7, Some(15)).unwrap();
        assert_eq!(ordinal_target, 15);
        assert_eq!(tab_target, 15);
    }

    #[test]
    fn resolve_focus_target_skips_folded_pwd_groups() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        group_order::save_folded(&config, &HashSet::from(["~/tmp/a".into()])).unwrap();

        let mut sessions = HashMap::new();
        for tab_index in 1..=3 {
            sessions.insert(
                format!("tmux:win:{tab_index}"),
                Session {
                    tab_index,
                    cwd_label: "~/tmp/a".into(),
                    description: format!("hidden-{tab_index}"),
                    ..sample_entry(&format!("hidden-{tab_index}"))
                },
            );
        }
        sessions.insert(
            "tmux:win:4".into(),
            Session {
                tab_index: 4,
                cwd_label: "~/tmp/b".into(),
                description: "visible".into(),
                ..sample_entry("visible")
            },
        );

        let target = resolve_focus_target(
            &config,
            &sessions,
            &suppressions_for(&config, &sessions),
            1,
            None,
        )
        .unwrap();
        assert_eq!(target, 4);
    }

    #[test]
    fn resolve_focus_target_uses_sidebar_group_order_not_active_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        group_order::save(
            &config,
            &group_order::SidebarGroupOrder {
                groups: vec!["~/projects/other".into(), "~/projects/active".into()],
            },
        )
        .unwrap();

        let mut sessions = HashMap::new();
        sessions.insert(
            "tmux:win:1".into(),
            Session {
                tab_index: 1,
                cwd_label: "~/projects/other".into(),
                description: "other-first".into(),
                ..sample_entry("other-first")
            },
        );
        sessions.insert(
            "tmux:win:5".into(),
            Session {
                id: "tmux:win:5".into(),
                tab_index: 5,
                cwd_label: "~/projects/active".into(),
                description: "active-dir".into(),
                is_active: true,
                ..sample_entry("active-dir")
            },
        );

        let supp = suppressions_for(&config, &sessions);
        let legacy_first = sorted_sessions(sessions.values(), &supp)[0].tab_index;
        assert_eq!(legacy_first, 5);

        let target = resolve_focus_target(&config, &sessions, &supp, 1, None).unwrap();
        assert_eq!(target, 1);
    }

    #[test]
    fn refresh_merge_upgrades_codex_binary_placeholder_from_rollout() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let session_id = "019eb0bb-3711-72d2-a80c-15259d6349e4";
        let cwd = env!("CARGO_MANIFEST_DIR");
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path(session_id),
            "TMUX_PANE=%513\nSESSIONS_WINDOW_INDEX=37\n",
        )
        .unwrap();
        let rollout_dir = config.home.join(".codex/sessions/2026/06/10");
        std::fs::create_dir_all(&rollout_dir).unwrap();
        std::fs::write(
            rollout_dir.join(format!("rollout-2026-06-10T18-21-59-{session_id}.jsonl")),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"019eb0bb-3711-72d2-a80c-15259d6349e4","cwd":"{cwd}","timestamp":"2026-06-10T18:21:59Z"}}}}
{{"type":"event_msg","payload":{{"type":"task_started","started_at":"2026-06-10T18:21:59Z"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"fix codex sidebar titles"}}}}"#
            ),
        )
        .unwrap();

        let mut existing = Session {
            agent_session_id: Some(session_id.into()),
            title: "codex-aarch64-a".into(),
            description: "codex-aarch64-a".into(),
            project: String::new(),
            cwd: cwd.into(),
            cwd_label: "~/projects/sessions-cli".into(),
            tmux_pane_id: "%513".into(),
            tab_index: 37,
            state: AgentState::Working,
            ..sample_entry("sessions-cli")
        };
        existing.id = "tmux:win:37".into();
        existing.kitty_window_id = 37;
        let mut fresh = Session {
            agent_session_id: Some(session_id.into()),
            title: "codex-aarch64-a".into(),
            description: "codex-aarch64-a".into(),
            project: String::new(),
            cwd: cwd.into(),
            cwd_label: "~/projects/sessions-cli".into(),
            tmux_pane_id: "%513".into(),
            tab_index: 37,
            state: AgentState::Working,
            ..sample_entry("sessions-cli")
        };
        fresh.id = "tmux:win:37".into();
        fresh.kitty_window_id = 37;

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.title, "codex · fix codex sidebar titles");
        assert_eq!(fresh.description, "fix codex sidebar titles");
        assert_eq!(fresh.project, "codex");
    }

    #[test]
    fn refresh_merge_keeps_codex_placeholder_until_confident_rollout_title() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let session_id = "019eb0bb-3711-72d2-a80c-15259d6349e4";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let rollout_dir = config.home.join(".codex/sessions/2026/06/10");
        std::fs::create_dir_all(&rollout_dir).unwrap();
        std::fs::write(
            rollout_dir.join(format!("rollout-2026-06-10T18-21-59-{session_id}.jsonl")),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"019eb0bb-3711-72d2-a80c-15259d6349e4","cwd":"{cwd}"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"testing"}}}}"#
            ),
        )
        .unwrap();

        let existing = Session {
            agent_session_id: Some(session_id.into()),
            title: "codex-aarch64-a".into(),
            description: "codex-aarch64-a".into(),
            project: String::new(),
            cwd: cwd.into(),
            cwd_label: "~/projects/sessions-cli".into(),
            tmux_pane_id: "%513".into(),
            tab_index: 37,
            state: AgentState::Working,
            ..sample_entry("sessions-cli")
        };
        let mut fresh = existing.clone();

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.title, "codex · ?");
        assert_eq!(fresh.description, "?");
        assert_eq!(fresh.project, "codex");
    }

    #[test]
    fn refresh_merge_clears_done_probe_codex_completion() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let session_id = "019eb0bb-3711-72d2-a80c-15259d6349e4";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let rollout_dir = config.home.join(".codex/sessions/2026/06/10");
        std::fs::create_dir_all(&rollout_dir).unwrap();
        std::fs::write(
            rollout_dir.join(format!("rollout-2026-06-10T18-21-59-{session_id}.jsonl")),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"019eb0bb-3711-72d2-a80c-15259d6349e4","cwd":"{cwd}"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"testing"}}}}"#
            ),
        )
        .unwrap();

        let existing = Session {
            agent_session_id: Some(session_id.into()),
            title: "codex · testing".into(),
            description: "testing".into(),
            project: "codex".into(),
            cwd: cwd.into(),
            cwd_label: "~/projects/sessions-cli".into(),
            tmux_pane_id: "%513".into(),
            tab_index: 37,
            state: AgentState::Done,
            completed_thread: Some("testing".into()),
            completed_at: Some(Utc::now()),
            ..sample_entry("sessions-cli")
        };
        let mut fresh = existing.clone();

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.state, AgentState::Idle);
        assert_eq!(fresh.title, "codex · ?");
        assert_eq!(fresh.description, "?");
        assert!(fresh.completed_thread.is_none());
    }

    #[test]
    fn refresh_merge_clears_stale_probe_codex_title() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let session_id = "019eb0bb-3711-72d2-a80c-15259d6349e4";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let rollout_dir = config.home.join(".codex/sessions/2026/06/10");
        std::fs::create_dir_all(&rollout_dir).unwrap();
        std::fs::write(
            rollout_dir.join(format!("rollout-2026-06-10T18-21-59-{session_id}.jsonl")),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"019eb0bb-3711-72d2-a80c-15259d6349e4","cwd":"{cwd}"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"testing"}}}}"#
            ),
        )
        .unwrap();

        let existing = Session {
            agent_session_id: Some(session_id.into()),
            title: "codex · testing".into(),
            description: "testing".into(),
            project: "codex".into(),
            cwd: cwd.into(),
            cwd_label: "~/projects/sessions-cli".into(),
            tmux_pane_id: "%513".into(),
            tab_index: 37,
            state: AgentState::Working,
            ..sample_entry("sessions-cli")
        };
        let mut fresh = existing.clone();

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.title, "codex · ?");
        assert_eq!(fresh.description, "?");
    }

    #[test]
    fn refresh_merge_groups_cross_project_grok_under_agent_cwd() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let session_id = "019ea671-54cc-7fb0-91e4-2a567b4ce022";
        let sessions_cwd = env!("CARGO_MANIFEST_DIR");
        let superflip_cwd = "/home/testuser/projects/superflip";
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path(session_id),
            "TMUX_PANE=%394\nSESSIONS_WINDOW_INDEX=10\n",
        )
        .unwrap();
        let summary_dir = grok_session_dir(&config.home, sessions_cwd, session_id);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            format!(
                r#"{{"generated_title":"Smooth Tmux Pane Compositor","info":{{"cwd":"{sessions_cwd}"}}}}"#
            ),
        )
        .unwrap();

        let mut existing = Session {
            agent_session_id: Some(session_id.into()),
            title: "grok · Smooth Tmux Pane Compositor".into(),
            description: "Smooth Tmux Pane Compositor".into(),
            project: "grok".into(),
            cwd: superflip_cwd.into(),
            cwd_label: "~/projects/superflip".into(),
            tmux_pane_id: "%394".into(),
            tab_index: 10,
            state: AgentState::Working,
            ..sample_entry("superflip")
        };
        existing.id = "tmux:win:10".into();
        existing.kitty_window_id = 10;
        let mut fresh = existing.clone();

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(
            fresh.cwd_label,
            format_tilde_path(sessions_cwd, &config.home)
        );
        assert_eq!(fresh.cwd, superflip_cwd);
    }

    #[test]
    fn refresh_merge_clears_cross_project_grok_on_idle_superflip_shell() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let session_id = "019ea6d5-31c4-7260-92a9-de3122c6b0f5";
        let sessions_cwd = env!("CARGO_MANIFEST_DIR");
        let superflip_cwd = "/home/testuser/projects/superflip";
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path(session_id),
            "TMUX_PANE=%389\nSESSIONS_WINDOW_INDEX=3\n",
        )
        .unwrap();
        std::fs::write(
            config.session_title_path(session_id),
            "grok · Bridge sessions-cli to native host architecture Mode B\n",
        )
        .unwrap();
        let summary_dir = grok_session_dir(&config.home, sessions_cwd, session_id);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            format!(
                r#"{{"generated_title":"Bridge sessions-cli to native host architecture Mode B","info":{{"cwd":"{sessions_cwd}"}}}}"#
            ),
        )
        .unwrap();

        let existing = Session {
            agent_session_id: Some(session_id.into()),
            title: "grok · Bridge sessions-cli to native host architecture Mode B".into(),
            description: "Bridge sessions-cli to native host architecture Mode B".into(),
            project: "grok".into(),
            cwd: superflip_cwd.into(),
            cwd_label: "~/projects/superflip".into(),
            ..sample_entry("superflip")
        };
        let mut fresh = Session {
            agent_session_id: None,
            title: "superflip".into(),
            description: "superflip".into(),
            project: String::new(),
            cwd: superflip_cwd.into(),
            cwd_label: "~/projects/superflip".into(),
            ..sample_entry("superflip")
        };

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.agent_session_id, None);
        assert!(!fresh.title.contains("sessions-cli"));
        assert!(!fresh.description.contains("sessions-cli"));
        assert_eq!(
            fresh.cwd_label,
            format_tilde_path(superflip_cwd, &config.home)
        );
        assert!(fresh.project.is_empty());
    }

    #[test]
    fn refresh_merge_shell_pane_clears_stale_grok_session() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let session_id = "session-456";
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_title_path(session_id),
            "grok · stale agent thread\n",
        )
        .unwrap();

        let existing = Session {
            agent_session_id: Some(session_id.into()),
            title: "grok · stale agent thread".into(),
            description: "stale agent thread".into(),
            project: "grok".into(),
            ..sample_entry("ship api")
        };
        let mut fresh = Session {
            agent_session_id: None,
            title: "console".into(),
            description: "console".into(),
            project: String::new(),
            ..sample_entry("ship api")
        };

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.agent_session_id, None);
        assert_eq!(fresh.title, "console");
        assert_eq!(fresh.description, "console");
        assert!(fresh.project.is_empty());
    }

    #[test]
    fn refresh_merge_keeps_fresh_polled_title_for_shell_pane() {
        let config = Config::default();
        let existing = Session {
            agent_session_id: None,
            title: "grok · main workspace".into(),
            description: "main workspace".into(),
            project: "grok".into(),
            ..sample_entry("ship api")
        };
        let mut fresh = Session {
            agent_session_id: None,
            title: "console".into(),
            description: "console".into(),
            project: String::new(),
            ..sample_entry("ship api")
        };

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(fresh.title, "console");
        assert_eq!(fresh.description, "console");
        assert!(fresh.project.is_empty());
    }

    #[test]
    fn refresh_merge_syncs_disk_title_over_stale_sidebar_identity() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();

        let session_id = "019e9fcb-965f-7a12-89e2-1d3aab6ee273";
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_title_path(session_id),
            "grok · Add Hover Highlight for Draggable PWD/Folder Sections and Replace Icon\n",
        )
        .unwrap();
        write_grok_turn_started(&config, "/tmp", session_id);

        let existing = Session {
            agent_session_id: Some(session_id.into()),
            title: "grok · grok resume".into(),
            description: "grok resume".into(),
            project: "grok".into(),
            ..sample_entry("grok resume")
        };
        let mut fresh = Session {
            agent_session_id: Some(session_id.into()),
            title: "grok · grok resume".into(),
            description: "grok resume".into(),
            project: "grok".into(),
            ..sample_entry("grok resume")
        };

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(
            fresh.title,
            "grok · Add Hover Highlight for Draggable PWD/Folder Sections and Replace Icon"
        );
        assert_eq!(
            fresh.description,
            "Add Hover Highlight for Draggable PWD/Folder Sections and Replace Icon"
        );
    }

    #[test]
    fn refresh_merge_drops_stale_bootstrap_command_label() {
        let config = Config::default();
        let existing = Session {
            agent_session_id: Some("session-789".into()),
            title: "superflip · ./run-local.sh".into(),
            description: "./run-local.sh".into(),
            project: "superflip".into(),
            ..sample_entry("./run-local.sh")
        };
        let mut fresh = Session {
            agent_session_id: Some("session-789".into()),
            title: "grok · Copy Paste Tmux Terminals to System Clipboard Naturally".into(),
            description: "Copy Paste Tmux Terminals to System Clipboard Naturally".into(),
            project: "grok".into(),
            ..sample_entry("Copy Paste Tmux Terminals to System Clipboard Naturally")
        };

        merge_session_refresh_state(
            &config,
            &existing,
            &mut fresh,
            &HashMap::new(),
            &WorkspaceCatalog::default(),
        );

        assert_eq!(
            fresh.title,
            "grok · Copy Paste Tmux Terminals to System Clipboard Naturally"
        );
        assert_eq!(
            fresh.description,
            "Copy Paste Tmux Terminals to System Clipboard Naturally"
        );
    }

    #[test]
    fn preserve_hook_cwd_over_stale_home_poll() {
        let mut config = Config::default();
        config.home = PathBuf::from("/home/testuser");
        let existing = Session {
            cwd: env!("CARGO_MANIFEST_DIR").into(),
            cwd_label: "~/projects/sessions-cli".into(),
            ..sample_entry("ship api")
        };
        let mut fresh = Session {
            cwd: "/home/testuser".into(),
            cwd_label: "~".into(),
            ..sample_entry("ship api")
        };
        preserve_hook_cwd_over_stale_poll(&config, &existing, &mut fresh);
        assert_eq!(fresh.cwd, env!("CARGO_MANIFEST_DIR"));
        assert_eq!(fresh.cwd_label, "~/projects/sessions-cli");
    }

    #[test]
    fn cwd_label_for_path_does_not_map_empty_to_tilde() {
        let home = PathBuf::from("/home/testuser");
        assert_eq!(cwd_label_for_path("", &home), "");
        assert_eq!(cwd_label_for_path("/home/testuser", &home), "~");
    }

    #[test]
    fn resolve_window_index_accepts_env_window_zero() {
        use crate::model::NotifyMessage;

        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        std::fs::create_dir_all(config.grok_state_dir()).unwrap();
        std::fs::write(
            config.session_env_path("sid-a"),
            "SESSIONS_WINDOW_INDEX=0\nTMUX_SESSION=agents\n",
        )
        .unwrap();
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: None,
            session_id: Some("sid-a".into()),
            kitty_window_id: None,
            tmux_pane_id: None,
            tmux_session: Some("agents".into()),
            event: "turn_complete".into(),
            ts: 1,
            payload: serde_json::json!({}),
            cwd: None,
            kitty_pid: None,
            kitty_listen_on: None,
            ..Default::default()
        };
        assert_eq!(resolve_window_index(&msg, &config), Some(0));
    }

    #[test]
    fn resolve_window_index_accepts_managed_sessions_session_id() {
        use crate::model::NotifyMessage;
        use crate::session::ManagedLaunchRecord;

        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let record = ManagedLaunchRecord {
            sessions_session_id: "ssn_new_chat".into(),
            launch_id: "lch_test".into(),
            agent: "grok".into(),
            tmux_session: "agents".into(),
            window_index: 12,
            pane_id: Some("%712".into()),
            initial_cwd: env!("CARGO_MANIFEST_DIR").into(),
            created_at: Utc::now().to_rfc3339(),
            agent_session_id: None,
        };
        crate::session::save_managed_record(&config.home, &record).unwrap();
        let msg = NotifyMessage {
            t: "grok".into(),
            agent: Some("grok".into()),
            session_id: Some("019ebb73-88d4-7083-9cd8-74c948855d84".into()),
            kitty_window_id: None,
            tmux_pane_id: None,
            tmux_session: Some("agents".into()),
            event: "session_start".into(),
            ts: 1,
            payload: serde_json::json!({}),
            cwd: Some(env!("CARGO_MANIFEST_DIR").into()),
            kitty_pid: None,
            kitty_listen_on: None,
            sessions_session_id: Some("ssn_new_chat".into()),
            ..Default::default()
        };
        assert_eq!(resolve_window_index(&msg, &config), Some(12));
    }

    #[test]
    fn closed_session_marker_matches_agent_session_id() {
        let marker = ClosedSessionMarker {
            agent_session_id: Some("sid-a".into()),
            tmux_pane_id: "%1".into(),
        };
        let session = Session {
            agent_session_id: Some("sid-a".into()),
            tmux_pane_id: "%9".into(),
            ..sample_entry("task")
        };
        assert!(marker.matches(&session));
        assert!(marker.matches_notify(Some("sid-a"), "%9"));
        assert!(!marker.matches_notify(Some("sid-b"), "%9"));
    }

    #[test]
    fn closed_session_marker_matches_pane_id_without_agent() {
        let marker = ClosedSessionMarker {
            agent_session_id: None,
            tmux_pane_id: "%5".into(),
        };
        let session = Session {
            agent_session_id: None,
            tmux_pane_id: "%5".into(),
            ..sample_entry("shell")
        };
        assert!(marker.matches(&session));
        assert!(marker.matches_notify(None, "%5"));
        assert!(!marker.matches_notify(None, "%6"));
    }

    fn config_without_agents_session() -> (TempDir, Config) {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        config.tmux_session = "sessions-test-missing-agents".into();
        (dir, config)
    }

    #[tokio::test]
    async fn set_booting_resets_after_restore_complete() {
        let (_dir, config) = config_without_agents_session();
        let state = test_daemon_state(config, vec![sample_entry("restore cycle")]);
        assert!(state.is_booting().await, "daemon starts booting until reconcile");

        state.restore_complete().await;
        assert!(!state.is_booting().await);

        state.set_booting(true).await;
        assert!(state.is_booting().await);
    }

    #[tokio::test]
    async fn refresh_preserves_sessions_while_booting_when_agents_session_missing() {
        let (_dir, config) = config_without_agents_session();
        let state = test_daemon_state(config, vec![sample_entry("keep me")]);
        state.restore_complete().await;
        assert!(!state.is_booting().await);

        state.set_booting(true).await;
        let _ = state.refresh_from_tmux().await;

        assert_eq!(state.session_count().await, 1);
        assert!(state.is_booting().await);
    }

    #[tokio::test]
    async fn refresh_reenters_booting_when_agents_session_missing() {
        let (_dir, config) = config_without_agents_session();
        let state = test_daemon_state(config, vec![sample_entry("restore pending")]);
        state.restore_complete().await;
        assert!(!state.is_booting().await);

        let _ = state.refresh_from_tmux().await;

        assert!(state.is_booting().await);
        assert_eq!(state.session_count().await, 1);
    }
