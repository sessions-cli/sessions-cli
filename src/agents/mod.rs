mod adapter;
pub mod claude;
pub mod codex;
pub mod common;
pub mod grok;
pub mod launcher;
pub mod opencode;
pub mod parse_cache;
pub mod registry;
mod traits;

pub use adapter::{turn_is_complete, AgentAdapter, SessionSummary};
pub use claude::{
    assign_session_for_cwd as assign_claude_session_for_cwd, claude_session_index, Claude,
};
pub use codex::{assign_thread_for_cwd, rollout_index, Codex};
pub use grok::Grok;
// Restore-v2 APIs (PR 1) — resume command builders; wired into restore in PR 2.
pub use launcher::{
    agent_accepts_cli_prompt, agent_by_id, build_launch_command, build_launch_command_with_prompt,
    build_quick_launch_command, build_resume_command, default_model_id, deliver_prompt_via_tmux,
    is_workspace_script, looks_like_shell_command, model_index, AgentEntry, AGENTS,
};
pub use opencode::{
    assign_session_for_cwd, is_opencode_session_id, opencode_session_index, OpenCode,
};
pub use registry::providers_by_detection_priority;
pub use traits::hooks::{AgentHookReport, HookProvider};
pub use traits::launch::LaunchProvider;

static AGENT_ADAPTERS: &[&'static dyn AgentAdapter] = &[&Grok, &Codex, &Claude, &OpenCode];

static AGENT_ID_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, &'static str>>,
> = std::sync::OnceLock::new();

fn agent_id_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, &'static str>> {
    AGENT_ID_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Invalidate a cached agent-id entry so the next call re-probes disk.
/// Call this on `session_start` events, where on-disk files may not yet exist.
pub fn invalidate_agent_id_cache(sid: &str) {
    if let Ok(mut cache) = agent_id_cache().lock() {
        cache.remove(sid);
    }
}

pub fn list_agents() -> &'static [&'static dyn AgentAdapter] {
    AGENT_ADAPTERS
}

pub fn get_agent(id: &str) -> Option<&'static dyn AgentAdapter> {
    let id = id.trim().to_ascii_lowercase();
    AGENT_ADAPTERS
        .iter()
        .copied()
        .find(|agent| agent.id() == id)
}

pub fn agent_for_binary(binary: &str) -> Option<&'static dyn AgentAdapter> {
    let base = binary.rsplit('/').next().unwrap_or(binary);
    AGENT_ADAPTERS
        .iter()
        .copied()
        .find(|agent| agent.binary_matches(base))
}

/// Infer agent from on-disk thread data. Shared session env files under the state dir are
/// tmux bindings only — codex rollouts and grok summaries disambiguate the agent.
/// Result is cached for the process lifetime — session_id → agent never changes.
pub fn detect_agent_id_for_session(home: &std::path::Path, sid: &str) -> Option<&'static str> {
    if let Ok(cache) = agent_id_cache().lock() {
        if let Some(&cached) = cache.get(sid) {
            return Some(cached);
        }
    }

    let result = detect_agent_id_from_disk(home, sid);

    if let Some(id) = result {
        if let Ok(mut cache) = agent_id_cache().lock() {
            cache.entry(sid.to_string()).or_insert(id);
        }
    }

    result
}

fn detect_agent_id_from_disk(home: &std::path::Path, sid: &str) -> Option<&'static str> {
    providers_by_detection_priority().find_map(|provider| {
        provider
            .adapter
            .detect_session_on_disk(home, sid)
            .then_some(provider.id)
    })
}

pub fn agent_for_session_id(
    sid: &str,
    home: &std::path::Path,
) -> Option<&'static dyn AgentAdapter> {
    detect_agent_id_for_session(home, sid).and_then(get_agent)
}

/// Resolve agent for a session id from on-disk thread data.
pub fn infer_agent_for_session(
    home: &std::path::Path,
    cwd: &str,
    sid: &str,
) -> Option<&'static dyn AgentAdapter> {
    agent_for_session_id(sid, home).or_else(|| {
        for provider in providers_by_detection_priority() {
            if provider.id == "opencode" && is_opencode_session_id(sid) {
                return Some(provider.adapter);
            }
            if provider.id != "opencode" && provider.adapter.load_summary(home, cwd, sid).is_some()
            {
                return Some(provider.adapter);
            }
        }
        None
    })
}

pub fn resolve_session_id_from_env() -> Option<(String, &'static str)> {
    for agent in AGENT_ADAPTERS {
        if let Some(var) = agent.session_id_env_var() {
            if let Ok(sid) = std::env::var(var) {
                if !sid.is_empty() {
                    return Some((sid, agent.id()));
                }
            }
        }
    }
    None
}

pub fn detect_runtime_agent() -> Option<String> {
    if let Some((_, agent_id)) = resolve_session_id_from_env() {
        return Some(agent_id.to_string());
    }
    if codex::is_codex_env() {
        return Some("codex".into());
    }
    if claude::is_claude_env() {
        return Some("claude".into());
    }
    agent_from_process_tree()
}

fn agent_from_process_tree() -> Option<String> {
    let mut pid = std::process::id();
    for _ in 0..6 {
        let parent = parent_pid(pid)?;
        if parent == 0 || parent == pid {
            return None;
        }
        let command = command_for_pid(parent)?;
        let first = command.split_whitespace().next()?;
        if let Some(agent) = agent_for_binary(first) {
            return Some(agent.id().to_string());
        }
        pid = parent;
    }
    None
}

fn parent_pid(pid: u32) -> Option<u32> {
    process_output(&["-o", "ppid=", "-p", &pid.to_string()])?
        .parse()
        .ok()
}

fn command_for_pid(pid: u32) -> Option<String> {
    process_output(&["-o", "command=", "-p", &pid.to_string()])
}

fn process_output(args: &[&str]) -> Option<String> {
    crate::daemon::metrics::record_ps_call();
    let output = std::process::Command::new("ps").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

pub fn normalize_canonical_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.to_string()
    }
}

pub fn expand_canonical_cwd(home: &std::path::Path, cwd: &str) -> String {
    let cwd = normalize_canonical_cwd(cwd);
    if cwd == "~" {
        return home.display().to_string();
    }
    if let Some(rest) = cwd.strip_prefix("~/") {
        return format!("{}/{rest}", home.display());
    }
    cwd
}

pub fn same_canonical_cwd(home: &std::path::Path, a: &str, b: &str) -> bool {
    expand_canonical_cwd(home, a) == expand_canonical_cwd(home, b)
}

pub fn agent_session_cwd(home: &std::path::Path, sid: &str) -> Option<String> {
    if let Some(agent_id) = detect_agent_id_for_session(home, sid) {
        if let Some(adapter) = get_agent(agent_id) {
            return adapter.session_cwd(home, sid);
        }
    }
    providers_by_detection_priority().find_map(|provider| provider.adapter.session_cwd(home, sid))
}

/// True when `sid` is plausibly owned by `runtime_agent`.
pub fn agent_session_id_matches_runtime_agent(
    home: &std::path::Path,
    sid: &str,
    runtime_agent: Option<&str>,
) -> bool {
    let Some(runtime) = runtime_agent else {
        return true;
    };
    if runtime.is_empty() {
        return true;
    }
    let Some(detected) = detect_agent_id_for_session(home, sid) else {
        return true;
    };
    detected == runtime
}

/// True when on-disk evidence for `sid` is the same agent as `expected_agent`.
///
/// Unlike [`agent_session_id_matches_runtime_agent`], a known mismatch returns
/// false even when the expected agent is set — used to reject cross-agent
/// bindings on managed launches (e.g. a Grok UUID written into an OpenCode
/// managed record, which then groups the row under the wrong project).
pub fn agent_session_matches_expected_agent(
    home: &std::path::Path,
    sid: &str,
    expected_agent: &str,
) -> bool {
    let expected = expected_agent.trim().to_ascii_lowercase();
    if expected.is_empty() || expected == "console" {
        return true;
    }
    match detect_agent_id_for_session(home, sid) {
        Some(detected) => detected == expected.as_str(),
        // Unknown on disk yet (brand-new session) — allow; hooks bind before
        // the agent writes its first artifact.
        None => true,
    }
}

/// True when the agent thread's on-disk cwd matches the tmux pane cwd.
pub fn agent_session_matches_pane_cwd(home: &std::path::Path, pane_cwd: &str, sid: &str) -> bool {
    agent_session_cwd(home, sid)
        .is_none_or(|session_cwd| same_canonical_cwd(home, pane_cwd, &session_cwd))
}

/// Sidebar group key: agent thread project when bound, otherwise pane cwd.
pub fn group_cwd_for_session(
    home: &std::path::Path,
    pane_cwd: &str,
    agent_session_id: Option<&str>,
) -> String {
    let group_cwd = agent_session_id
        .and_then(|sid| agent_session_cwd(home, sid))
        .unwrap_or_else(|| pane_cwd.to_string());
    crate::pty::format_tilde_path(&group_cwd, home)
}

pub fn disk_lookup_cwd(
    home: &std::path::Path,
    pane_cwd: &str,
    agent_session_id: Option<&str>,
) -> String {
    agent_session_id
        .and_then(|sid| agent_session_cwd(home, sid))
        .unwrap_or_else(|| pane_cwd.to_string())
}

pub fn load_session_summary(
    home: &std::path::Path,
    cwd: &str,
    sid: &str,
) -> Option<(SessionSummary, &'static str)> {
    providers_by_detection_priority().find_map(|provider| {
        provider
            .adapter
            .load_summary(home, cwd, sid)
            .map(|summary| (summary, provider.id))
    })
}

pub fn thread_title_from_summary(summary: &SessionSummary, agent: &str) -> Option<String> {
    let adapter = get_agent(agent).unwrap_or(&Grok as &dyn AgentAdapter);
    adapter.thread_title_from_summary(summary)
}

/// Parent Grok thread id when `session_id` is a Task/subagent child.
pub fn parent_session_id_for_subagent(home: &std::path::Path, session_id: &str) -> Option<String> {
    crate::agents::grok::parent_session_id_for_subagent(home, session_id)
}

pub fn is_subagent_of(home: &std::path::Path, child_id: &str, parent_id: &str) -> bool {
    crate::agents::grok::is_subagent_of(home, child_id, parent_id)
}

/// True once the agent thread has a real user turn on disk (`turn_started` / equivalent).
pub fn session_has_commenced(home: &std::path::Path, cwd: &str, sid: &str) -> bool {
    let lookup_cwd = disk_lookup_cwd(home, cwd, Some(sid));
    session_messaged_at(home, &lookup_cwd, sid).is_some()
}

pub fn session_messaged_at(
    home: &std::path::Path,
    _cwd: &str,
    sid: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Some(agent_id) = detect_agent_id_for_session(home, sid) {
        if let Some(adapter) = get_agent(agent_id) {
            return adapter.messaged_at(home, sid);
        }
    }
    providers_by_detection_priority().find_map(|provider| provider.adapter.messaged_at(home, sid))
}

pub fn session_activity_at(
    home: &std::path::Path,
    _cwd: &str,
    sid: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Some(agent_id) = detect_agent_id_for_session(home, sid) {
        if let Some(adapter) = get_agent(agent_id) {
            return adapter.activity_at(home, sid);
        }
    }
    providers_by_detection_priority().find_map(|provider| provider.adapter.activity_at(home, sid))
}

pub fn is_agent_app(app: &str) -> bool {
    get_agent(app).is_some()
}

pub fn agent_from_command_name(name: &str) -> Option<String> {
    agent_for_binary(name).map(|agent| agent.id().to_string())
}

pub fn agent_from_command(command: &str) -> Option<String> {
    let command = crate::pty::normalize_workspace_command(command);
    let first = command.split_whitespace().next()?;
    agent_from_command_name(first)
}

pub fn ensure_classify_registry() {
    crate::pty::ensure_app_registry();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_for_binary_matches_versioned_binaries() {
        assert_eq!(
            agent_for_binary("grok-0.2.39-mac").map(|a| a.id()),
            Some("grok")
        );
        assert_eq!(
            agent_for_binary("codex-aarch64-a").map(|a| a.id()),
            Some("codex")
        );
        assert_eq!(
            agent_for_binary("opencode").map(|a| a.id()),
            Some("opencode")
        );
        assert!(agent_for_binary("htop").is_none());
    }

    #[test]
    fn opencode_classify_and_lifecycle_without_hooks() {
        crate::pty::ensure_app_registry();
        let kind = crate::pty::classify_pane(
            "opencode",
            "opencode refactor-auth",
            env!("CARGO_MANIFEST_DIR"),
        );
        match kind {
            crate::pty::PaneKind::Tool { app, thread, .. } => {
                assert_eq!(app, "opencode");
                assert_eq!(thread, "refactor-auth");
            }
            _ => panic!("expected tool pane"),
        }
        assert_eq!(
            crate::pty::infer_pane_state("opencode", false, None),
            crate::model::AgentState::Working
        );
        assert_eq!(
            crate::pty::infer_pane_state("opencode", true, Some(0)),
            crate::model::AgentState::Done
        );
    }

    #[test]
    fn grok_summary_and_events_still_work_after_extract() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");
        let sid = "019ea057-3abe-74e2-b130-2f01c3dd1988";
        let events_dir = crate::agents::grok::session_dir(home, cwd, sid);
        std::fs::create_dir_all(&events_dir).unwrap();
        std::fs::write(
            events_dir.join("events.jsonl"),
            r#"{"ts":"2026-06-07T04:38:45.548Z","type":"turn_started"}
{"ts":"2026-06-07T04:40:09.787Z","type":"phase_changed","phase":"streaming_reasoning"}
"#,
        )
        .unwrap();
        let agent = infer_agent_for_session(home, cwd, sid).unwrap();
        assert_eq!(agent.id(), "grok");
        let activity = AgentAdapter::live_activity(&Grok, home, cwd, sid).unwrap();
        assert_eq!(activity.state, crate::model::AgentState::Working);
    }

    #[test]
    fn codex_session_resolves_from_codex_thread_id_env() {
        std::env::set_var("CODEX_THREAD_ID", "thread-abc");
        let resolved = resolve_session_id_from_env();
        std::env::remove_var("CODEX_THREAD_ID");
        assert_eq!(
            resolved.map(|(sid, agent)| (sid, agent)),
            Some(("thread-abc".into(), "codex"))
        );
    }

    #[test]
    fn detect_agent_id_recognizes_claude_project_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let sid = "06b67c89-bd76-4922-b6ec-518172be4267";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let project_dir = claude::claude_home(home)
            .join("projects")
            .join(claude::encode_claude_project_dir(cwd));
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join(format!("{sid}.jsonl")),
            format!(
                r#"{{"type":"user","message":{{"role":"user","content":"build claude sidebar support"}},"cwd":"{cwd}","timestamp":"2026-06-11T06:38:24.372Z"}}
{{"type":"ai-title","aiTitle":"Claude sidebar integration","sessionId":"{sid}"}}"#
            ),
        )
        .unwrap();

        assert_eq!(detect_agent_id_for_session(home, sid), Some("claude"));
        let (summary, agent) = load_session_summary(home, cwd, sid).unwrap();
        assert_eq!(agent, "claude");
        assert_eq!(
            summary.generated_title.as_deref(),
            Some("Claude sidebar integration")
        );
        assert_eq!(
            thread_title_from_summary(&summary, agent).as_deref(),
            Some("Claude sidebar integration")
        );
    }

    #[test]
    fn detect_agent_id_prefers_codex_rollout_over_shared_env_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let sid = "019eb0bb-3711-72d2-a80c-15259d6349e4";
        let cwd = env!("CARGO_MANIFEST_DIR");
        std::fs::create_dir_all(crate::paths::state_dir(home)).unwrap();
        std::fs::write(
            crate::paths::state_dir(home).join(format!("{sid}.env")),
            "TMUX_PANE=%513\nSESSIONS_WINDOW_INDEX=37\n",
        )
        .unwrap();
        let rollout_dir = home.join(".codex/sessions/2026/06/10");
        std::fs::create_dir_all(&rollout_dir).unwrap();
        std::fs::write(
            rollout_dir.join(format!("rollout-2026-06-10T18-21-59-{sid}.jsonl")),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"019eb0bb-3711-72d2-a80c-15259d6349e4","cwd":"{cwd}"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"fix codex sidebar titles"}}}}"#
            ),
        )
        .unwrap();

        assert_eq!(detect_agent_id_for_session(home, sid), Some("codex"));
        let (summary, agent) = load_session_summary(home, env!("CARGO_MANIFEST_DIR"), sid).unwrap();
        assert_eq!(agent, "codex");
        assert_eq!(
            summary.generated_title.as_deref(),
            Some("fix codex sidebar titles")
        );
    }

    #[test]
    fn group_cwd_for_session_uses_agent_project_not_pane() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let sid = "019ea671-54cc-7fb0-91e4-2a567b4ce022";
        let sessions_cwd = env!("CARGO_MANIFEST_DIR");
        let acme_cwd = "/home/testuser/projects/acme";
        let summary_dir = grok::session_dir(home, sessions_cwd, sid);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            format!(r#"{{"info":{{"cwd":"{sessions_cwd}"}}}}"#),
        )
        .unwrap();

        assert_eq!(
            group_cwd_for_session(home, acme_cwd, Some(sid)),
            sessions_cwd
        );
        assert_eq!(group_cwd_for_session(home, acme_cwd, None), acme_cwd);
    }

    #[test]
    fn agent_session_matches_expected_agent_rejects_cross_agent() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let sid = "019ea671-54cc-7fb0-91e4-2a567b4ce099";
        let sessions_cwd = env!("CARGO_MANIFEST_DIR");
        let summary_dir = grok::session_dir(home, sessions_cwd, sid);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            format!(r#"{{"info":{{"cwd":"{sessions_cwd}"}}}}"#),
        )
        .unwrap();

        assert!(agent_session_matches_expected_agent(home, sid, "grok"));
        assert!(!agent_session_matches_expected_agent(home, sid, "opencode"));
        // Unknown SID: allow (hooks bind before disk artifacts exist).
        assert!(agent_session_matches_expected_agent(
            home,
            "ses_brand_new_unknown",
            "opencode"
        ));
    }

    #[test]
    fn agent_session_matches_pane_cwd_uses_summary_project() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let sid = "019ea6d5-31c4-7260-92a9-de3122c6b0f5";
        let sessions_cwd = env!("CARGO_MANIFEST_DIR");
        let acme_cwd = "/home/testuser/projects/acme";
        let summary_dir = grok::session_dir(home, sessions_cwd, sid);
        std::fs::create_dir_all(&summary_dir).unwrap();
        std::fs::write(
            summary_dir.join("summary.json"),
            format!(
                r#"{{"generated_title":"Bridge sessions-cli","info":{{"cwd":"{sessions_cwd}"}}}}"#
            ),
        )
        .unwrap();
        let state_dir = crate::paths::state_dir(home);
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join(format!("{sid}.env")),
            "TMUX_PANE=%389\nSESSIONS_WINDOW_INDEX=3\n",
        )
        .unwrap();

        assert!(agent_session_matches_pane_cwd(home, sessions_cwd, sid));
        assert!(!agent_session_matches_pane_cwd(home, acme_cwd, sid));
        assert_eq!(
            grok::load_session_summary(home, acme_cwd, sid)
                .and_then(|summary| grok::thread_title_from_summary(&summary)),
            Some("Bridge sessions-cli".into())
        );
    }

    #[test]
    fn session_has_commenced_requires_turn_started() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let sid = "019eb477-0000-7000-8000-000000000001";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let session_dir = grok::session_dir(home, cwd, sid);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("summary.json"),
            r#"{"generated_title":"Should not show yet"}"#,
        )
        .unwrap();
        assert!(!session_has_commenced(home, cwd, sid));

        std::fs::write(
            session_dir.join("events.jsonl"),
            r#"{"ts":"2026-06-11T12:00:00.000Z","type":"turn_started"}"#,
        )
        .unwrap();
        assert!(session_has_commenced(home, cwd, sid));
    }

    #[test]
    fn session_has_commenced_finds_turn_started_beyond_tail_window() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let sid = "019eb477-0000-7000-8000-000000000099";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let session_dir = grok::session_dir(home, cwd, sid);
        std::fs::create_dir_all(&session_dir).unwrap();
        let mut events = String::from(r#"{"ts":"2026-06-11T12:00:00.000Z","type":"turn_started"}"#);
        events.push('\n');
        events.push_str(&format!(
            r#"{{"ts":"2026-06-11T12:01:00.000Z","type":"phase_changed","phase":"streaming_text"}}"#,
        ));
        events.push('\n');
        for _ in 0..8_000 {
            events.push_str(
                r#"{"ts":"2026-06-11T12:02:00.000Z","type":"phase_changed","phase":"tool_execution"}"#,
            );
            events.push('\n');
        }
        std::fs::write(session_dir.join("events.jsonl"), events).unwrap();
        assert!(session_has_commenced(home, cwd, sid));
    }

    #[test]
    fn resolve_session_names_hides_summary_until_commenced() {
        use crate::pty::naming::resolve_session_names;

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let sid = "019eb477-0000-7000-8000-000000000002";
        let cwd = env!("CARGO_MANIFEST_DIR");
        let session_dir = grok::session_dir(home, cwd, sid);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("summary.json"),
            r#"{"generated_title":"Premature Grok title"}"#,
        )
        .unwrap();

        let (title, thread, app) =
            resolve_session_names(home, cwd, Some("grok"), Some(sid), "", "", "", None, false);
        assert_eq!(app, "grok");
        assert_eq!(thread, "?");
        assert_eq!(title, "grok · ?");
    }
}
