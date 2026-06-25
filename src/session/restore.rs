use crate::agents::launcher::effective_restore_command_at;
use crate::config::Config;
use crate::daemon::server;
use crate::daemon::tmux::{self, sessions_binary};
use crate::model::ClientCommand;
use crate::session::lifecycle::{bootstrap_sessions_session_id, create_unified, LaunchSpec};
use crate::session::manifest::{load_manifest, ManifestEntry, SessionManifest};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

/// Full `sessions up` boot sequence: manifest restore, UI bootstrap, attach.
pub fn orchestrate_up(config: &Config) -> Result<()> {
    let manifest = load_manifest(config)?;
    let live_ssns = live_sessions_session_ids(&config.tmux_session);
    let live_set: HashSet<String> = live_ssns.keys().cloned().collect();

    ensure_daemon(config)?;
    send_prepare_restore(config)?;

    if needs_cold_boot_restore(&live_set, &manifest) {
        tmux::bootstrap_session(config)?;
        restore_missing_from_manifest(config, &manifest)?;
        if let Some(ref sessions_session_id) = manifest.last_active_sessions_session_id {
            let _ = tmux::select_window_by_sessions_session_id(
                &config.tmux_session,
                sessions_session_id,
            );
        }
    } else if !tmux::session_exists(&config.tmux_session) {
        tmux::bootstrap_session(config)?;
    }

    gc_stale_managed_after_restore(config);

    send_restore_complete(config)?;

    tmux::bootstrap_ui_session(
        &config.tmux_ui_session,
        &config.tmux_session,
        &sessions_binary(),
    )?;
    eprintln!("sessions ready — attaching tmux UI");
    tmux::attach_ui_session(&config.tmux_ui_session)
}

pub fn restore_missing_from_manifest(config: &Config, manifest: &SessionManifest) -> Result<()> {
    let live_ssns = live_sessions_session_ids(&config.tmux_session);
    let live_set: HashSet<String> = live_ssns.keys().cloned().collect();
    // Snapshot live ids once; skip duplicate manifest rows in this pass without
    // re-querying tmux — new windows are not visible until the next restore call.
    let mut restored_this_pass = HashSet::new();
    let mut bootstrap_agents = !tmux::session_exists(&config.tmux_session);

    for entry in entries_needing_restore(manifest, &live_set) {
        if !restored_this_pass.insert(entry.sessions_session_id.clone()) {
            continue;
        }
        let mut spec = launch_spec_from_entry(config, entry);
        if bootstrap_agents {
            spec.bootstrap_new_session = true;
            bootstrap_agents = false;
        }
        let created = create_unified(config, spec)
            .with_context(|| format!("restore {}", entry.sessions_session_id))?;
        seed_session_env_after_restore(config, entry, &created)?;
    }
    Ok(())
}

fn gc_stale_managed_after_restore(config: &Config) {
    if !tmux::session_exists(&config.tmux_session) {
        return;
    }
    let live_by_window: HashMap<u32, String> =
        tmux::list_live_sessions_session_ids(&config.tmux_session)
            .unwrap_or_default()
            .into_iter()
            .map(|(ssn, index)| (index, ssn))
            .collect();
    crate::session::managed::gc_managed_records_superseded_at_window(
        &config.home,
        &config.tmux_session,
        &live_by_window,
    );
}

fn live_sessions_session_ids(tmux_session: &str) -> HashMap<String, u32> {
    if !tmux::session_exists(tmux_session) {
        return HashMap::new();
    }
    tmux::list_live_sessions_session_ids(tmux_session).unwrap_or_default()
}

/// Cold-boot restore when open manifest entries are missing from live `@sessions.id` set.
pub(crate) fn needs_cold_boot_restore(
    live_sessions_session_ids: &HashSet<String>,
    manifest: &SessionManifest,
) -> bool {
    !entries_needing_restore(manifest, live_sessions_session_ids).is_empty()
}

pub(crate) fn workspace_bootstrap_closed(
    manifest: &SessionManifest,
    workspace_index: u32,
    cwd: &str,
    command: &str,
) -> bool {
    let sessions_session_id = bootstrap_sessions_session_id(workspace_index, cwd, command);
    manifest
        .entries
        .iter()
        .any(|entry| entry.sessions_session_id == sessions_session_id && entry.closed)
}

pub(crate) fn entries_needing_restore<'a>(
    manifest: &'a SessionManifest,
    live_sessions_session_ids: &HashSet<String>,
) -> Vec<&'a ManifestEntry> {
    let mut pending: Vec<&ManifestEntry> = manifest
        .entries
        .iter()
        .filter(|entry| {
            !entry.closed && !live_sessions_session_ids.contains(&entry.sessions_session_id)
        })
        .collect();
    pending.sort_by(|left, right| {
        use std::cmp::Ordering;
        match (left.messaged_at, right.messaged_at) {
            (Some(a), Some(b)) => b.cmp(&a),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => left.sessions_session_id.cmp(&right.sessions_session_id),
        }
    });
    pending
}

fn seed_session_env_after_restore(
    config: &Config,
    entry: &ManifestEntry,
    created: &tmux::CreatedWindow,
) -> Result<()> {
    let Some(agent_session_id) = entry.agent_session_id.as_deref() else {
        return Ok(());
    };
    tmux::write_session_env_tmux(
        config,
        agent_session_id,
        Some(&created.pane_id),
        Some(created.index),
        &config.tmux_session,
        Some(&entry.sessions_session_id),
        Some(entry.agent.as_str()),
    )
}

fn launch_spec_from_entry(config: &Config, entry: &ManifestEntry) -> LaunchSpec {
    launch_spec_from_entry_at(&config.home, entry)
}

fn launch_spec_from_entry_at(home: &Path, entry: &ManifestEntry) -> LaunchSpec {
    let launch_command = effective_restore_command_at(home, entry);
    let agent = crate::session::lifecycle::agent_for_launch_command(&launch_command);
    LaunchSpec {
        sessions_session_id: entry.sessions_session_id.clone(),
        source: entry.source,
        cwd: entry.cwd.clone(),
        agent,
        launch_command,
        workspace_index: entry.workspace_index,
        focus: false,
        window_name: entry.title.clone(),
        bootstrap_new_session: false,
        model_id: None,
        user_prompt: None,
    }
}

fn ensure_daemon(config: &Config) -> Result<()> {
    if server::socket_responds(&config.socket_path) {
        return Ok(());
    }
    let sessions = crate::paths::resolve_binary(&config.home)
        .to_string_lossy()
        .into_owned();
    std::process::Command::new(&sessions)
        .args(["daemon", "--foreground"])
        .stdout(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&config.log_path)?,
        )
        .stderr(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&config.log_path)?,
        )
        .spawn()?;
    for _ in 0..20 {
        if server::socket_responds(&config.socket_path) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::bail!("sessionsd failed to start");
}

fn send_prepare_restore(config: &Config) -> Result<()> {
    if !server::socket_responds(&config.socket_path) {
        return Ok(());
    }
    let mut stream = UnixStream::connect(&config.socket_path)
        .with_context(|| format!("connect {}", config.socket_path.display()))?;
    let line = serde_json::to_string(&ClientCommand::PrepareRestore)? + "\n";
    stream.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    Ok(())
}

pub fn send_restore_complete(config: &Config) -> Result<()> {
    let mut stream = UnixStream::connect(&config.socket_path)
        .with_context(|| format!("connect {}", config.socket_path.display()))?;
    let line = serde_json::to_string(&ClientCommand::RestoreComplete)? + "\n";
    stream.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::manifest::{append_entry, ManifestSource};
    use chrono::Utc;
    use tempfile::TempDir;

    const GROK_AGENT_SESSION_ID: &str = "019ef8c8-a1b2-73c4-d5e6-f78901234567";
    const CODEX_AGENT_SESSION_ID: &str = "019ef8c8-b1b2-73c4-d5e6-f78901234568";
    const CLAUDE_AGENT_SESSION_ID: &str = "06b67c89-bd76-4922-b6ec-518172be4267";
    const OPENCODE_AGENT_SESSION_ID: &str = "ses_14c367547ffe7g1N1inGGONKuZ";
    const STALE_AGENT_SESSION_ID: &str = "019ef8c8-0000-7000-8000-000000000099";

    fn sample_dynamic_entry(ssn: &str, cwd: &str, title: &str) -> ManifestEntry {
        ManifestEntry {
            sessions_session_id: ssn.into(),
            source: ManifestSource::NewChat,
            workspace_index: None,
            cwd: cwd.into(),
            cwd_label: cwd.into(),
            agent: "grok".into(),
            launch_command: format!("grok --model grok-build --resume {GROK_AGENT_SESSION_ID}"),
            agent_session_id: Some(GROK_AGENT_SESSION_ID.into()),
            title: Some(title.into()),
            messaged_at: Some(Utc::now()),
            closed: false,
        }
    }

    fn manifest_entry(
        agent: &str,
        launch_command: &str,
        agent_session_id: Option<&str>,
    ) -> ManifestEntry {
        ManifestEntry {
            sessions_session_id: "ssn_test_restore".into(),
            source: ManifestSource::NewChat,
            workspace_index: None,
            cwd: env!("CARGO_MANIFEST_DIR").into(),
            cwd_label: "~/sessions-cli".into(),
            agent: agent.into(),
            launch_command: launch_command.into(),
            agent_session_id: agent_session_id.map(str::to_string),
            title: None,
            messaged_at: None,
            closed: false,
        }
    }

    fn assert_launch_command_has_no_ssn_resume_args(command: &str) {
        assert!(
            !command.contains("ssn_"),
            "resume CLI args must use agent UUID, not sessions_session_id: {command}"
        );
    }

    #[test]
    fn reboot_dynamic_threads() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        config.state_path = crate::paths::state_dir(dir.path()).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        let cwd = "/tmp/reboot-dynamic";
        let threads = [
            ("ssn_dyn_1", "thread one"),
            ("ssn_dyn_2", "thread two"),
            ("ssn_dyn_3", "thread three"),
            ("ssn_dyn_4", "thread four"),
            ("ssn_dyn_5", "thread five"),
        ];
        for (ssn, title) in threads {
            append_entry(&config, sample_dynamic_entry(ssn, cwd, title)).unwrap();
        }

        let manifest = load_manifest(&config).unwrap();
        let live = HashSet::new();
        let pending = entries_needing_restore(&manifest, &live);
        assert_eq!(pending.len(), 5);
        assert!(pending
            .iter()
            .all(|entry| entry.sessions_session_id.starts_with("ssn_dyn_")));
        assert!(pending.iter().all(|entry| !entry.closed));

        let session_dir =
            crate::agents::grok::session_dir(&config.home, cwd, GROK_AGENT_SESSION_ID);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("summary.json"),
            r#"{"generated_title":"test"}"#,
        )
        .unwrap();
        for entry in &pending {
            let spec = launch_spec_from_entry_at(&config.home, entry);
            assert!(
                spec.launch_command.contains(GROK_AGENT_SESSION_ID),
                "resume must use agent UUID: {}",
                spec.launch_command
            );
            assert_launch_command_has_no_ssn_resume_args(&spec.launch_command);
        }

        let partial_live = HashSet::from(["ssn_dyn_2".into(), "ssn_dyn_4".into()]);
        let pending = entries_needing_restore(&manifest, &partial_live);
        assert_eq!(pending.len(), 3);
        assert!(!pending
            .iter()
            .any(|entry| partial_live.contains(&entry.sessions_session_id)));
    }

    #[test]
    fn down_then_up() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        config.state_path = crate::paths::state_dir(dir.path()).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        append_entry(
            &config,
            sample_dynamic_entry("ssn_down_up", "/tmp/down-up", "survives down"),
        )
        .unwrap();
        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: bootstrap_sessions_session_id(0, "/tmp/ws", "grok"),
                source: ManifestSource::WorkspaceBootstrap,
                workspace_index: Some(0),
                cwd: "/tmp/ws".into(),
                cwd_label: "/tmp/ws".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: None,
                title: Some("workspace".into()),
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let manifest = load_manifest(&config).unwrap();
        assert_eq!(entries_needing_restore(&manifest, &HashSet::new()).len(), 2);
        assert!(needs_cold_boot_restore(&HashSet::new(), &manifest));
    }

    #[test]
    fn up_with_existing_agents_no_dup() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        config.state_path = crate::paths::state_dir(dir.path()).join("sessionsd.json");

        append_entry(
            &config,
            sample_dynamic_entry("ssn_existing", "/tmp/existing", "already live"),
        )
        .unwrap();
        let manifest = load_manifest(&config).unwrap();

        let live = HashSet::from(["ssn_existing".into()]);
        assert!(entries_needing_restore(&manifest, &live).is_empty());
        assert!(!needs_cold_boot_restore(&live, &manifest));
    }

    #[test]
    fn restore_runs_when_empty_agents_session() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        config.state_path = crate::paths::state_dir(dir.path()).join("sessionsd.json");

        append_entry(
            &config,
            sample_dynamic_entry("ssn_empty_agents", "/tmp/empty-agents", "needs restore"),
        )
        .unwrap();
        let manifest = load_manifest(&config).unwrap();

        let live = HashSet::new();
        assert!(needs_cold_boot_restore(&live, &manifest));
        assert_eq!(entries_needing_restore(&manifest, &live).len(), 1);
    }

    #[test]
    fn resume_when_agent_session_id_present() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");

        let session_dir = crate::agents::grok::session_dir(home, cwd, GROK_AGENT_SESSION_ID);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("summary.json"),
            r#"{"generated_title":"test"}"#,
        )
        .unwrap();

        let entry = manifest_entry(
            "grok",
            "grok --model grok-build --resume stale-ssn-should-not-appear",
            Some(GROK_AGENT_SESSION_ID),
        );
        let spec = launch_spec_from_entry_at(home, &entry);
        assert_eq!(
            spec.launch_command,
            format!("grok --model grok-build --resume {GROK_AGENT_SESSION_ID}")
        );
        assert_launch_command_has_no_ssn_resume_args(&spec.launch_command);
    }

    #[test]
    fn fallback_when_stale_id() {
        let home = Path::new("/tmp");
        let entry = manifest_entry(
            "grok",
            "grok --model grok-build --resume stale",
            Some(STALE_AGENT_SESSION_ID),
        );
        let spec = launch_spec_from_entry_at(home, &entry);
        assert_eq!(spec.launch_command, "grok");
        assert_launch_command_has_no_ssn_resume_args(&spec.launch_command);
    }

    #[test]
    fn skip_resume_for_console() {
        let home = Path::new("/tmp");
        let entry = manifest_entry("console", "", None);
        let spec = launch_spec_from_entry_at(home, &entry);
        assert_eq!(spec.launch_command, "");
        assert_launch_command_has_no_ssn_resume_args(&spec.launch_command);

        let script = manifest_entry("console", "./run-local.sh", None);
        let spec = launch_spec_from_entry_at(home, &script);
        assert_eq!(spec.launch_command, "./run-local.sh");
        assert_launch_command_has_no_ssn_resume_args(&spec.launch_command);
    }

    #[test]
    fn entries_needing_restore_sorted_by_messaged_at_desc() {
        let older = Utc::now() - chrono::Duration::hours(2);
        let newer = Utc::now() - chrono::Duration::minutes(5);
        let mut old_entry = sample_dynamic_entry("ssn_old", "/tmp/order", "old");
        old_entry.messaged_at = Some(older);
        let mut new_entry = sample_dynamic_entry("ssn_new", "/tmp/order", "new");
        new_entry.messaged_at = Some(newer);
        let manifest = SessionManifest {
            version: 1,
            last_active_sessions_session_id: None,
            migrated_from: None,
            entries: vec![old_entry, new_entry],
        };
        let pending = entries_needing_restore(&manifest, &HashSet::new());
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].sessions_session_id, "ssn_new");
        assert_eq!(pending[1].sessions_session_id, "ssn_old");
    }

    #[test]
    fn launch_spec_agent_matches_resume_command() {
        use crate::agents::launcher::build_resume_command;

        let cwd = env!("CARGO_MANIFEST_DIR");
        let cases: &[(&str, &str, &str)] = &[
            ("grok", GROK_AGENT_SESSION_ID, "grok --model grok-build"),
            ("codex", CODEX_AGENT_SESSION_ID, "codex --model gpt-5.4"),
            ("claude", CLAUDE_AGENT_SESSION_ID, "claude --model sonnet"),
            ("opencode", OPENCODE_AGENT_SESSION_ID, "opencode"),
        ];

        for (agent, session_id, launch_command) in cases {
            let dir = TempDir::new().unwrap();
            let home = dir.path();
            seed_agent_session_on_disk(home, cwd, agent, session_id);

            let entry = manifest_entry(agent, launch_command, Some(session_id));
            let spec = launch_spec_from_entry_at(home, &entry);
            let expected_resume = build_resume_command(agent, launch_command, session_id);
            assert_eq!(spec.agent, *agent, "agent id for {agent}");
            assert_eq!(
                spec.launch_command, expected_resume,
                "resume command for {agent}"
            );
            assert_launch_command_has_no_ssn_resume_args(&spec.launch_command);
        }
    }

    fn seed_agent_session_on_disk(home: &Path, cwd: &str, agent: &str, session_id: &str) {
        match agent {
            "grok" => {
                let session_dir = crate::agents::grok::session_dir(home, cwd, session_id);
                std::fs::create_dir_all(&session_dir).unwrap();
                std::fs::write(
                    session_dir.join("summary.json"),
                    r#"{"generated_title":"test"}"#,
                )
                .unwrap();
            }
            "codex" => {
                let rollout_dir = home.join(".codex/sessions/2026/06/10");
                std::fs::create_dir_all(&rollout_dir).unwrap();
                std::fs::write(
                    rollout_dir.join(format!(
                        "rollout-2026-06-10T18-21-59-{session_id}.jsonl"
                    )),
                    format!(
                        r#"{{"type":"session_meta","payload":{{"id":"{session_id}","cwd":"{cwd}"}}}}"#
                    ),
                )
                .unwrap();
            }
            "claude" => {
                let project_dir = crate::agents::claude::claude_home(home)
                    .join("projects")
                    .join(crate::agents::claude::encode_claude_project_dir(cwd));
                std::fs::create_dir_all(&project_dir).unwrap();
                std::fs::write(
                    project_dir.join(format!("{session_id}.jsonl")),
                    format!(
                        r#"{{"type":"user","message":{{"role":"user","content":"resume test"}},"cwd":"{cwd}","timestamp":"2026-06-11T06:38:24.372Z"}}"#
                    ),
                )
                .unwrap();
            }
            "opencode" => {
                use rusqlite::Connection;

                let path = crate::agents::opencode::opencode_db_path(home);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let conn = Connection::open(&path).unwrap();
                conn.execute_batch(
                    "CREATE TABLE session (
                        id TEXT PRIMARY KEY,
                        title TEXT NOT NULL,
                        directory TEXT NOT NULL,
                        time_updated INTEGER NOT NULL
                    );",
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO session (id, title, directory, time_updated) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![session_id, "resume test", cwd, 1_781_134_800_000_i64],
                )
                .unwrap();
            }
            other => panic!("unsupported agent: {other}"),
        }
    }

    #[test]
    fn restore_seeds_session_env_for_bound_agent() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        config.tmux_session = "agents-restore-seed".into();

        let entry = sample_dynamic_entry("ssn_restore_seed", "/tmp/restore-seed", "seed env");
        let created = tmux::CreatedWindow {
            index: 3,
            pane_id: "%77".into(),
            sessions_session_id: entry.sessions_session_id.clone(),
        };

        seed_session_env_after_restore(&config, &entry, &created).unwrap();

        let env =
            crate::session::env::load_session_env(&config.session_env_path(GROK_AGENT_SESSION_ID));
        assert_eq!(env.tmux_pane_id.as_deref(), Some("%77"));
        assert_eq!(env.window_index, Some(3));
        assert_eq!(env.tmux_session.as_deref(), Some("agents-restore-seed"));
        assert_eq!(env.sessions_session_id.as_deref(), Some("ssn_restore_seed"));
        assert_eq!(env.managed_agent.as_deref(), Some("grok"));
    }

    #[test]
    fn restore_skips_duplicate_manifest_entries() {
        let manifest = SessionManifest {
            version: 1,
            last_active_sessions_session_id: None,
            migrated_from: None,
            entries: vec![
                sample_dynamic_entry("ssn_dup", "/tmp/dup", "first"),
                sample_dynamic_entry("ssn_dup", "/tmp/dup", "duplicate"),
            ],
        };
        let pending = entries_needing_restore(&manifest, &HashSet::new());
        assert_eq!(pending.len(), 2);

        let mut restored_this_pass = HashSet::new();
        let unique: Vec<_> = pending
            .iter()
            .filter(|entry| restored_this_pass.insert(entry.sessions_session_id.clone()))
            .collect();
        assert_eq!(unique.len(), 1);
    }
}
