//! Integration tests migrated from the deleted `grok/` facade (WS-14).

#[cfg(test)]
mod facade_migration_tests {

    use crate::model::AgentState;
    use crate::pty::{
        build_description, classify_pane, default_thread_name, derive_project,
        effective_workspace_command, ensure_app_registry, format_session_title,
        infer_pane_state, is_confident_thread_title, is_machine_derived_thread,
        is_sticky_thread_title, is_weak_thread_name, merge_lifecycle_state,
        parse_description, poll_foreground_app, resolve_agent_app, resolve_session_names,
        session_names_from_prompt, shorten_command, shorten_prompt, CONSOLE_LABEL,
        DEFAULT_AGENT_APP, PaneKind,
    };
    use crate::session::{
        load_session_env, session_id_for_pane, WorkspaceCatalog, WorkspaceEntry, WorkspaceRef,
    };
    use crate::agents::grok::session_dir as grok_session_dir;
    use std::path::{Path, PathBuf};

    fn home() -> PathBuf {
        PathBuf::from("/home/testuser")
    }

    fn repo_cwd() -> String {
        env!("CARGO_MANIFEST_DIR").to_string()
    }

    fn seed_session_summary(
        home: &Path,
        cwd: &str,
        session_id: &str,
        summary_json: &str,
        events_jsonl: &str,
    ) {
        let dir = grok_session_dir(home, cwd, session_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("summary.json"), summary_json).unwrap();
        std::fs::write(dir.join("events.jsonl"), events_jsonl).unwrap();
    }

    #[test]
    fn resolve_session_names_keeps_cursor_app_and_prior_thread() {
        let (title, thread, app) = resolve_session_names(
            &home(),
            env!("CARGO_MANIFEST_DIR"),
            None,
            None,
            "cursor · ship api",
            "user_query",
            "user_query",
            None,
            true,
        );
        assert_eq!(app, "cursor");
        assert_eq!(thread, "ship api");
        assert_eq!(title, "cursor · ship api");
    }

    #[test]
    fn generic_prompt_falls_back_to_summary() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let session_id = "019e9c9f-aabc-7e11-9b66-5470f5c7d47e";
        let cwd = repo_cwd();
        seed_session_summary(
            home,
            &cwd,
            session_id,
            r#"{
  "session_summary": "Session Colors Dropped and Sidebar Names Broken When Running",
  "generated_title": "Session Colors Dropped and Sidebar Names Broken When Running",
  "agent_name": "cursor"
}"#,
            r#"{"ts":"2026-06-06T11:09:30.297Z","type":"turn_started"}"#,
        );
        let (title, thread, app) = resolve_session_names(
            home,
            &cwd,
            None,
            Some(session_id),
            "cursor · user_query",
            "user_query",
            "user_query",
            None,
            true,
        );
        assert_eq!(app, "grok");
        assert_eq!(
            thread,
            "Session Colors Dropped and Sidebar Names Broken When Running"
        );
        assert_eq!(
            title,
            "grok · Session Colors Dropped and Sidebar Names Broken When Running"
        );
    }

    #[test]
    fn is_machine_derived_thread_detects_binary_placeholders() {
        assert!(is_machine_derived_thread("grok-0239-mac"));
        assert!(is_machine_derived_thread("grok-0.2.39-mac"));
        assert!(is_machine_derived_thread("codex-aarch64-a"));
        assert!(!is_machine_derived_thread("Fix Cmd+Num Shortcuts"));
        assert!(!is_machine_derived_thread("live thread"));
    }

    #[test]
    fn is_weak_thread_name_rejects_paths_and_placeholders() {
        assert!(is_weak_thread_name("user_query"));
        assert!(is_weak_thread_name("~/projects/sessions-cli"));
        assert!(is_weak_thread_name("./run-local.sh"));
        assert!(is_weak_thread_name("./run-local-dev.sh"));
        assert!(!is_weak_thread_name("fix sidebar titles"));
    }

    #[test]
    fn is_confident_thread_title_rejects_probes_and_short_placeholders() {
        assert!(!is_confident_thread_title("testing"));
        assert!(!is_confident_thread_title("codex-aarch64-a"));
        assert!(!is_confident_thread_title("?"));
        assert!(is_confident_thread_title("fix codex sidebar titles"));
        assert!(is_confident_thread_title("refactor-auth"));
    }

    #[test]
    fn is_sticky_thread_title_accepts_prompt_labels_before_confidence_threshold() {
        assert!(is_sticky_thread_title("fix sidebar"));
        assert!(!is_sticky_thread_title("testing"));
        assert!(!is_sticky_thread_title("grok"));
        assert!(!is_confident_thread_title("refactor"));
        assert!(is_sticky_thread_title("refactor"));
    }

    #[test]
    fn shorten_prompt_strips_slash_commands() {
        let s = shorten_prompt("/implement fix the sidebar navigation bug please");
        assert!(!s.starts_with("/implement"));
        assert!(s.len() <= 42);
    }

    #[test]
    fn shorten_prompt_truncates_long() {
        let s = shorten_prompt("one two three four five six seven eight nine ten");
        assert!(s.len() <= 42);
    }

    #[test]
    fn derive_project_from_cwd() {
        assert_eq!(
            derive_project(
                "/home/testuser/projects/superflip/superflip-frontend",
                &home()
            ),
            "superflip"
        );
        assert_eq!(
            derive_project("/home/testuser/projects/aeo-copy-optimiser", &home()),
            "aeo-copy-optimiser"
        );
    }






    #[test]
    fn load_session_env_parses_pane_and_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.env");
        std::fs::write(
            &path,
            "TMUX_PANE=%25\nSESSIONS_WINDOW_INDEX=13\nTMUX_SESSION=agents\n",
        )
        .unwrap();
        let env = load_session_env(&path);
        assert_eq!(env.tmux_pane_id.as_deref(), Some("%25"));
        assert_eq!(env.window_index, Some(13));
        assert_eq!(env.tmux_session.as_deref(), Some("agents"));
    }

    #[test]
    fn build_description_uses_grok_app() {
        let desc = build_description("fix sidebar", "/home/testuser/projects/superflip", &home());
        assert_eq!(desc, "grok · fix sidebar");
    }

    #[test]
    fn format_session_title_joins_app_and_thread() {
        assert_eq!(
            format_session_title("grok", "Smooth terminal transitions"),
            "grok · Smooth terminal transitions"
        );
    }

    #[test]
    fn session_names_from_prompt_uses_grok_app() {
        let names =
            session_names_from_prompt("Smooth terminal transitions", "superflip · frontend dev")
                .unwrap();
        assert_eq!(names.0, "grok · Smooth terminal transitions");
        assert_eq!(names.1, "Smooth terminal transitions");
    }

    #[test]
    fn resolve_agent_app_ignores_project_prefix() {
        assert_eq!(resolve_agent_app("superflip · frontend dev"), "grok");
        assert_eq!(resolve_agent_app("grok · existing thread"), "grok");
    }

    #[test]
    fn resolve_session_names_prefers_summary() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let session_id = "019e9c5d-11ab-7111-8c9a-6d728fd445d5";
        let cwd = repo_cwd();
        seed_session_summary(
            home,
            &cwd,
            session_id,
            r#"{
  "session_summary": "Session Naming: App + Thread and Last Used Time",
  "generated_title": "Session Naming: App + Thread and Last Used Time",
  "agent_name": "cursor"
}"#,
            r#"{"ts":"2026-06-06T09:56:45.739Z","type":"turn_started"}"#,
        );
        let (title, thread, app) =
            resolve_session_names(home, &cwd, None, Some(session_id), "", "", "", None, false);
        assert_eq!(app, "grok");
        assert_eq!(thread, "Session Naming: App + Thread and Last Used Time");
        assert_eq!(
            title,
            "grok · Session Naming: App + Thread and Last Used Time"
        );
    }

    #[test]
    fn effective_workspace_command_prefers_agent_bootstrap_at_shell_prompt() {
        assert_eq!(effective_workspace_command("grok", "zsh"), "grok");
        assert_eq!(
            effective_workspace_command("grok --resume abc", "zsh"),
            "grok --resume abc"
        );
        assert_eq!(
            effective_workspace_command("grok", "/usr/bin/grok"),
            "/usr/bin/grok"
        );
        assert_eq!(effective_workspace_command("/bin/zsh -l", "zsh"), "zsh");
        assert_eq!(
            effective_workspace_command("./run-local-dev.sh", "zsh"),
            "zsh"
        );
    }

    #[test]
    fn idle_grok_workspace_is_not_console() {
        let workspace = WorkspaceRef {
            title: "superflip · main workspace",
            command: "grok",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            "/home/testuser/projects/superflip",
            None,
            None,
            "",
            "",
            "",
            Some(workspace),
            false,
        );
        assert_eq!(app, "grok");
        assert_eq!(thread, "main workspace");
        assert_eq!(title, "grok · main workspace");
        assert_ne!(thread, CONSOLE_LABEL);
    }

    #[test]
    fn shell_workspace_uses_cwd_leaf_at_project_path() {
        let workspace = WorkspaceRef {
            title: "superflip · backend",
            command: "/bin/zsh -l",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            "/home/testuser/projects/superflip/superflip-backend",
            None,
            None,
            "",
            "",
            "",
            Some(workspace),
            false,
        );
        assert!(app.is_empty());
        assert_eq!(thread, "superflip-backend");
        assert_eq!(title, "superflip-backend");
    }

    #[test]
    fn command_workspace_shows_command_descriptor() {
        let workspace = WorkspaceRef {
            title: "superflip · frontend dev",
            command: "./run-local-dev.sh",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            "/home/testuser/projects/superflip/superflip-frontend",
            None,
            None,
            "",
            "",
            "",
            Some(workspace),
            false,
        );
        assert_eq!(app, "superflip");
        assert_eq!(thread, "./run-local-dev.sh");
        assert_eq!(title, "superflip · ./run-local-dev.sh");
    }

    #[test]
    fn agent_workspace_without_session_uses_workspace_thread() {
        let workspace = WorkspaceRef {
            title: "superflip · main workspace",
            command: "grok",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            "/home/testuser/projects/superflip",
            None,
            None,
            "",
            "",
            "",
            Some(workspace),
            false,
        );
        assert_eq!(app, "grok");
        assert_eq!(thread, "main workspace");
        assert_eq!(title, "grok · main workspace");
    }

    #[test]
    fn agent_workspace_without_thread_falls_back_to_question_mark() {
        let workspace = WorkspaceRef {
            title: "grok",
            command: "codex",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            env!("CARGO_MANIFEST_DIR"),
            None,
            Some("codex"),
            "",
            "",
            "",
            Some(workspace),
            false,
        );
        assert_eq!(app, "codex");
        assert_eq!(thread, "?");
        assert_eq!(title, "codex · ?");
    }

    #[test]
    fn resolve_session_names_uses_runtime_agent_without_workspace() {
        let (title, thread, app) = resolve_session_names(
            &home(),
            env!("CARGO_MANIFEST_DIR"),
            Some("grok"),
            None,
            "",
            "",
            "",
            None,
            false,
        );
        assert_eq!(app, "grok");
        assert_eq!(thread, "?");
        assert_eq!(title, "grok · ?");
        assert_ne!(title, CONSOLE_LABEL);
    }

    #[test]
    fn runtime_agent_overrides_stale_workspace_agent() {
        let workspace = WorkspaceRef {
            title: "grok · main workspace",
            command: "/bin/zsh -l",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            env!("CARGO_MANIFEST_DIR"),
            Some("codex"),
            Some("sid-1"),
            "",
            "",
            "",
            Some(workspace),
            false,
        );
        assert_eq!(app, "codex");
        assert_eq!(thread, "main workspace");
        assert_eq!(title, "codex · main workspace");
    }

    #[test]
    fn default_thread_name_home_is_console() {
        let home = home();
        assert_eq!(
            default_thread_name(home.to_string_lossy().as_ref(), &home),
            CONSOLE_LABEL
        );
    }

    #[test]
    fn shell_ignores_stale_agent_window_name_without_live_context() {
        let workspace = WorkspaceRef {
            title: "grok · main workspace",
            command: "/bin/zsh -l",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            env!("CARGO_MANIFEST_DIR"),
            None,
            None,
            "grok · main workspace",
            "",
            "",
            Some(workspace),
            false,
        );
        assert!(app.is_empty());
        assert_eq!(thread, "sessions-cli");
        assert_eq!(title, "sessions-cli");
    }

    #[test]
    fn shell_command_beats_stale_workspace_thread_name() {
        let workspace = WorkspaceRef {
            title: "superflip · frontend dev",
            command: "zsh",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            "/home/testuser/projects/superflip/superflip-frontend",
            None,
            None,
            "superflip · frontend dev",
            "",
            "",
            Some(workspace),
            false,
        );
        assert!(app.is_empty());
        assert_eq!(thread, "superflip-frontend");
        assert_eq!(title, "superflip-frontend");
    }

    #[test]
    fn session_id_for_pane_requires_matching_window() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path();
        let state_dir = crate::paths::state_dir(home);
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("sid-a.env"),
            "TMUX_PANE=%11\nSESSIONS_WINDOW_INDEX=1\nTMUX_SESSION=agents\n",
        )
        .unwrap();
        std::fs::write(
            state_dir.join("sid-b.env"),
            "TMUX_PANE=%11\nSESSIONS_WINDOW_INDEX=3\nTMUX_SESSION=agents\n",
        )
        .unwrap();

        assert_eq!(
            session_id_for_pane(home, "%11", 1, "agents").as_deref(),
            Some("sid-a")
        );
        assert_eq!(
            session_id_for_pane(home, "%11", 3, "agents").as_deref(),
            Some("sid-b")
        );
        assert_eq!(session_id_for_pane(home, "%11", 2, "agents"), None);
    }

    #[test]
    fn shorten_command_keeps_flags() {
        assert_eq!(
            shorten_command("./run_server --port 2302 --reload"),
            "./run_server --port 2302 --reload"
        );
    }

    #[test]
    fn infer_pane_state_tracks_process_lifecycle() {
        assert_eq!(infer_pane_state("zsh", false, None), AgentState::Idle);
        assert_eq!(infer_pane_state("claude", false, None), AgentState::Working);
        assert_eq!(infer_pane_state("claude", true, Some(0)), AgentState::Done);
        assert_eq!(infer_pane_state("claude", true, Some(1)), AgentState::Error);
    }

    #[test]
    fn merge_lifecycle_state_respects_hook_priority() {
        assert_eq!(
            merge_lifecycle_state(AgentState::Working, AgentState::Done),
            AgentState::Working
        );
        assert_eq!(
            merge_lifecycle_state(AgentState::Idle, AgentState::Done),
            AgentState::Done
        );
        assert_eq!(
            merge_lifecycle_state(AgentState::Idle, AgentState::Working),
            AgentState::Working
        );
        assert_eq!(
            merge_lifecycle_state(AgentState::Done, AgentState::Idle),
            AgentState::Done
        );
        assert_eq!(
            merge_lifecycle_state(AgentState::Done, AgentState::Working),
            AgentState::Done
        );
    }

    #[test]
    fn poll_foreground_app_includes_non_agent_binaries() {
        assert_eq!(poll_foreground_app(false, Some("htop"), None), Some("htop"));
        assert_eq!(poll_foreground_app(false, Some("grok"), None), Some("grok"));
        assert_eq!(poll_foreground_app(true, Some("htop"), Some("grok")), None);
        assert_eq!(
            poll_foreground_app(false, None, Some("codex")),
            Some("codex")
        );
    }

    #[test]
    fn resolve_session_names_prefers_non_agent_foreground_over_stale_agent_thread() {
        let (title, thread, app) = resolve_session_names(
            &home(),
            env!("CARGO_MANIFEST_DIR"),
            Some("htop"),
            Some("019eb049-9fdc-77d2-bd4a-faac4cfbb0c1"),
            "grok · stale task",
            "stale task",
            "",
            None,
            false,
        );
        assert_eq!(title, "htop");
        assert_eq!(thread, "htop");
        assert_ne!(app, DEFAULT_AGENT_APP);

        let (title, thread, _) = resolve_session_names(
            &home(),
            env!("CARGO_MANIFEST_DIR"),
            Some("cargo"),
            None,
            "",
            "cargo run --release",
            "",
            None,
            false,
        );
        assert_eq!(title, "cargo · cargo run --release");
        assert_eq!(thread, "cargo run --release");
    }

    #[test]
    fn resolve_session_names_labels_tools_without_grok_prefix() {
        for tool in ["opencode", "htop"] {
            let (title, thread, app) = resolve_session_names(
                &home(),
                env!("CARGO_MANIFEST_DIR"),
                Some(tool),
                None,
                "",
                tool,
                tool,
                None,
                false,
            );
            assert_eq!(thread, tool, "thread for {tool}");
            assert_eq!(title, tool, "title for {tool}");
            assert_ne!(app, DEFAULT_AGENT_APP, "app for {tool}");
        }
    }

    #[test]
    fn resolve_session_names_shell_workspace_ignores_grok_bootstrap() {
        let workspace = WorkspaceRef {
            title: "superflip · main workspace",
            command: "zsh",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            "/home/testuser/projects/superflip",
            None,
            None,
            "",
            "",
            "",
            Some(workspace),
            false,
        );
        assert!(app.is_empty());
        assert_eq!(thread, "superflip");
        assert_eq!(title, "superflip");
        assert!(!title.starts_with("grok"));
    }

    #[test]
    fn resolve_session_names_shell_uses_cwd_leaf_not_workspace_title() {
        let workspace = WorkspaceRef {
            title: "superflip · frontend dev",
            command: "zsh",
        };
        let (title, thread, app) = resolve_session_names(
            &home(),
            "/home/testuser/projects/superflip/superflip-frontend",
            None,
            None,
            "superflip · frontend dev",
            "",
            "",
            Some(workspace),
            false,
        );
        assert!(app.is_empty());
        assert_eq!(thread, "superflip-frontend");
        assert_eq!(title, "superflip-frontend");
        assert_ne!(thread, "frontend dev");
    }

    #[test]
    fn workspace_ref_requires_matching_window_index_not_just_cwd() {
        let catalog = WorkspaceCatalog {
            entries: vec![WorkspaceEntry {
                title: "superflip · frontend dev".into(),
                cwd: "/home/testuser/projects/superflip/superflip-frontend".into(),
                command: "./run-local-dev.sh".into(),
            }],
        };
        assert!(catalog
            .workspace_ref_for_window(0, "/home/testuser/projects/superflip/superflip-frontend")
            .is_some());
        assert!(catalog
            .workspace_ref_for_window(19, "/home/testuser/projects/superflip/superflip-frontend")
            .is_none());
    }

    #[test]
    fn classify_pane_extracts_agent_prompt() {
        ensure_app_registry();
        let kind = classify_pane(
            "claude",
            "claude refactor-auth-module",
            env!("CARGO_MANIFEST_DIR"),
        );
        match kind {
            PaneKind::Tool { app, thread, .. } => {
                assert_eq!(app, "claude");
                assert_eq!(thread, "refactor-auth-module");
            }
            _ => panic!("expected tool pane"),
        }
    }

    #[test]
    fn workspace_catalog_extracts_thread_only() {
        let path = PathBuf::from("/home/testuser/.config/sessions/workspaces.toml");
        if !path.exists() {
            return;
        }
        let catalog = WorkspaceCatalog::load(&path);
        assert_eq!(
            catalog.thread_for_window_index(6).as_deref(),
            Some("copy optimiser")
        );
    }

    #[test]
    fn workspace_ref_matches_index_and_cwd_together() {
        let catalog = WorkspaceCatalog {
            entries: vec![
                WorkspaceEntry {
                    title: "superflip · dashboard agent".into(),
                    cwd: "/home/testuser/projects/superflip/superflip-dashboard".into(),
                    command: "grok".into(),
                },
                WorkspaceEntry {
                    title: "aeo · copy optimiser".into(),
                    cwd: "/home/testuser/projects/aeo-copy-optimiser".into(),
                    command: "grok".into(),
                },
            ],
        };
        let workspace = catalog
            .workspace_ref_for_window(2, "/home/testuser/projects/aeo-copy-optimiser")
            .unwrap();
        assert_eq!(workspace.title, "aeo · copy optimiser");
        assert!(catalog
            .workspace_ref_for_window(1, "/home/testuser/projects/aeo-copy-optimiser")
            .is_none());
    }

    #[test]
    fn workspace_ref_ignores_index_when_cwd_does_not_match() {
        let catalog = WorkspaceCatalog {
            entries: vec![WorkspaceEntry {
                title: "superflip · dashboard local".into(),
                cwd: "/home/testuser/projects/superflip/superflip-dashboard".into(),
                command: "./run-local.sh".into(),
            }],
        };
        assert!(catalog
            .workspace_ref_for_window(1, env!("CARGO_MANIFEST_DIR"))
            .is_none());
    }

}
