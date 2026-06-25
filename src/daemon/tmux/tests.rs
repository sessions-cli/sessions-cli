use super::*;
use crate::pty::{
    agent_from_command, bootstrap_command_from_pane_start, effective_workspace_command,
    is_shell_command, resolve_session_names, CONSOLE_LABEL, DEFAULT_AGENT_APP,
};
use crate::session::WorkspaceCatalog;
use std::path::{Path, PathBuf};

    #[test]
    fn instant_key_creates_managed() {
        let bin = "'/usr/local/bin/sessions'";
        let grok_script = instant_key_bind_script(bin, "grok");
        assert!(grok_script.contains("create-instant grok"));
        assert!(!grok_script.contains("@sessions.id"));
        assert!(grok_script.contains("refresh"));

        let console_script = instant_key_bind_script(bin, "console");
        assert!(console_script.contains("create-instant console"));

        let spec = crate::session::launch_spec_for_agent(
            "/tmp/work".into(),
            "grok",
            None,
            crate::session::ManifestSource::InstantKey,
            true,
        );
        assert_eq!(spec.source, crate::session::ManifestSource::InstantKey);
        assert!(spec.sessions_session_id.starts_with("ssn_"));
    }

    #[test]
    fn literal_send_chunks_respect_tmux_limit_and_utf8() {
        let ascii = "a".repeat(2_500);
        let parts: Vec<_> = literal_send_chunks(&ascii).map(str::to_string).collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|part| part.len() <= TMUX_SEND_KEYS_MAX_LITERAL));
        assert_eq!(parts.concat(), ascii);

        let emoji = "😀".repeat(600);
        let parts: Vec<_> = literal_send_chunks(&emoji).map(str::to_string).collect();
        assert!(parts.iter().all(|part| part.len() <= TMUX_SEND_KEYS_MAX_LITERAL));
        assert_eq!(parts.concat(), emoji);
    }

    #[test]
    fn idle_shell_poll_ignores_stale_tmux_window_name() {
        let home = PathBuf::from("/home/testuser");
        let cwd = "/home/testuser/projects/superflip";
        let workspace =
            WorkspaceCatalog::workspace_ref_with_command("superflip · main workspace", "zsh");
        let stale_window_name = "grok · Bridge sessions-cli to native host architecture Mode B";
        let with_stale = resolve_session_names(
            &home,
            cwd,
            None,
            None,
            stale_window_name,
            "",
            "",
            Some(workspace),
            false,
        );
        let (title, description, project) =
            resolve_session_names(&home, cwd, None, None, "", "", "", Some(workspace), false);
        assert!(!with_stale.0.contains("sessions-cli"));
        assert!(!title.contains("sessions-cli"));
        assert_eq!(title, "superflip");
        assert_eq!(description, "superflip");
        assert!(project.is_empty());
    }

    #[test]
    fn poll_names_manual_grok_without_workspace_catalog() {
        let home = PathBuf::from("/home/testuser");
        let win = TmuxWindow {
            index: 9,
            name: "session".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
            current_command: "grok".into(),
            start_command: "grok".into(),
            pane_id: "%19".into(),
            pane_pid: 0,
            active: true,
            pane_dead: false,
            pane_dead_status: None,
            sessions_session_id: None,
        };
        let bootstrap_command = "";
        let effective_command =
            effective_workspace_command(bootstrap_command, &win.current_command);
        let runtime_agent = agent_from_command(effective_command);
        let workspace = runtime_agent.as_ref().map(|_| {
            WorkspaceCatalog::workspace_ref_with_command(DEFAULT_AGENT_APP, effective_command)
        });
        let (title, description, project) = resolve_session_names(
            &home,
            &win.cwd,
            runtime_agent.as_deref(),
            None,
            &win.name,
            "",
            "",
            workspace,
            false,
        );
        assert_eq!(project, "grok");
        assert_eq!(description, "?");
        assert_eq!(title, "grok · ?");
    }

    #[test]
    fn parse_window_line_works() {
        let win = parse_window_line(
            "3\taeo · copy\t/home/testuser/projects/aeo\tzsh\t/bin/zsh -l\t%12\t4242\t1\t0\t0\tssn_live",
        )
        .unwrap();
        assert_eq!(win.index, 3);
        assert_eq!(win.current_command, "zsh");
        assert_eq!(win.start_command, "/bin/zsh -l");
        assert_eq!(win.pane_id, "%12");
        assert_eq!(win.pane_pid, 4242);
        assert!(win.active);
        assert_eq!(win.sessions_session_id.as_deref(), Some("ssn_live"));
    }

    #[test]
    fn poll_names_managed_launch_uses_window_title_during_shell_bootstrap() {
        let home = PathBuf::from("/home/testuser");
        let cwd = env!("CARGO_MANIFEST_DIR");
        let wrapped = crate::session::wrap_managed_launch_command(
            "grok",
            cwd,
            "ssn_test",
            "grok --model grok-composer-2.5-fast",
        );
        let start_command = format!(r#"/bin/zsh -lc "{wrapped} || exec /bin/zsh -l""#);
        let win = TmuxWindow {
            index: 31,
            name: "grok · ?".into(),
            cwd: cwd.into(),
            current_command: "zsh".into(),
            start_command,
            pane_id: "%31".into(),
            pane_pid: 0,
            active: false,
            pane_dead: false,
            pane_dead_status: None,
            sessions_session_id: None,
        };
        let start_bootstrap = bootstrap_command_from_pane_start(&win.start_command);
        let bootstrap_command = start_bootstrap.as_deref().unwrap_or("");
        let effective_command =
            effective_workspace_command(bootstrap_command, &win.current_command);
        let mut runtime_agent = agent_from_command(effective_command);
        if runtime_agent.is_none() {
            runtime_agent = Some("grok".into());
        }
        let at_shell_prompt = is_shell_command(effective_command);
        let workspace = runtime_agent.as_ref().map(|agent| {
            WorkspaceCatalog::workspace_ref_with_command(agent, effective_command)
        });
        let naming_foreground = runtime_agent.as_deref();
        let poll_title_source = win.name.as_str();
        let (title, description, project) = resolve_session_names(
            &home,
            &win.cwd,
            naming_foreground,
            None,
            poll_title_source,
            "",
            "",
            workspace,
            false,
        );
        assert!(at_shell_prompt);
        assert_eq!(project, "grok");
        assert_eq!(description, "?");
        assert_eq!(title, "grok · ?");
    }

    #[test]
    fn poll_names_managed_console_uses_window_title_during_shell_bootstrap() {
        let home = PathBuf::from("/home/testuser");
        let win = TmuxWindow {
            index: 32,
            name: CONSOLE_LABEL.into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
            current_command: "zsh".into(),
            start_command: format!("/bin/zsh -lc \"{}\"", console_shell_command()),
            pane_id: "%32".into(),
            pane_pid: 0,
            active: false,
            pane_dead: false,
            pane_dead_status: None,
            sessions_session_id: None,
        };
        let effective_command =
            effective_workspace_command("", &win.current_command);
        let at_shell_prompt = is_shell_command(effective_command);
        let (title, description, project) = resolve_session_names(
            &home,
            &win.cwd,
            None,
            None,
            win.name.as_str(),
            "",
            "",
            None,
            false,
        );
        assert!(at_shell_prompt);
        assert_eq!(project, "");
        assert_eq!(description, CONSOLE_LABEL);
        assert_eq!(title, CONSOLE_LABEL);
    }

    #[test]
    fn poll_names_agent_bootstrap_from_pane_start_when_current_is_shell() {
        let home = PathBuf::from("/home/testuser");
        let win = TmuxWindow {
            index: 27,
            name: "session".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
            current_command: "zsh".into(),
            start_command: r#"/bin/zsh -lc "grok || exec /bin/zsh -l""#.into(),
            pane_id: "%27".into(),
            pane_pid: 0,
            active: true,
            pane_dead: false,
            pane_dead_status: None,
            sessions_session_id: None,
        };
        let start_bootstrap = bootstrap_command_from_pane_start(&win.start_command).unwrap();
        let effective_command = effective_workspace_command(&start_bootstrap, &win.current_command);
        let runtime_agent = agent_from_command(effective_command);
        let workspace = runtime_agent
            .as_ref()
            .map(|agent| WorkspaceCatalog::workspace_ref_with_command(agent, effective_command));
        let (title, description, project) = resolve_session_names(
            &home,
            &win.cwd,
            runtime_agent.as_deref(),
            None,
            &win.name,
            "",
            "",
            workspace,
            false,
        );
        assert_eq!(project, "grok");
        assert_eq!(description, "?");
        assert_eq!(title, "grok · ?");
    }

    #[test]
    fn effective_pane_cwd_prefers_process_cwd_over_tmux() {
        let pid = std::process::id();
        let proc_cwd = std::env::current_dir()
            .expect("current dir")
            .display()
            .to_string();
        assert_eq!(effective_pane_cwd("/home/testuser", pid), proc_cwd);
    }

    #[test]
    fn clipboard_copy_pipe_prefers_pbcopy_on_macos() {
        if !Path::new("/usr/bin/pbcopy").is_file() {
            return;
        }
        let pipe = clipboard_copy_pipe_command().expect("pbcopy pipe");
        assert!(pipe.contains("pbcopy"));
        assert!(pipe.contains("display-message"));
    }

    #[test]
    fn workspace_settings_mode_detects_active_settings_shell() {
        let wrapper = r#"/bin/zsh -lc "/home/testuser/.local/bin/sessions settings; exec env -u TMUX tmux attach-session -t agents""#;
        assert!(workspace_pane_is_panel_mode(wrapper, "zsh", "settings"));
        assert!(!workspace_pane_is_panel_mode(wrapper, "tmux", "settings"));
    }

    #[test]
    fn workspace_settings_mode_ignores_normal_attach() {
        let attach = r#"/bin/zsh -lc "exec env -u TMUX tmux attach-session -t agents""#;
        assert!(!workspace_pane_is_panel_mode(attach, "tmux", "settings"));
        assert!(!workspace_pane_is_panel_mode(attach, "zsh", "settings"));
    }

    #[test]
    fn workspace_new_session_command_is_detected() {
        let cmd = "/bin/zsh -lc /home/testuser/.local/bin/sessions new-session";
        assert!(cmd.contains("new-session"));
        let legacy = "/bin/zsh -lc /home/testuser/.local/bin/sessions new-chat";
        assert!(workspace_pane_is_new_session_panel(legacy, legacy));
    }

    #[test]
    fn cancel_popup_on_all_clients_targets_every_client() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/daemon/tmux/popups.rs"));
        let cancel_block = source
            .lines()
            .skip_while(|line| !line.contains("fn cancel_popup_on_all_clients"))
            .take(25)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            cancel_block.contains(r#"["display-popup", "-C", "-c", client]"#),
            "cancel_popup_on_all_clients must cancel via -c target-client"
        );
    }

    #[test]
    fn close_ui_panel_popup_cancels_legacy_popups() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/daemon/tmux/popups.rs"));
        let close_block = source
            .lines()
            .skip_while(|line| !line.contains("pub fn close_ui_panel_popup"))
            .take(6)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            close_block.contains("cancel_popup_on_all_clients"),
            "close_ui_panel_popup must cancel legacy tmux popups"
        );
    }

    #[test]
    fn open_workspace_settings_respawns_workspace_pane() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/daemon/tmux/popups.rs"));
        let open_block = source
            .lines()
            .skip_while(|line| !line.contains("pub fn open_workspace_settings"))
            .take(12)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            open_block.contains("respawn-pane"),
            "settings must replace the workspace pane via respawn-pane"
        );
    }

    #[test]
    fn open_workspace_new_session_respawns_workspace_pane() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/daemon/tmux/popups.rs"));
        let open_block = source
            .lines()
            .skip_while(|line| !line.contains("pub fn open_workspace_new_session"))
            .take(20)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            open_block.contains("respawn-pane"),
            "new-session must replace the workspace pane via respawn-pane"
        );
        assert!(
            open_block.contains("workspace_new_session_wrapper"),
            "new-session wrapper must run sessions new-session then restore attach"
        );
    }

    #[test]
    fn ui_panel_toggle_script_targets_panel_subcommand() {
        let script = format!(
            "{} panel new-session",
            shell_quote(&sessions_binary().display().to_string())
        );
        assert!(script.contains("panel new-session"));
    }

    #[test]
    fn ui_panel_key_binding_uses_top_level_if_shell_with_run_shell() {
        let bin = shell_quote(&sessions_binary().display().to_string());
        let guard = panel_key_session_guard("sessions-ui", "agents");
        let panel_script = format!("{bin} panel open-new-session </dev/null >/dev/null 2>&1");
        let true_cmd = format!("run-shell -b \"{panel_script}\"");
        assert!(true_cmd.starts_with("run-shell -b"));
        assert!(guard.contains("pane_index"));
        assert!(!true_cmd.contains("if-shell"));
    }

    #[test]
    fn panel_key_bindings_apply_in_ui_and_agents_sessions() {
        let guard = panel_key_session_guard("sessions-ui", "agents");
        assert!(guard.contains("sessions-ui"));
        assert!(guard.contains("agents"));
        assert!(guard.contains("pane_index"));
        assert!(guard.starts_with("#{||:"));
    }

    #[test]
    fn workspace_panel_wrapper_runs_panel_then_restores_attach() {
        let wrapper = workspace_panel_wrapper("agents", "new-session").unwrap();
        assert!(wrapper.contains("new-session"));
        assert!(wrapper.contains("tmux attach") || wrapper.contains("attach-session"));
    }

    #[test]
    fn ui_click_binding_forwards_sidebar_mouse_without_dismiss() {
        let click = ui_click_binding("sessions-ui");
        assert!(!click.contains("panel dismiss"));
        assert!(click.contains("send-keys -M -t ="));
        assert!(click.contains("select-pane -t ="));
        assert!(click.contains("pane_index},0"));
    }

    #[test]
    fn console_shell_command_sets_black_terminal_backdrop() {
        let cmd = console_shell_command();
        assert!(cmd.contains("]11;#000000"));
        assert!(cmd.contains("exec /bin/zsh -l"));
    }

    #[test]
    fn workspace_shell_command_preserves_launcher_prompt_quoting() {
        let inner = "grok --model grok-build 'fix the bug\nplease help'";
        let cmd = workspace_shell_command(inner);
        assert_eq!(cmd, format!("{inner} || exec /bin/zsh -l"));
        let status = std::process::Command::new("/bin/zsh")
            .args(["-lc", &format!("{cmd}; echo OK")])
            .status()
            .expect("zsh");
        assert!(status.success());
    }

    #[test]
    fn host_terminal_backdrop_sets_sessions_tab_title() {
        let seq = host_terminal_backdrop_sequence();
        assert!(seq.contains("\x1b]0;sessions\x07"));
        assert!(seq.contains("\x1b]2;sessions\x07"));
        assert!(seq.contains("]11;#000000"));
    }

    #[test]
    fn attach_ui_session_sets_title_before_exec_alias_attach() {
        let script = format!(
            "{title_printf} && exec -a {title} {tmux} attach-session -t {session}",
            title_printf = host_terminal_title_bash_printf(),
            title = shell_quote(HOST_TERMINAL_TITLE),
            tmux = shell_quote("/usr/local/bin/tmux"),
            session = shell_quote("sessions-ui"),
        );
        assert!(script.contains("printf '\\033]0;sessions\\007"));
        assert!(
            script.contains("exec -a sessions /usr/local/bin/tmux attach-session -t sessions-ui")
        );
    }

    #[test]
    fn push_client_terminal_title_targets_attached_client_tty() {
        let script = push_client_terminal_title_script();
        assert!(script.contains("printf '\\033]0;sessions\\007"));
        assert!(script.contains("#{client_tty}"));
    }

    #[test]
    fn window_style_uses_sessions_black_backdrop() {
        assert_eq!(WINDOW_STYLE, "bg=#000000");
    }

    #[test]
    fn ui_pane_border_is_single_contrasting_line() {
        assert_eq!(UI_PANE_BORDER_LINES, "single");
        assert_eq!(UI_PANE_BORDER_STYLE, "fg=#4a4a4a,bg=#000000");
    }

    #[test]
    fn agents_pane_border_stays_invisible() {
        assert_eq!(PANE_BORDER_STYLE, "fg=#000000,bg=#000000");
    }

    #[test]
    fn nested_attach_shell_command_paints_workspace_backdrop() {
        let cmd = nested_attach_shell_command("agents").expect("attach command");
        assert!(cmd.contains("]11;#000000"));
        assert!(cmd.contains("attach-session"));
    }

    #[test]
    fn terminal_capabilities_include_graphics_passthrough() {
        // Keep in sync with configure_terminal_capabilities — Kitty graphics need this in tmux.
        const CAPS: &[(&str, &str)] = &[
            ("default-terminal", "tmux-256color"),
            ("terminal-features", "xterm-kitty:RGB"),
        ("terminal-features", "xterm-ghostty:extkeys"),
        ("terminal-features", "ghostty:extkeys"),
            ("allow-passthrough", "on"),
        ];
        assert!(CAPS
            .iter()
            .any(|(k, v)| *k == "allow-passthrough" && *v == "on"));
    }

    #[test]
    fn clipboard_paste_passes_through_sessions_tui_panes() {
        let tui = is_sessions_tui_pane_format();
        assert!(tui.contains("pane_current_command"));
        assert!(tui.contains("sessions"));
        assert_eq!(paste_pass_through_key(), "send-keys C-v");
    }

    #[test]
    fn clipboard_paste_uses_pbpaste_on_macos() {
        if !Path::new("/usr/bin/pbpaste").is_file() {
            return;
        }
        let paste = clipboard_paste_shell_command().unwrap();
        assert!(paste.contains("pbpaste"));
        assert!(paste.contains("if [ -n"));
        assert!(paste.contains("paste-buffer -p"));
        assert!(!paste.contains("pbpaste | tmux load-buffer"));
    }

    #[test]
    fn ui_pane_info_parses_list_panes_line() {
        let pane = UiPaneInfo::parse("1\t%42\t0\t\"exec sessions bar\"").expect("parse pane");
        assert_eq!(pane.index, 1);
        assert_eq!(pane.pane_id, "%42");
        assert!(!pane.dead);
        assert!(is_sidebar_start_command(&pane.start_command));
    }

    #[test]
    fn is_sidebar_start_command_detects_sessions_bar() {
        assert!(is_sidebar_start_command(
            r#""exec /home/testuser/.local/bin/sessions bar""#
        ));
        assert!(!is_sidebar_start_command(
            r#""exec sessions workspace-wrap""#
        ));
    }

    #[test]
    fn nested_attach_shell_command_uses_direct_tmux_attach() {
        let cmd = nested_attach_shell_command("agents").expect("attach command");
        assert!(cmd.contains("attach-session"));
        assert!(cmd.contains("env -u TMUX"));
        assert!(!cmd.contains("workspace-wrap"));
        assert!(!cmd.contains("screenrc-workspace"));
    }

    #[test]
    fn ui_wheel_forward_focuses_mouse_pane_before_send_keys() {
        let wheel = ui_wheel_binding("sessions-ui", "agents", WheelDirection::Up);
        assert!(wheel.contains("select-pane -t = ; send-keys -M"));
        assert!(
            wheel.contains("#{==:#{session_name},sessions-ui}"),
            "ui branch must be session-guarded: {wheel}"
        );
        assert!(
            !wheel.contains("agents:="),
            "wheel must not proxy to agents scrollback cross-session: {wheel}"
        );
        assert!(
            !wheel.contains("'select-pane"),
            "wheel forward must not break if-shell quoting: {wheel}"
        );
    }

    #[test]
    fn agents_wheel_up_enters_hidden_scrollback_only_without_mouse_app() {
        let wheel = ui_wheel_binding("sessions-ui", "agents", WheelDirection::Up);
        assert!(
            wheel.contains("#{||:#{pane_in_mode},#{mouse_any_flag}}"),
            "mouse-owning TUIs must keep the raw event: {wheel}"
        );
        assert!(
            wheel.contains("copy-mode -eH -t ="),
            "scrollback must auto-exit at bottom (-e) and hide the indicator (-H): {wheel}"
        );
        assert!(
            wheel.contains("send-keys -M -t ="),
            "all send-keys -M must target the pane under the mouse, not current pane: {wheel}"
        );
    }

    #[test]
    fn agents_wheel_down_never_enters_scrollback() {
        let wheel = ui_wheel_binding("sessions-ui", "agents", WheelDirection::Down);
        assert!(
            !wheel.contains("copy-mode"),
            "wheel down at bottom must be a no-op, not copy-mode: {wheel}"
        );
        assert!(wheel.contains("send-keys -M -t ="));
    }

    #[test]
    fn ui_drag_keeps_events_on_active_pane_without_reselect() {
        let drag = ui_drag_binding("sessions-ui", "agents");
        assert!(drag.contains("'send-keys -M'"));
        assert!(
            !drag.contains("select-pane -t ="),
            "reselecting on drag drops divider events into the workspace pane: {drag}"
        );
        assert!(
            drag.contains("#{==:#{session_name},sessions-ui}"),
            "ui branch must be session-guarded: {drag}"
        );
        assert!(
            !drag.contains("agents:="),
            "drag must not proxy cross-session: {drag}"
        );
    }

    #[test]
    fn ui_up_forwards_to_active_pane_without_reselect() {
        let up = ui_up_binding("sessions-ui", "agents");
        assert!(up.contains("'send-keys -M'"));
        assert!(
            !up.contains("select-pane -t ="),
            "mouseup must reach the drag-start pane: {up}"
        );
    }

    #[test]
    fn ui_drag_never_enters_copy_mode_in_sidebar_session() {
        let drag = ui_drag_binding("sessions-ui", "agents");
        let ui_branch_end = drag
            .find("#{==:#{session_name},agents}")
            .unwrap_or(drag.len());
        let ui_branch = &drag[..ui_branch_end];
        assert!(
            !ui_branch.contains("copy-mode"),
            "sidebar drags must reach the TUI, not copy-mode: {drag}"
        );
    }

    #[test]
    fn clamp_sidebar_width_allows_growth_past_bootstrap_default() {
        // 165-col clients used to cap at 55 via `min(client-48, client/3)`.
        assert_eq!(clamp_sidebar_width(80, 165), 80);
        assert_eq!(clamp_sidebar_width(120, 165), 117);
    }

    #[test]
    fn agents_drag_enters_copy_mode_without_mouse_app() {
        let drag = ui_drag_binding("sessions-ui", "agents");
        assert!(
            drag.contains("#{||:#{pane_in_mode},#{mouse_any_flag}}"),
            "mouse-owning TUIs must keep the raw event: {drag}"
        );
        assert!(
            drag.contains("copy-mode -M -t ="),
            "agents scrollback drag must use copy-mode: {drag}"
        );
    }
