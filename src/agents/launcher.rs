use super::registry::{launch_provider_by_id, provider_by_id};
use super::traits::ModelOption;
use super::{detect_agent_id_for_session, infer_agent_for_session};
use crate::session::manifest::ManifestEntry;
use std::path::Path;

pub struct AgentEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub default_model: &'static str,
    pub models: &'static [ModelOption],
}

pub const AGENTS: &[AgentEntry] = &[
    AgentEntry {
        id: "grok",
        label: "Grok",
        default_model: "grok-4.5",
        models: &[
            // Keep in sync with GrokLaunch / `grok models`.
            ModelOption {
                id: "grok-4.5",
                label: "Grok 4.5",
            },
            ModelOption {
                id: "grok-composer-2.5-fast",
                label: "Composer 2.5",
            },
        ],
    },
    AgentEntry {
        id: "codex",
        label: "Codex",
        default_model: "gpt-5.4",
        models: &[
            ModelOption {
                id: "gpt-5.4",
                label: "GPT-5.4",
            },
            ModelOption {
                id: "gpt-5.5",
                label: "GPT-5.5",
            },
            ModelOption {
                id: "gpt-5.3-codex",
                label: "GPT-5.3 Codex",
            },
            ModelOption {
                id: "o3",
                label: "o3",
            },
        ],
    },
    AgentEntry {
        id: "claude",
        label: "Claude",
        default_model: "sonnet",
        models: &[
            ModelOption {
                id: "sonnet",
                label: "Sonnet",
            },
            ModelOption {
                id: "opus",
                label: "Opus",
            },
            ModelOption {
                id: "haiku",
                label: "Haiku",
            },
        ],
    },
    AgentEntry {
        id: "opencode",
        label: "OpenCode",
        default_model: "default",
        models: &[ModelOption {
            id: "default",
            label: "Default",
        }],
    },
    AgentEntry {
        id: "console",
        label: "None (console)",
        default_model: "shell",
        models: &[ModelOption {
            id: "shell",
            label: "Terminal",
        }],
    },
];

pub fn agent_by_id(id: &str) -> Option<&'static AgentEntry> {
    AGENTS.iter().find(|agent| agent.id == id)
}

pub fn default_model_id(agent_id: &str) -> &'static str {
    agent_by_id(agent_id)
        .map(|agent| agent.default_model)
        .unwrap_or("grok-4.5")
}

pub fn model_index(agent: &AgentEntry, model_id: &str) -> usize {
    agent
        .models
        .iter()
        .position(|model| model.id == model_id)
        .unwrap_or(0)
}

pub fn model_label(agent: &AgentEntry, model_id: &str) -> String {
    agent
        .models
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| model.label.to_string())
        .unwrap_or_else(|| model_id.to_string())
}

/// Launch command for keyboard shortcuts — matches workspaces.toml (plain agent binary).
pub fn build_quick_launch_command(agent_id: &str) -> String {
    if let Some(launch) = launch_provider_by_id(agent_id) {
        return launch.build_quick_launch_command();
    }
    match agent_id {
        "console" => String::new(),
        other => build_launch_command(other, default_model_id(other)),
    }
}

/// Agents whose CLIs accept an initial prompt as a positional argument.
pub fn agent_accepts_cli_prompt(agent_id: &str) -> bool {
    launch_provider_by_id(agent_id)
        .map(|launch| launch.accepts_cli_prompt())
        .unwrap_or(false)
}

/// tmux rejects `new-window` shell commands beyond this size ("command too long").
pub const TMUX_SHELL_COMMAND_MAX_BYTES: usize = 16_000;

/// Full `zsh -lc` command that would be passed to `tmux new-window`.
pub fn launch_shell_command(
    agent_id: &str,
    model_id: &str,
    cwd: &str,
    prompt: Option<&str>,
) -> String {
    let launch = build_launch_command_with_prompt(agent_id, model_id, prompt);
    let wrapped = crate::session::wrap_managed_launch_command(
        agent_id,
        cwd,
        "00000000-0000-0000-0000-000000000001",
        &launch,
    );
    format!("{wrapped} || exec /bin/zsh -l")
}

/// Whether a non-empty prompt should be typed into the pane after the agent starts.
pub fn deliver_prompt_via_tmux(agent_id: &str, model_id: &str, cwd: &str, prompt: &str) -> bool {
    if !agent_accepts_cli_prompt(agent_id) {
        return true;
    }
    launch_shell_command(agent_id, model_id, cwd, Some(prompt)).len() > TMUX_SHELL_COMMAND_MAX_BYTES
}

/// Shell command launched in a new tmux window (before the `|| exec zsh` fallback).
pub fn build_launch_command(agent_id: &str, model_id: &str) -> String {
    build_launch_command_with_prompt(agent_id, model_id, None)
}

/// Resume command for cold-boot restore — uses agent UUID, never `ssn_*`.
pub fn build_resume_command(
    agent_id: &str,
    launch_command: &str,
    agent_session_id: &str,
) -> String {
    let model_hint = parse_model_from_launch_command(launch_command);
    if let Some(launch) = launch_provider_by_id(agent_id) {
        return launch.build_resume_command(model_hint.as_deref(), agent_session_id);
    }
    agent_id.to_string()
}

/// Resolve the launch shell command for manifest restore.
///
/// Resume requires both `agent_session_id` (agent UUID, never `ssn_*`) and an on-disk
/// thread for that id. Workspace bootstrap entries that store a bare agent binary or a
/// `--resume` flag in `launch_command` still quick-launch when `agent_session_id` is
/// absent or the on-disk thread is gone.
pub fn effective_restore_command(entry: &ManifestEntry) -> String {
    effective_restore_command_at(&crate::paths::home(), entry)
}

pub(crate) fn effective_restore_command_at(home: &Path, entry: &ManifestEntry) -> String {
    // Resume-first: down-sync often stores agent as "console" while agent_session_id
    // and title still refer to a live grok/codex thread on disk.
    if let Some(agent_session_id) = entry.agent_session_id.as_deref() {
        if let Some(agent) = resolve_restore_agent(home, entry, agent_session_id) {
            if agent_session_exists_at(home, &agent, agent_session_id) {
                return build_resume_command(&agent, &entry.launch_command, agent_session_id);
            }
        }
    }

    if entry.agent == "console" || is_workspace_script(&entry.launch_command) {
        return normalized_launch_command(entry);
    }

    if let Some(agent_session_id) = entry.agent_session_id.as_deref() {
        if agent_session_exists_at(home, &entry.agent, agent_session_id) {
            return build_resume_command(&entry.agent, &entry.launch_command, agent_session_id);
        }
    }

    if entry.launch_command.trim().is_empty() {
        return build_quick_launch_command(&entry.agent);
    }

    if looks_like_shell_command(&entry.launch_command)
        && !contains_prompt(&entry.launch_command)
        && super::agent_from_command(&entry.launch_command).is_none()
    {
        return entry.launch_command.clone();
    }

    build_quick_launch_command(&entry.agent)
}

pub fn normalized_launch_command(entry: &ManifestEntry) -> String {
    entry.launch_command.trim().to_string()
}

pub fn is_workspace_script(launch_command: &str) -> bool {
    let command = crate::pty::naming::normalize_workspace_command(launch_command);
    !command.is_empty()
        && !crate::pty::naming::is_shell_command(&command)
        && super::agent_from_command(&command).is_none()
}

pub fn looks_like_shell_command(launch_command: &str) -> bool {
    let command = crate::pty::naming::normalize_workspace_command(launch_command);
    if command.is_empty() {
        return true;
    }
    if command.contains(" · ") {
        return false;
    }
    if crate::pty::naming::is_shell_command(&command) {
        return true;
    }
    if super::agent_from_command(&command).is_some() {
        return true;
    }
    is_workspace_script(&command)
}

pub fn contains_prompt(launch_command: &str) -> bool {
    let command = crate::pty::naming::normalize_workspace_command(launch_command);
    if command.is_empty() {
        return false;
    }
    if flag_value(&command, "--prompt").is_some() {
        return true;
    }
    if command.contains('\'') || command.contains('"') {
        return true;
    }
    if super::agent_from_command(&command).is_some() && has_positional_prompt(&command) {
        return true;
    }
    false
}

pub fn parse_model_from_launch_command(launch_command: &str) -> Option<String> {
    let command = crate::pty::naming::normalize_workspace_command(launch_command);
    if let Some(model) = flag_value(&command, "--model") {
        return Some(model);
    }
    flag_value(&command, "-m")
}

pub fn agent_session_exists(agent: &str, agent_session_id: &str) -> bool {
    agent_session_exists_at(&crate::paths::home(), agent, agent_session_id)
}

pub(crate) fn resolve_restore_agent(
    home: &Path,
    entry: &ManifestEntry,
    agent_session_id: &str,
) -> Option<String> {
    if !entry.agent.is_empty() && entry.agent != "console" && entry.agent != "session" {
        return Some(entry.agent.clone());
    }
    if let Some(agent) = detect_agent_id_for_session(home, agent_session_id) {
        return Some(agent.to_string());
    }
    if infer_agent_for_session(home, &entry.cwd, agent_session_id).is_some() {
        return detect_agent_id_for_session(home, agent_session_id).map(str::to_string);
    }
    entry.title.as_deref().and_then(|title| {
        crate::pty::parse_app(title)
            .filter(|app| crate::pty::is_agent_app(app))
            .map(|app| app.to_string())
    })
}

fn agent_session_exists_at(home: &Path, agent: &str, agent_session_id: &str) -> bool {
    provider_by_id(agent)
        .map(|provider| {
            provider
                .adapter
                .detect_session_on_disk(home, agent_session_id)
        })
        .unwrap_or(false)
}

fn flag_value(command: &str, flag: &str) -> Option<String> {
    let tokens = shell_tokens(command);
    let mut index = 0;
    while index < tokens.len() {
        if let Some(value) = tokens[index].strip_prefix(&format!("{flag}=")) {
            return Some(unquote_token(value));
        }
        if tokens[index] == flag {
            index += 1;
            if index < tokens.len() {
                return Some(unquote_token(&tokens[index]));
            }
            return None;
        }
        index += 1;
    }
    None
}

fn has_positional_prompt(command: &str) -> bool {
    let mut tokens = shell_tokens(command);
    if tokens.len() <= 1 {
        return false;
    }
    tokens.remove(0);
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if token == "--" {
            index += 1;
            break;
        }
        if token.starts_with('-') {
            index += 1;
            if needs_flag_value(token) && index < tokens.len() && !tokens[index].starts_with('-') {
                index += 1;
            }
            continue;
        }
        return true;
    }
    index < tokens.len()
}

fn needs_flag_value(token: &str) -> bool {
    !matches!(
        token,
        "--continue"
            | "--fork"
            | "--pure"
            | "--print-logs"
            | "-c"
            | "-h"
            | "--help"
            | "-v"
            | "--version"
    )
}

fn shell_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match quote {
            Some(delimiter) => {
                if ch == delimiter {
                    quote = None;
                } else if ch == '\\' && delimiter == '\'' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
                continue;
            }
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(unquote_token(&current));
                    current.clear();
                }
            }
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(unquote_token(&current));
    }
    tokens
}

fn unquote_token(token: &str) -> String {
    if (token.starts_with('\'') && token.ends_with('\'') && token.len() >= 2)
        || (token.starts_with('"') && token.ends_with('"') && token.len() >= 2)
    {
        token[1..token.len() - 1].replace("\\'", "'")
    } else {
        token.to_string()
    }
}

/// Like [`build_launch_command`], but passes a non-empty prompt on the agent CLI when supported.
pub fn build_launch_command_with_prompt(
    agent_id: &str,
    model_id: &str,
    prompt: Option<&str>,
) -> String {
    let prompt = prompt.filter(|text| !text.trim().is_empty());
    if agent_id == "console" {
        return String::new();
    }
    if let Some(launch) = launch_provider_by_id(agent_id) {
        let prompt_arg = prompt.filter(|_| launch.accepts_cli_prompt());
        return launch.build_command(model_id, prompt_arg);
    }
    agent_id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::registry::PROVIDERS;

    #[test]
    fn providers_and_agents_cover_same_launch_metadata() {
        for provider in PROVIDERS {
            let launch = provider.launch;
            let entry = agent_by_id(provider.id).expect("provider missing AGENTS shim entry");
            assert_eq!(entry.id, launch.id());
            assert_eq!(entry.label, launch.label());
            assert_eq!(entry.default_model, launch.default_model());
            assert_eq!(entry.models.len(), launch.models().len());
            for (left, right) in entry.models.iter().zip(launch.models().iter()) {
                assert_eq!(left.id, right.id);
                assert_eq!(left.label, right.label);
            }
        }
    }

    #[test]
    fn each_agent_has_models_and_default() {
        for agent in AGENTS {
            assert!(!agent.models.is_empty());
            assert!(agent.models.iter().any(|m| m.id == agent.default_model));
        }
    }

    #[test]
    fn build_quick_launch_command_uses_plain_agent_binary() {
        assert_eq!(build_quick_launch_command("grok"), "grok");
        assert_eq!(build_quick_launch_command("codex"), "codex");
        assert_eq!(build_quick_launch_command("opencode"), "opencode");
        assert_eq!(build_quick_launch_command("console"), "");
    }

    #[test]
    fn build_launch_command_includes_model_flag() {
        assert_eq!(
            build_launch_command("grok", "grok-composer-2.5-fast"),
            "grok --model grok-composer-2.5-fast"
        );
        assert_eq!(
            build_launch_command("codex", "gpt-5.4"),
            "codex --model gpt-5.4"
        );
        assert_eq!(build_launch_command("opencode", "default"), "opencode");
        assert_eq!(build_launch_command("console", "shell"), "");
    }

    #[test]
    fn build_launch_command_with_prompt_uses_cli_argument_for_grok() {
        assert_eq!(
            build_launch_command_with_prompt("grok", "grok-build", Some("fix the bug")),
            "grok --model grok-build 'fix the bug'"
        );
        assert_eq!(
            build_launch_command_with_prompt(
                "grok",
                "grok-build",
                Some("it's broken\nplease help")
            ),
            "grok --model grok-build 'it'\\''s broken\nplease help'"
        );
        assert_eq!(
            build_launch_command_with_prompt("opencode", "default", Some("a task")),
            "opencode --prompt 'a task'"
        );
        assert_eq!(
            build_launch_command_with_prompt("console", "shell", Some("ignored")),
            ""
        );
    }

    #[test]
    fn model_index_falls_back_to_zero() {
        let grok = agent_by_id("grok").unwrap();
        assert_eq!(model_index(grok, "missing"), 0);
        assert_eq!(model_index(grok, "grok-4.5"), 0);
        assert_eq!(model_index(grok, "grok-composer-2.5-fast"), 1);
    }

    #[test]
    fn deliver_prompt_via_tmux_for_opencode_and_oversized_shell_commands() {
        let cwd = env!("CARGO_MANIFEST_DIR");
        assert!(!deliver_prompt_via_tmux("opencode", "default", cwd, "task"));
        assert!(!deliver_prompt_via_tmux("grok", "grok-build", cwd, "short"));
        let long = "x".repeat(TMUX_SHELL_COMMAND_MAX_BYTES);
        assert!(deliver_prompt_via_tmux("grok", "grok-build", cwd, &long));
        let quoted = "don't ".repeat(2_000);
        assert!(deliver_prompt_via_tmux("grok", "grok-build", cwd, &quoted));
    }

    const GROK_AGENT_SESSION_ID: &str = "019ef8c8-a1b2-73c4-d5e6-f78901234567";
    const CODEX_AGENT_SESSION_ID: &str = "019ef8c8-b1b2-73c4-d5e6-f78901234568";
    const CLAUDE_AGENT_SESSION_ID: &str = "06b67c89-bd76-4922-b6ec-518172be4267";
    const OPENCODE_AGENT_SESSION_ID: &str = "ses_14c367547ffe7g1N1inGGONKuZ";

    fn manifest_entry(
        agent: &str,
        launch_command: &str,
        agent_session_id: Option<&str>,
    ) -> ManifestEntry {
        use crate::session::manifest::ManifestSource;

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

    #[test]
    fn build_resume_command_matches_live_cli_syntax() {
        assert_eq!(
            build_resume_command("grok", "grok --model grok-build", GROK_AGENT_SESSION_ID),
            format!("grok --model grok-build --resume {GROK_AGENT_SESSION_ID}")
        );
        assert_eq!(
            build_resume_command("grok", "grok", GROK_AGENT_SESSION_ID),
            format!("grok --resume {GROK_AGENT_SESSION_ID}")
        );
        assert_eq!(
            build_resume_command("codex", "codex --model gpt-5.4", GROK_AGENT_SESSION_ID),
            format!("codex resume --model gpt-5.4 {GROK_AGENT_SESSION_ID}")
        );
        assert_eq!(
            build_resume_command("codex", "codex", GROK_AGENT_SESSION_ID),
            format!("codex resume {GROK_AGENT_SESSION_ID}")
        );
        assert_eq!(
            build_resume_command("claude", "claude --model sonnet", GROK_AGENT_SESSION_ID),
            format!("claude --model sonnet --resume {GROK_AGENT_SESSION_ID}")
        );
        assert_eq!(
            build_resume_command("claude", "claude", GROK_AGENT_SESSION_ID),
            format!("claude --resume {GROK_AGENT_SESSION_ID}")
        );
        assert_eq!(
            build_resume_command(
                "opencode",
                "opencode -m anthropic/claude-sonnet",
                OPENCODE_AGENT_SESSION_ID
            ),
            format!(
                "opencode --model 'anthropic/claude-sonnet' --session {OPENCODE_AGENT_SESSION_ID}"
            )
        );
        assert_eq!(
            build_resume_command("opencode", "opencode", OPENCODE_AGENT_SESSION_ID),
            format!("opencode --session {OPENCODE_AGENT_SESSION_ID}")
        );
    }

    #[test]
    fn parse_model_from_launch_command_reads_model_flags() {
        assert_eq!(
            parse_model_from_launch_command("grok --model grok-build 'fix the bug'").as_deref(),
            Some("grok-build")
        );
        assert_eq!(
            parse_model_from_launch_command("codex --model=gpt-5.4").as_deref(),
            Some("gpt-5.4")
        );
        assert_eq!(
            parse_model_from_launch_command("opencode -m anthropic/claude-sonnet").as_deref(),
            Some("anthropic/claude-sonnet")
        );
        assert!(parse_model_from_launch_command("grok").is_none());
    }

    #[test]
    fn helper_classification_for_shell_workspace_and_prompts() {
        assert!(looks_like_shell_command("grok --model grok-build"));
        assert!(looks_like_shell_command("./run-local.sh"));
        assert!(!looks_like_shell_command("grok · sticky title"));

        assert!(is_workspace_script("./run-local.sh"));
        assert!(!is_workspace_script("grok"));
        assert!(!is_workspace_script(""));

        assert!(contains_prompt("grok --model grok-build 'fix the bug'"));
        assert!(contains_prompt("opencode --prompt 'a task'"));
        assert!(!contains_prompt("grok --model grok-build"));
        assert!(!contains_prompt("./run-local.sh arg"));
    }

    #[test]
    fn effective_restore_command_quick_launch_without_agent_session_id() {
        let entry = manifest_entry("grok", "grok --model grok-build", None);
        assert_eq!(effective_restore_command(&entry), "grok");
    }

    #[test]
    fn effective_restore_command_console_and_workspace_scripts_are_verbatim() {
        let console = manifest_entry("console", "", None);
        assert_eq!(effective_restore_command(&console), "");

        let script = manifest_entry("console", "./run-local.sh", None);
        assert_eq!(effective_restore_command(&script), "./run-local.sh");

        let workspace = manifest_entry("grok", "./scripts/dev.sh", None);
        assert_eq!(effective_restore_command(&workspace), "./scripts/dev.sh");
    }

    #[test]
    fn effective_restore_command_resumes_when_grok_session_exists_on_disk() {
        let dir = tempfile::tempdir().unwrap();
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
            "grok --model grok-build",
            Some(GROK_AGENT_SESSION_ID),
        );
        assert_eq!(
            effective_restore_command_at(home, &entry),
            format!("grok --model grok-build --resume {GROK_AGENT_SESSION_ID}")
        );
    }

    #[test]
    fn effective_restore_command_resumes_when_codex_session_exists_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");

        let rollout_dir = home.join(".codex/sessions/2026/06/10");
        std::fs::create_dir_all(&rollout_dir).unwrap();
        std::fs::write(
            rollout_dir.join(format!(
                "rollout-2026-06-10T18-21-59-{CODEX_AGENT_SESSION_ID}.jsonl"
            )),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{CODEX_AGENT_SESSION_ID}","cwd":"{cwd}"}}}}"#
            ),
        )
        .unwrap();

        let entry = manifest_entry(
            "codex",
            "codex --model gpt-5.4",
            Some(CODEX_AGENT_SESSION_ID),
        );
        assert_eq!(
            effective_restore_command_at(home, &entry),
            format!("codex resume --model gpt-5.4 {CODEX_AGENT_SESSION_ID}")
        );
    }

    #[test]
    fn effective_restore_command_resumes_when_claude_session_exists_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");

        let project_dir = crate::agents::claude::claude_home(home)
            .join("projects")
            .join(crate::agents::claude::encode_claude_project_dir(cwd));
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join(format!("{CLAUDE_AGENT_SESSION_ID}.jsonl")),
            format!(
                r#"{{"type":"user","message":{{"role":"user","content":"resume test"}},"cwd":"{cwd}","timestamp":"2026-06-11T06:38:24.372Z"}}"#
            ),
        )
        .unwrap();

        let entry = manifest_entry(
            "claude",
            "claude --model sonnet",
            Some(CLAUDE_AGENT_SESSION_ID),
        );
        assert_eq!(
            effective_restore_command_at(home, &entry),
            format!("claude --model sonnet --resume {CLAUDE_AGENT_SESSION_ID}")
        );
    }

    #[test]
    fn effective_restore_command_falls_back_when_agent_session_is_stale() {
        let entry = manifest_entry(
            "grok",
            "grok --model grok-build",
            Some("019ef8c8-0000-7000-8000-000000000099"),
        );
        assert_eq!(effective_restore_command(&entry), "grok");
    }

    #[test]
    fn effective_restore_command_resumes_console_manifest_row_with_grok_thread() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");

        let session_dir = crate::agents::grok::session_dir(home, cwd, GROK_AGENT_SESSION_ID);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("summary.json"),
            r#"{"generated_title":"test"}"#,
        )
        .unwrap();

        let mut entry = manifest_entry("console", "", Some(GROK_AGENT_SESSION_ID));
        entry.title = Some("grok · resume after down-sync".into());
        entry.cwd = cwd.into();
        assert_eq!(
            effective_restore_command_at(home, &entry),
            format!("grok --resume {GROK_AGENT_SESSION_ID}")
        );
    }

    #[test]
    fn effective_restore_command_resumes_when_opencode_session_exists_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");

        seed_opencode_session_on_disk(home, cwd, OPENCODE_AGENT_SESSION_ID);

        let entry = manifest_entry("opencode", "opencode", Some(OPENCODE_AGENT_SESSION_ID));
        assert_eq!(
            effective_restore_command_at(home, &entry),
            format!("opencode --session {OPENCODE_AGENT_SESSION_ID}")
        );
    }

    #[test]
    fn effective_restore_command_resumes_console_manifest_row_with_codex_thread() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");

        let rollout_dir = home.join(".codex/sessions/2026/06/10");
        std::fs::create_dir_all(&rollout_dir).unwrap();
        std::fs::write(
            rollout_dir.join(format!(
                "rollout-2026-06-10T18-21-59-{CODEX_AGENT_SESSION_ID}.jsonl"
            )),
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{CODEX_AGENT_SESSION_ID}","cwd":"{cwd}"}}}}"#
            ),
        )
        .unwrap();

        let mut entry = manifest_entry("console", "", Some(CODEX_AGENT_SESSION_ID));
        entry.title = Some("codex · resume after down-sync".into());
        entry.cwd = cwd.into();
        assert_eq!(
            effective_restore_command_at(home, &entry),
            format!("codex resume {CODEX_AGENT_SESSION_ID}")
        );
    }

    #[test]
    fn effective_restore_command_resumes_console_manifest_row_with_claude_thread() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");

        let project_dir = crate::agents::claude::claude_home(home)
            .join("projects")
            .join(crate::agents::claude::encode_claude_project_dir(cwd));
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join(format!("{CLAUDE_AGENT_SESSION_ID}.jsonl")),
            format!(
                r#"{{"type":"user","message":{{"role":"user","content":"resume test"}},"cwd":"{cwd}","timestamp":"2026-06-11T06:38:24.372Z"}}"#
            ),
        )
        .unwrap();

        let mut entry = manifest_entry("console", "", Some(CLAUDE_AGENT_SESSION_ID));
        entry.title = Some("claude · resume after down-sync".into());
        entry.cwd = cwd.into();
        assert_eq!(
            effective_restore_command_at(home, &entry),
            format!("claude --resume {CLAUDE_AGENT_SESSION_ID}")
        );
    }

    #[test]
    fn effective_restore_command_resumes_console_manifest_row_with_opencode_thread() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");

        seed_opencode_session_on_disk(home, cwd, OPENCODE_AGENT_SESSION_ID);

        let mut entry = manifest_entry("console", "", Some(OPENCODE_AGENT_SESSION_ID));
        entry.title = Some("opencode · resume after down-sync".into());
        entry.cwd = cwd.into();
        assert_eq!(
            effective_restore_command_at(home, &entry),
            format!("opencode --session {OPENCODE_AGENT_SESSION_ID}")
        );
    }

    fn seed_opencode_session_on_disk(home: &Path, cwd: &str, session_id: &str) {
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

    #[test]
    fn effective_restore_command_replays_shell_command_without_prompt() {
        let entry = manifest_entry("console", "zsh -l", None);
        assert_eq!(effective_restore_command(&entry), "zsh -l");
    }

    #[test]
    fn effective_restore_command_replays_shell_for_non_console_agent() {
        let home = Path::new("/tmp");
        let entry = manifest_entry("grok", "zsh -l", None);
        assert_eq!(effective_restore_command_at(home, &entry), "zsh -l");
    }

    #[test]
    fn effective_restore_command_empty_launch_command_quick_launches() {
        let home = Path::new("/tmp");
        let entry = manifest_entry("grok", "", None);
        assert_eq!(effective_restore_command_at(home, &entry), "grok");
    }
}
