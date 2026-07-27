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
    assert!(parts
        .iter()
        .all(|part| part.len() <= TMUX_SEND_KEYS_MAX_LITERAL));
    assert_eq!(parts.concat(), ascii);

    let emoji = "😀".repeat(600);
    let parts: Vec<_> = literal_send_chunks(&emoji).map(str::to_string).collect();
    assert!(parts
        .iter()
        .all(|part| part.len() <= TMUX_SEND_KEYS_MAX_LITERAL));
    assert_eq!(parts.concat(), emoji);
}

#[test]
fn auto_window_switch_disable_options_block_bell_and_activity() {
    // Global tmux defaults are often `bell-action any` / `activity-action other`,
    // which steal the nested right pane when a background agent finishes with BEL.
    // Workspace configure must pin both to none so only explicit focus switches.
    let opts = auto_window_switch_disable_options();
    assert!(opts.contains(&("bell-action", "none")));
    assert!(opts.contains(&("activity-action", "none")));
    assert_eq!(opts.len(), 2);
}

#[test]
fn idle_shell_poll_ignores_stale_tmux_window_name() {
    let home = PathBuf::from("/home/testuser");
    let cwd = "/home/testuser/projects/acme";
    let workspace = WorkspaceCatalog::workspace_ref_with_command("acme · main workspace", "zsh");
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
    assert_eq!(title, "acme");
    assert_eq!(description, "acme");
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
        pool: false,
    };
    let bootstrap_command = "";
    let effective_command = effective_workspace_command(bootstrap_command, &win.current_command);
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
        pool: false,
    };
    let start_bootstrap = bootstrap_command_from_pane_start(&win.start_command);
    let bootstrap_command = start_bootstrap.as_deref().unwrap_or("");
    let effective_command = effective_workspace_command(bootstrap_command, &win.current_command);
    let mut runtime_agent = agent_from_command(effective_command);
    if runtime_agent.is_none() {
        runtime_agent = Some("grok".into());
    }
    let at_shell_prompt = is_shell_command(effective_command);
    let workspace = runtime_agent
        .as_ref()
        .map(|agent| WorkspaceCatalog::workspace_ref_with_command(agent, effective_command));
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
    // Home shell with window name "console" stays console.
    let win = TmuxWindow {
        index: 32,
        name: CONSOLE_LABEL.into(),
        cwd: home.display().to_string(),
        current_command: "zsh".into(),
        start_command: format!("/bin/zsh -lc \"{}\"", console_shell_command()),
        pane_id: "%32".into(),
        pane_pid: 0,
        active: false,
        pane_dead: false,
        pane_dead_status: None,
        sessions_session_id: None,
        pool: false,
    };
    let effective_command = effective_workspace_command("", &win.current_command);
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

    // Project-dir shell named "console" resolves to the directory leaf instead.
    let (title, description, _) = resolve_session_names(
        &home,
        env!("CARGO_MANIFEST_DIR"),
        None,
        None,
        CONSOLE_LABEL,
        "",
        "",
        None,
        false,
    );
    assert_eq!(description, "sessions-cli");
    assert_eq!(title, "sessions-cli");
}

#[test]
fn idle_managed_console_ignores_workspace_agent_bootstrap() {
    let home = PathBuf::from("/home/testuser");
    let cwd = env!("CARGO_MANIFEST_DIR");
    let workspace =
        WorkspaceCatalog::workspace_ref_with_command("sessions-cli · stale workspace task", "grok");
    let (title, description, project) = resolve_session_names(
        &home,
        cwd,
        None,
        None,
        CONSOLE_LABEL,
        "",
        "",
        Some(workspace),
        false,
    );
    assert_eq!(title, CONSOLE_LABEL);
    assert_eq!(description, CONSOLE_LABEL);
    assert!(project.is_empty());
}

#[test]
fn idle_managed_agent_keeps_placeholder_over_workspace_thread() {
    let home = PathBuf::from("/home/testuser");
    let cwd = env!("CARGO_MANIFEST_DIR");
    let workspace =
        WorkspaceCatalog::workspace_ref_with_command("sessions-cli · stale workspace task", "grok");
    let (title, description, project) = resolve_session_names(
        &home,
        cwd,
        Some("grok"),
        None,
        "grok · ?",
        "",
        "",
        Some(workspace),
        false,
    );
    assert_eq!(project, "grok");
    assert_eq!(description, "?");
    assert_eq!(title, "grok · ?");
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
        pool: false,
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
fn workspace_mcps_mode_detects_active_mcps_shell() {
    let wrapper = r#"/bin/zsh -lc "/home/testuser/.local/bin/sessions mcps; exec env -u TMUX tmux attach-session -t agents""#;
    assert!(workspace_pane_is_panel_mode(wrapper, "zsh", "mcps"));
    assert!(!workspace_pane_is_panel_mode(wrapper, "tmux", "mcps"));
    assert!(!workspace_pane_is_panel_mode(wrapper, "zsh", "automations"));
}

#[test]
fn workspace_mcps_mode_ignores_normal_attach() {
    let attach = r#"/bin/zsh -lc "exec env -u TMUX tmux attach-session -t agents""#;
    assert!(!workspace_pane_is_panel_mode(attach, "tmux", "mcps"));
    assert!(!workspace_pane_is_panel_mode(attach, "zsh", "mcps"));
}

#[test]
fn cancel_popup_on_all_clients_targets_every_client() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/daemon/tmux/popups.rs"
    ));
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
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/daemon/tmux/popups.rs"
    ));
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
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/daemon/tmux/popups.rs"
    ));
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
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/daemon/tmux/popups.rs"
    ));
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
    assert!(click.contains("send-keys -M"));
    assert!(!click.contains("send-keys -M -t ="));
    assert!(click.contains("select-pane -t ="));
    assert!(
        click.contains("#{==:#{session_name},sessions-ui}"),
        "UI clicks must select the pane under the cursor: {click}"
    );
    assert!(
        !click.contains("pane_index},0"),
        "all UI-session clicks must select-pane, not only sidebar pane 0: {click}"
    );
    assert!(
        click.contains("select-pane -t = ; send-keys -M"),
        "click must use a literal semicolon to separate commands: {click}"
    );
    assert!(
        !click.contains("\\;"),
        "quoted \\; in click binding is treated literally by tmux: {click}"
    );
    assert!(
        !click.contains("{ select-pane"),
        "brace groups inside if-shell branches cause runtime syntax errors: {click}"
    );
}

/// Bind a key on an isolated tmux server so syntax is validated without racing
/// the developer's live sessions socket (or a missing default server).
fn assert_tmux_bind_parses(key: &str, binding: &str) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static BIND_SEQ: AtomicU64 = AtomicU64::new(0);

    let Some(_) = which_executable("tmux") else {
        eprintln!("skip: tmux not on PATH");
        return;
    };
    // Unique socket + session per call so parallel cargo tests never collide.
    let seq = BIND_SEQ.fetch_add(1, Ordering::Relaxed);
    let socket = format!(
        "sessions-bind-{}-{}-{}",
        std::process::id(),
        seq,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let session = format!("bind-check-{seq}");
    let start = std::process::Command::new("tmux")
        .args(["-L", &socket, "new-session", "-d", "-s", &session])
        .output()
        .expect("tmux new-session");
    if !start.status.success() {
        let _ = std::process::Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .status();
        panic!(
            "tmux new-session failed: {}",
            String::from_utf8_lossy(&start.stderr)
        );
    }
    let bind = std::process::Command::new("tmux")
        .args(["-L", &socket, "bind-key", "-T", "root", key, binding])
        .output()
        .expect("tmux bind-key");
    let _ = std::process::Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .status();
    assert!(
        bind.status.success(),
        "{key} binding must parse in tmux: {binding}\nstderr: {}",
        String::from_utf8_lossy(&bind.stderr)
    );
}

#[test]
fn ui_click_binding_is_valid_tmux_syntax() {
    assert_tmux_bind_parses("MouseDown1Pane", &ui_click_binding("sessions-ui"));
}

#[test]
fn ui_root_bindings_avoid_brace_command_groups() {
    for binding in [
        ui_click_binding("sessions-ui"),
        ui_drag_binding("sessions-ui", "agents"),
        ui_up_binding("sessions-ui", "agents"),
        ui_wheel_binding("sessions-ui", "agents", WheelDirection::Up),
        ui_wheel_binding("sessions-ui", "agents", WheelDirection::Down),
    ] {
        assert!(
            !binding.contains("{ send-keys")
                && !binding.contains("{ select-pane")
                && !binding.contains("{ copy-mode"),
            "root mouse bindings must not use brace groups inside if-shell branches: {binding}"
        );
    }
}

/// Drive the generated bindings through a real tmux server + pty client.
/// This catches runtime syntax errors that tmux only reports on a real
/// mouse event, not at bind-key time.
///
/// Ignored by default because it forks a pty client and is unreliable when
/// run in parallel with other tmux tests. Run manually or in CI with:
///   cargo test -- --ignored --test-threads=1
#[test]
#[ignore = "requires pty; run with --test-threads=1"]
fn ui_mouse_bindings_do_not_flash_syntax_error_at_runtime() {
    let python = std::env::var_os("PYTHON")
        .or_else(|| which_executable("python3"))
        .or_else(|| which_executable("python"));
    let Some(python) = python else {
        eprintln!("python not available; skipping runtime mouse binding test");
        return;
    };
    let manifest = std::env!("CARGO_MANIFEST_DIR");
    let script = std::path::Path::new(manifest).join("tests/ui_mouse_bindings.py");
    let output = std::process::Command::new(python)
        .arg(&script)
        .env("CLICK", ui_click_binding("test-ui"))
        .env("UP", ui_up_binding("test-ui", "test-agents"))
        .env("DRAG", ui_drag_binding("test-ui", "test-agents"))
        .env(
            "WHEEL_UP",
            ui_wheel_binding("test-ui", "test-agents", WheelDirection::Up),
        )
        .env(
            "WHEEL_DOWN",
            ui_wheel_binding("test-ui", "test-agents", WheelDirection::Down),
        )
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .output()
        .expect("run python mouse binding test");
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("runtime mouse binding test failed:\nstdout: {stdout}\nstderr: {stderr}");
    }
}

fn which_executable(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| p.is_file())
            .map(|p| p.into_os_string())
    })
}

#[test]
fn ui_multi_click_bindings_avoid_copy_mode_chains() {
    let click = ui_click_binding("sessions-ui");
    for key in ["DoubleClick1Pane", "TripleClick1Pane"] {
        assert!(
            !click.contains("copy-mode"),
            "{key} must not enter copy-mode: {click}"
        );
        assert_tmux_bind_parses(key, &click);
    }
}

#[test]
fn ui_drag_binding_is_valid_tmux_syntax() {
    assert_tmux_bind_parses("MouseDrag1Pane", &ui_drag_binding("sessions-ui", "agents"));
}

#[test]
fn ui_up_binding_is_valid_tmux_syntax() {
    assert_tmux_bind_parses("MouseUp1Pane", &ui_up_binding("sessions-ui", "agents"));
}

#[test]
fn console_shell_command_sets_black_terminal_backdrop() {
    let cmd = console_shell_command();
    assert!(cmd.contains("]11;#000000"));
    assert!(cmd.contains("/bin/zsh -l"));
    assert!(
        cmd.contains("-u NO_COLOR"),
        "console shell must force color: {cmd}"
    );
}

#[test]
fn workspace_shell_command_preserves_launcher_prompt_quoting() {
    let inner = "grok --model grok-build 'fix the bug\nplease help'";
    let cmd = workspace_shell_command(inner);
    assert!(cmd.contains(inner), "inner launch command preserved: {cmd}");
    assert!(cmd.contains("|| exec"), "fallback shell present: {cmd}");
    assert!(
        cmd.contains("-u NO_COLOR"),
        "fallback shell must force color: {cmd}"
    );
    if !Path::new("/bin/zsh").is_file() {
        eprintln!("skip: /bin/zsh not present");
        return;
    }
    // Command fails if grok is missing; only validate zsh accepts the quoting.
    let status = std::process::Command::new("/bin/zsh")
        .args(["-n", "-c", &cmd])
        .status()
        .expect("zsh");
    assert!(status.success(), "zsh -n rejected command: {cmd}");
}

#[test]
fn host_terminal_backdrop_sets_sessions_tab_title() {
    let seq = host_terminal_backdrop_sequence();
    assert!(seq.contains("\x1b]0;sessions\x07"));
    assert!(seq.contains("\x1b]1;sessions\x07"));
    assert!(seq.contains("\x1b]2;sessions\x07"));
    assert!(seq.contains("\x1b]0;sessions\x1b\\"));
    // OSC 11: BEL + ST for xterm.js (VS Code) terminator quirks; `#RRGGBB` form.
    assert!(seq.contains("\x1b]11;#000000\x07"));
    assert!(seq.contains("\x1b]11;#000000\x1b\\"));
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
    assert!(script.contains("exec -a sessions /usr/local/bin/tmux attach-session -t sessions-ui"));
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
    assert!(
        cmd.contains("exec -a sessions"),
        "nested agents client must rename argv0 so IDE tabs say sessions: {cmd}"
    );
}

#[test]
fn terminal_capabilities_include_graphics_passthrough() {
    // Keep in sync with configure_terminal_capabilities — Kitty graphics need this in tmux.
    const CAPS: &[(&str, &str)] = &[
        ("default-terminal", "tmux-256color"),
        ("allow-passthrough", "on"),
    ];
    assert!(CAPS
        .iter()
        .any(|(k, v)| *k == "allow-passthrough" && *v == "on"));
    // Appended features (Cursor/VS Code xterm.js + kitty/ghostty + nested clipboard).
    const FEATURES: &[&str] = &[
        ",xterm-kitty:RGB",
        ",xterm-ghostty:extkeys",
        ",ghostty:extkeys",
        ",xterm-256color:RGB",
        ",xterm*:RGB",
        // Nested `tmux attach` client TERM — required for Grok OSC 52 copy.
        ",tmux*:clipboard",
        ",tmux-256color:clipboard",
    ];
    assert!(FEATURES.iter().any(|f| f.contains("xterm-256color:RGB")));
    assert!(
        FEATURES.iter().any(|f| f.contains("tmux*:clipboard")),
        "nested agents client must advertise clipboard for Grok OSC 52"
    );
}

#[test]
fn agents_drag_always_forwards_not_copy_mode() {
    // Grok native select: always forward agents drag — never gate on mouse_any
    // (that flag flickers and used to drop into copy-mode).
    let drag = ui_drag_binding("sessions-ui", "agents");
    assert!(
        !drag.contains("mouse_any_flag"),
        "agents drag must not require mouse_any_flag: {drag}"
    );
    assert!(
        drag.contains("#{==:#{session_name},agents}"),
        "agents session must be in the forward guard: {drag}"
    );
    assert!(
        drag.contains("'send-keys -M'"),
        "forward path required for native TUI selection: {drag}"
    );
}

#[test]
fn osc52_os_clipboard_hook_pipes_buffer() {
    let Some(hook) = osc52_os_clipboard_hook_command() else {
        // CI without pbcopy/xclip/wl-copy — skip content check.
        return;
    };
    assert!(
        hook.contains("save-buffer"),
        "hook must read tmux paste buffer: {hook}"
    );
    assert!(
        hook.contains("pbcopy") || hook.contains("xclip") || hook.contains("wl-copy"),
        "hook must pipe to an OS clipboard tool: {hook}"
    );
}

#[test]
fn clipboard_paste_passes_through_sessions_tui_panes() {
    let tui = is_sessions_tui_pane_format();
    assert!(tui.contains("pane_current_command"));
    assert!(tui.contains("sessions"));
    assert_eq!(paste_pass_through_key("C-v"), "send-keys C-v");
    assert_eq!(paste_pass_through_key("M-v"), "send-keys M-v");
}

#[test]
fn clipboard_paste_shell_command_invokes_sessions_paste_tmux() {
    let paste = clipboard_paste_shell_command();
    assert!(paste.contains("paste-tmux"));
    assert!(paste.contains("-t #{pane_id}"));
    assert!(!paste.contains("bash -lc"));
    assert!(!paste.contains("pbpaste"));
    let load = clipboard_paste_load_shell_command();
    assert!(load.contains("paste-tmux --load-only"));
    let key_cmd = clipboard_paste_key_command();
    assert!(key_cmd.contains("run-shell \""));
    assert!(key_cmd.contains("paste-tmux --load-only"));
    assert!(key_cmd.contains("paste-buffer -p"));
    let run = clipboard_paste_run_shell_command();
    assert!(run.starts_with("run-shell \""));
    assert!(run.contains("paste-tmux"));
}

#[test]
fn clipboard_paste_root_bindings_parse_in_tmux() {
    for key in ["C-v", "M-v"] {
        let binding = clipboard_paste_root_binding(key);
        assert!(
            binding.contains("paste-tmux --load-only"),
            "{key} binding should load via paste-tmux: {binding}"
        );
        assert!(
            binding.contains("paste-buffer -p"),
            "{key} binding should paste-buffer in key context: {binding}"
        );
        assert!(
            !binding.contains("bash -lc"),
            "{key} binding must not nest bash -lc quotes: {binding}"
        );
        // Real tmux bind-key — catches the quote-syntax error that broke C-v.
        assert_tmux_bind_parses(key, &binding);
    }
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
fn parse_tmux_client_line_reads_fields() {
    let client =
        parse_tmux_client_line("/dev/ttys038\tagents\t6634\ttmux-256color").expect("parse client");
    assert_eq!(client.tty, "/dev/ttys038");
    assert_eq!(client.session, "agents");
    assert_eq!(client.pid, 6634);
    assert_eq!(client.term, "tmux-256color");
}

#[test]
fn filter_stray_agents_clients_keeps_nested_workspace_tty() {
    let clients = vec![
        TmuxClient {
            tty: "/dev/ttys038".into(),
            session: "agents".into(),
            pid: 6634,
            term: "tmux-256color".into(),
        },
        TmuxClient {
            tty: "/dev/ttys099".into(),
            session: "agents".into(),
            pid: 9999,
            term: "xterm-256color".into(),
        },
        TmuxClient {
            tty: "/dev/ttys001".into(),
            session: "sessions-ui".into(),
            pid: 1,
            term: "xterm-256color".into(),
        },
    ];
    let strays = filter_stray_agents_clients(&clients, "agents", Some("/dev/ttys038"));
    assert_eq!(strays.len(), 1);
    assert_eq!(strays[0].tty, "/dev/ttys099");
}

#[test]
fn filter_stray_agents_clients_all_bare_when_no_ui() {
    let clients = vec![TmuxClient {
        tty: "/dev/ttys038".into(),
        session: "agents".into(),
        pid: 6634,
        term: "tmux-256color".into(),
    }];
    let strays = filter_stray_agents_clients(&clients, "agents", None);
    assert_eq!(strays.len(), 1);
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
fn ui_wheel_forwards_without_reselecting_pane() {
    let wheel = ui_wheel_binding("sessions-ui", "agents", WheelDirection::Up);
    assert!(
        !wheel.contains("select-pane -t ="),
        "ui wheel must not reselect panes (steals workspace focus): {wheel}"
    );
    assert!(wheel.contains("'send-keys -M'"));
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
        wheel.contains("copy-mode -eH"),
        "scrollback must auto-exit at bottom (-e) and hide the indicator (-H): {wheel}"
    );
    assert!(
        wheel.contains("send-keys -M"),
        "forward wheel events to the active agents pane: {wheel}"
    );
    assert!(
        !wheel.contains("-t ="),
        "nested agents dispatch must not use mouse target =: {wheel}"
    );
}

#[test]
fn agents_wheel_down_never_enters_scrollback() {
    let wheel = ui_wheel_binding("sessions-ui", "agents", WheelDirection::Down);
    assert!(
        !wheel.contains("copy-mode"),
        "wheel down at bottom must be a no-op, not copy-mode: {wheel}"
    );
    assert!(wheel.contains("send-keys -M"));
    assert!(!wheel.contains("-t ="));
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
fn clamp_sidebar_width_allows_collapsed_rail() {
    assert_eq!(
        clamp_sidebar_width(COLLAPSED_SIDEBAR_WIDTH, 80),
        COLLAPSED_SIDEBAR_WIDTH
    );
    assert_eq!(clamp_sidebar_width(1, 80), 1);
    // Drag-to-collapse zone is reachable (snaps to rail at/below this width).
    assert_eq!(clamp_sidebar_width(10, 80), 16);
    assert_eq!(clamp_sidebar_width(16, 80), 16);
}

#[test]
fn clamp_sidebar_width_peek_allows_near_default_width() {
    // 80-col client: normal clamp leaves only 32 for the sidebar; peek floor
    // leaves room for the full default (54) list width.
    assert_eq!(clamp_sidebar_width(54, 80), 32);
    assert_eq!(
        clamp_sidebar_width_with_workspace_min(54, 80, PEEK_WORKSPACE_MIN),
        54
    );
    // Desired wider than default still grows until the peek workspace floor.
    assert_eq!(
        clamp_sidebar_width_with_workspace_min(60, 80, PEEK_WORKSPACE_MIN),
        56
    );
}

#[test]
fn agents_drag_never_uses_mouse_target_equals() {
    let drag = ui_drag_binding("sessions-ui", "agents");
    assert!(
        !drag.contains("-t ="),
        "nested dispatch must not use mouse target =: {drag}"
    );
    // copy-mode only for non-sessions sessions (false branch).
    assert!(
        drag.contains("copy-mode -M"),
        "non-agents sessions still get copy-mode fallback: {drag}"
    );
}
