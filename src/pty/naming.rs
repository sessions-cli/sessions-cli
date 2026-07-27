use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::agents::{
    agent_from_command as registry_agent_from_command,
    agent_from_command_name as registry_agent_from_command_name, infer_agent_for_session,
    is_agent_app as registry_is_agent_app,
};

use crate::session::workspace::WorkspaceRef;

use super::classify::{ensure_app_registry, get_app_profile};

static RE_PROMPT_CMD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^/(implement|review|design|check-work|best-of-n)\s+").expect("valid regex")
});
static RE_PROMPT_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid regex"));
static RE_PROMPT_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\w\s·\-]").expect("valid regex"));

pub const CONSOLE_LABEL: &str = "console";
pub const DEFAULT_AGENT_APP: &str = "grok";

/// Binary/command placeholder before hooks or summary.json produce a real thread title.
pub fn is_machine_derived_thread(thread: &str) -> bool {
    let thread = thread.trim();
    if thread.is_empty() || is_weak_thread_name(thread) {
        return true;
    }
    let lower = thread.to_ascii_lowercase();
    if lower.starts_with("grok-")
        || lower.starts_with("codex-")
        || lower.starts_with("claude-")
        || lower == "grok"
        || lower == "codex"
        || lower == "claude"
        || lower == "opencode"
    {
        return true;
    }
    if let Some((agent, rest)) = lower.split_once(' ') {
        if rest == "resume" && registry_is_agent_app(agent) {
            return true;
        }
    }
    false
}

pub fn is_weak_thread_name(thread: &str) -> bool {
    let thread = thread.trim();
    if thread.is_empty() || thread == "?" || thread == "session" {
        return true;
    }
    if thread.starts_with("~/") || thread.starts_with('/') {
        return true;
    }
    if is_bootstrap_command_label(thread) {
        return true;
    }
    matches!(
        thread.to_ascii_lowercase().as_str(),
        "user_query" | "user query" | "prompt" | "continue" | "new chat"
    )
}

/// Probe/ping prompts are not thread titles — wait for a real task name.
pub fn is_probe_thread_title(thread: &str) -> bool {
    matches!(
        thread.to_ascii_lowercase().as_str(),
        "testing"
            | "test"
            | "hi"
            | "hello"
            | "hey"
            | "ping"
            | "pong"
            | "ok"
            | "yes"
            | "no"
            | "y"
            | "n"
    )
}

/// Sidebar-ready title from hooks or on-disk agent summaries.
pub fn is_confident_thread_title(thread: &str) -> bool {
    let thread = thread.trim();
    if !is_sticky_thread_title(thread) {
        return false;
    }
    thread.contains(' ') || thread.len() >= 12
}

/// User-facing thread label worth keeping for the current agent session.
pub fn is_sticky_thread_title(thread: &str) -> bool {
    let thread = thread.trim();
    if thread.is_empty()
        || is_console_label(thread)
        || is_weak_thread_name(thread)
        || is_machine_derived_thread(thread)
        || is_probe_thread_title(thread)
    {
        return false;
    }
    true
}

/// Sidebar thread labels that are bootstrap/placeholder identity — not real task names.
/// Sticky enough to pass `is_sticky_thread_title` but must not block upgrades.
pub fn is_bootstrap_sidebar_thread(thread: &str) -> bool {
    is_console_label(thread)
        || is_weak_thread_name(thread)
        || is_machine_derived_thread(thread)
        || is_probe_thread_title(thread)
        || is_bootstrap_command_label(thread)
}

/// Workspace bootstrap commands (e.g. ./run-local.sh) are not thread identities.
pub fn is_bootstrap_command_label(thread: &str) -> bool {
    let thread = thread.trim();
    let head = thread.split_whitespace().next().unwrap_or(thread);
    head.starts_with("./")
        || head.starts_with("../")
        || (head.contains('/')
            && head
                .rsplit('/')
                .next()
                .is_some_and(|base| base.contains('.')))
        || {
            let base = head.rsplit('/').next().unwrap_or(head);
            base.contains('.')
                && !base.starts_with('.')
                && base
                    .rsplit_once('.')
                    .is_some_and(|(_, ext)| !ext.is_empty() && ext.len() <= 8)
        }
}

pub fn is_weak_session_title(title: &str) -> bool {
    let title = title.trim();
    if title.is_empty() {
        return true;
    }
    if title.starts_with("~/") || title.starts_with('/') {
        return true;
    }
    is_weak_thread_name(&parse_description(title))
}

pub fn shorten_prompt(raw: &str) -> String {
    let text = raw.trim().lines().next().unwrap_or("").trim();
    if text.is_empty() {
        return String::new();
    }
    // Command/script paths are not user prompts. Stripping `/` and `.` turned
    // `./.local/bin/google-calendar-mcp` into the nonsense thread title
    // `localbingoogle-calendar-mcp` when poll fed classify identity as "prompt".
    if !text.contains(' ')
        && (is_bootstrap_command_label(text)
            || is_live_command_label(text)
            || text.starts_with('/')
            || text.starts_with("~/")
            || text.starts_with("./")
            || text.starts_with("../"))
    {
        return String::new();
    }
    let text = RE_PROMPT_CMD.replace(text, "").to_string();
    let text = RE_PROMPT_WS.replace_all(&text, " ").to_string();
    let text = RE_PROMPT_CHARS.replace_all(&text, "").to_string();
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }
    let short = words[..words.len().min(5)].join(" ");
    if short.len() > 42 {
        format!("{}...", short[..39].trim_end())
    } else {
        short
    }
}

pub fn format_tilde_path(path: &str, home: &Path) -> String {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return "~".into();
    }
    let home_str = home.to_string_lossy();
    if path == home_str.as_ref() {
        return "~".into();
    }
    if let Some(rest) = path.strip_prefix(&format!("{}/", home_str)) {
        return format!("~/{}", rest);
    }
    path.to_string()
}

pub fn derive_project(cwd: &str, home: &Path) -> String {
    let label = format_tilde_path(cwd, home);
    if let Some(rest) = label.strip_prefix("~/projects/") {
        return rest.split('/').next().unwrap_or("other").to_string();
    }
    if label == "~" {
        return "home".into();
    }
    "other".into()
}

pub fn format_session_title(app: &str, thread: &str) -> String {
    let thread = thread.trim();
    let app = app.trim();
    if thread.is_empty() {
        if app.is_empty() || app == "other" {
            return "session".into();
        }
        return app.to_string();
    }
    if app.is_empty() || app == "other" {
        return thread.to_string();
    }
    format!("{app} · {thread}")
}

pub fn is_console_label(text: &str) -> bool {
    matches!(text.trim(), "console" | "raw terminal")
}

pub fn is_console_session(description: &str, title: &str) -> bool {
    is_console_label(description) || is_console_label(&parse_description(title))
}

pub fn build_description(task: &str, _cwd: &str, _home: &Path) -> String {
    if task.is_empty() {
        "session".into()
    } else {
        format_session_title("grok", task)
    }
}

pub fn parse_app(title: &str) -> Option<String> {
    let title = title.trim();
    let (app, _) = title.split_once(" · ")?;
    let app = app.trim();
    (!app.is_empty()).then(|| app.to_string())
}

pub fn parse_description(title: &str) -> String {
    let title = title.trim();
    if let Some((_, rest)) = title.split_once(" · ") {
        let rest = rest.trim();
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    if title.is_empty() {
        "session".into()
    } else {
        title.to_string()
    }
}

pub fn is_agent_app(app: &str) -> bool {
    registry_is_agent_app(app) || get_app_profile(app).is_some()
}

pub fn resolve_agent_app(existing_title: &str) -> String {
    parse_app(existing_title)
        .filter(|app| is_agent_app(app))
        .map(|app| app.to_ascii_lowercase())
        .unwrap_or_else(|| DEFAULT_AGENT_APP.to_string())
}

pub fn detect_runtime_agent() -> Option<String> {
    crate::agents::detect_runtime_agent()
}

pub fn workspace_project(title: &str, cwd: &str, home: &Path) -> String {
    let title = title.trim();
    if let Some((prefix, _)) = title.split_once(" · ") {
        let prefix = prefix.trim();
        if !prefix.is_empty() && !is_agent_app(prefix) {
            return prefix.to_string();
        }
    }
    derive_project(cwd, home)
}

pub fn normalize_workspace_command(command: &str) -> String {
    let command = command.trim();
    if let Some((head, _)) = command.rsplit_once("|| exec") {
        return head.trim().to_string();
    }
    command.to_string()
}

/// Extract the agent/script bootstrap from a tmux pane start command.
pub fn bootstrap_command_from_pane_start(start_command: &str) -> Option<String> {
    let start = start_command.trim();
    if start.is_empty() {
        return None;
    }
    let inner = if let Some((_, rest)) = start.split_once("-lc") {
        rest.trim().trim_matches('"').trim_matches('\'').trim()
    } else {
        start
    };
    let norm = normalize_workspace_command(inner);
    if norm.is_empty() || is_shell_command(&norm) {
        return None;
    }
    if agent_from_command(&norm).is_some() || !descriptor_from_workspace_command(&norm).is_empty() {
        Some(norm)
    } else {
        None
    }
}

pub fn agent_from_command_name(name: &str) -> Option<String> {
    registry_agent_from_command_name(name)
}

pub fn agent_from_command(command: &str) -> Option<String> {
    registry_agent_from_command(command)
}

/// Live process labels that are weak as agent *thread* names but valid sidebar titles
/// while a script/tool is actually running (`./train.py`, `manage.py runserver`).
pub fn is_live_command_label(label: &str) -> bool {
    let label = label.trim();
    if label.is_empty() || is_console_label(label) || is_agent_app(label) {
        return false;
    }
    if is_bootstrap_command_label(label) {
        return true;
    }
    // Full command identity after script extraction may include args.
    let head = label.split_whitespace().next().unwrap_or(label);
    is_bootstrap_command_label(head)
        || head.contains('/')
        || super::classify::is_language_launcher(head)
}

/// Foreground app label for tmux poll naming — agents and non-agent binaries alike.
pub fn poll_foreground_app<'a>(
    at_shell_prompt: bool,
    classify_app: Option<&'a str>,
    runtime_agent: Option<&'a str>,
) -> Option<&'a str> {
    if at_shell_prompt {
        return None;
    }
    classify_app
        .filter(|app| {
            is_agent_app(app) || is_live_command_label(app) || !is_machine_derived_thread(app)
        })
        .or(runtime_agent)
}

/// Poll-resolved title for a non-agent foreground process (not console / placeholder).
pub fn is_foreground_tool_identity(title: &str, description: &str) -> bool {
    let title = title.trim();
    let description = description.trim();
    if description.is_empty() || is_console_label(description) || is_agent_app(description) {
        return false;
    }
    // Script paths are weak as agent threads but valid live tool titles.
    if is_live_command_label(description) {
        return title == description
            || parse_description(title) == description
            || title.ends_with(&format!(" · {description}"));
    }
    if is_weak_thread_name(description) || is_machine_derived_thread(description) {
        return false;
    }
    if let Some(app) = parse_app(title) {
        if is_agent_app(&app) {
            return false;
        }
        return description.starts_with(&format!("{app} "))
            || description.starts_with(&format!("{app}."))
            || description.contains('/');
    }
    description == title
}

pub fn is_shell_command(command: &str) -> bool {
    let command = normalize_workspace_command(command);
    if command.is_empty() {
        return true;
    }
    let first = command.split_whitespace().next().unwrap_or("");
    agent_from_command_name(first).is_none()
        && matches!(
            first.rsplit('/').next().unwrap_or(first),
            "zsh" | "bash" | "sh" | "fish" | "nu"
        )
}

/// Prefer the live foreground process; when idle at a shell prompt, fall back to agent
/// bootstrap commands (grok/codex) so those panes are not labeled "console". Script
/// bootstraps (e.g. ./run-local.sh) are ignored while at a prompt.
pub fn effective_workspace_command<'a>(bootstrap: &'a str, current: &'a str) -> &'a str {
    let current_norm = normalize_workspace_command(current);
    if !current_norm.is_empty() && !is_shell_command(&current_norm) {
        return current;
    }
    let bootstrap_norm = normalize_workspace_command(bootstrap);
    if !bootstrap_norm.is_empty()
        && !is_shell_command(&bootstrap_norm)
        && agent_from_command(&bootstrap_norm).is_some()
    {
        return bootstrap;
    }
    current
}

pub fn descriptor_from_workspace_command(command: &str) -> String {
    let command = normalize_workspace_command(command);
    // Empty / shell → no tool descriptor. Callers use cwd leaf via `is_shell_pane`
    // / `default_thread_name`. Returning "console" here made every idle unbound
    // shell title "console" even in project directories.
    if command.is_empty() || is_shell_command(&command) {
        return String::new();
    }
    if agent_from_command(&command).is_some() {
        return String::new();
    }
    super::classify::shorten_command(&command)
}

pub fn default_thread_name(cwd: &str, home: &Path) -> String {
    let cwd = cwd.trim_end_matches('/');
    if cwd.is_empty() || cwd == home.to_string_lossy().as_ref() {
        return CONSOLE_LABEL.into();
    }
    Path::new(cwd)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "session".into())
}

pub fn session_names(app: &str, thread: &str) -> (String, String) {
    let thread = thread.trim();
    let title = format_session_title(app, thread);
    (title, thread.to_string())
}

pub fn session_names_from_prompt(
    hook_prompt: &str,
    existing_title: &str,
) -> Option<(String, String)> {
    if hook_prompt.is_empty() {
        return None;
    }
    let thread = shorten_prompt(hook_prompt);
    if thread.is_empty() {
        return None;
    }
    let app = resolve_agent_app(existing_title);
    Some(session_names(&app, &thread))
}

pub fn title_from_prompt(hook_prompt: &str, existing_title: &str) -> Option<String> {
    session_names_from_prompt(hook_prompt, existing_title).map(|(title, _)| title)
}

pub fn resolve_session_names(
    home: &Path,
    cwd: &str,
    runtime_agent: Option<&str>,
    agent_session_id: Option<&str>,
    existing_title: &str,
    existing_description: &str,
    prompt: &str,
    workspace: Option<WorkspaceRef<'_>>,
    prefer_prompt: bool,
) -> (String, String, String) {
    ensure_app_registry();
    let at_home = {
        let cwd = cwd.trim_end_matches('/');
        cwd.is_empty() || cwd == home.to_string_lossy().as_ref()
    };
    // "console" is only sticky for real home shells. Project-dir panes that were
    // mislabeled console (or still have window_name console) must re-resolve to
    // the directory leaf / live tool identity.
    if agent_session_id.is_none()
        && runtime_agent.is_none()
        && at_home
        && is_console_session(existing_description, existing_title)
    {
        return (
            CONSOLE_LABEL.to_string(),
            CONSOLE_LABEL.to_string(),
            String::new(),
        );
    }
    let lookup_cwd = agent_session_id
        .map(|id| crate::agents::disk_lookup_cwd(home, cwd, Some(id)))
        .unwrap_or_else(|| cwd.to_string());
    let summary =
        agent_session_id.and_then(|id| crate::agents::load_session_summary(home, &lookup_cwd, id));
    let session_commenced = agent_session_id
        .map(|id| crate::agents::session_has_commenced(home, &lookup_cwd, id))
        .unwrap_or(false);
    let summary_thread = summary.as_ref().and_then(|(summary, agent)| {
        if !session_commenced && prompt.trim().is_empty() {
            return None;
        }
        crate::agents::thread_title_from_summary(summary, agent)
    });
    let session_agent = agent_session_id.and_then(|sid| {
        infer_agent_for_session(home, &lookup_cwd, sid).map(|agent| agent.id().to_string())
    });
    let prompt_thread = {
        let thread = shorten_prompt(prompt);
        let acceptable = if prefer_prompt {
            is_sticky_thread_title(&thread)
        } else {
            is_confident_thread_title(&thread)
        };
        acceptable.then_some(thread)
    };
    let has_summary_thread = summary_thread.is_some();
    let bare_agent_prompt = prompt_thread
        .as_ref()
        .is_none_or(|t| is_machine_derived_thread(t));

    let workspace_title = workspace.map(|w| w.title).unwrap_or("");
    let workspace_command = workspace.map(|w| w.command).unwrap_or("");
    let workspace_thread = workspace.and_then(|w| {
        agent_session_id?;
        if !w.title.contains(" · ") {
            return None;
        }
        let thread = parse_description(w.title);
        (!thread.is_empty() && thread != "session").then_some(thread)
    });
    let command_descriptor = descriptor_from_workspace_command(workspace_command);
    let project = workspace_project(workspace_title, cwd, home);
    let command_agent = if agent_session_id.is_none() && runtime_agent.is_none() {
        None
    } else {
        agent_from_command(workspace_command)
    };
    let existing_agent = parse_app(existing_title).filter(|app| is_agent_app(app));
    let has_existing_agent = existing_agent.is_some();
    // Shell panes: explicit shell bootstrap, or idle unbound poll with no workspace
    // command and no agent branding to preserve. Empty command used to hit
    // command_descriptor→"console" and wipe project-dir titles.
    let is_shell_pane = {
        let cmd = workspace_command.trim();
        if cmd.is_empty() {
            existing_agent.is_none()
                && agent_session_id.is_none()
                && runtime_agent
                    .map(str::trim)
                    .filter(|agent| !agent.is_empty())
                    .is_none()
        } else {
            is_shell_command(workspace_command)
        }
    };
    let foreground_app = runtime_agent
        .map(|agent| agent.trim().to_ascii_lowercase())
        .filter(|agent| !agent.is_empty());
    let known_agent = foreground_app
        .as_ref()
        .filter(|agent| is_agent_app(agent))
        .cloned();
    let has_known_agent_context = agent_session_id.is_some()
        || summary_thread.is_some()
        || known_agent.is_some()
        || command_agent.is_some()
        || prompt_thread.is_some() && (agent_session_id.is_some() || known_agent.is_some());
    let agent_app = known_agent
        .clone()
        .or(command_agent.clone())
        .or(session_agent)
        .or({
            if is_shell_pane && !has_known_agent_context {
                None
            } else {
                existing_agent
            }
        });
    let has_agent_context = if is_shell_pane {
        has_known_agent_context
    } else {
        has_known_agent_context || has_existing_agent
    };

    if let Some(tool_app) = foreground_app.filter(|app| !is_agent_app(app)) {
        // Live script labels (./train.py) are weak as agent threads but valid
        // tool titles while the process is running.
        let classified = non_empty(existing_description)
            .filter(|thread| !is_weak_thread_name(thread) || is_live_command_label(thread))
            .filter(|thread| {
                agent_session_id.is_none()
                    || thread == tool_app.as_str()
                    || thread.starts_with(tool_app.as_str())
                    || thread.contains('/')
                    || thread.contains('.')
                    || is_live_command_label(thread)
            });
        let thread = prompt_thread
            .or(classified)
            .or_else(|| {
                is_live_command_label(&tool_app)
                    .then(|| tool_app.clone())
                    .or_else(|| non_weak_thread(&tool_app))
            })
            .unwrap_or_else(|| tool_app.clone());
        // Prefer a bare script title over `python · script` — the launcher is noise.
        let app_label = if thread == tool_app
            || is_live_command_label(&thread)
            || super::classify::is_language_launcher(&tool_app)
        {
            String::new()
        } else {
            tool_app.clone()
        };
        let (title, description) = session_names(&app_label, &thread);
        let project = if app_label.is_empty() {
            project
        } else {
            tool_app
        };
        return (title, description, project);
    }

    let thread = if is_shell_pane && !has_known_agent_context {
        if cwd.trim_end_matches('/') == home.to_string_lossy().as_ref() {
            CONSOLE_LABEL.into()
        } else {
            default_thread_name(cwd, home)
        }
    } else if has_agent_context {
        let chain = if prefer_prompt {
            prompt_thread
                .or(summary_thread)
                .or_else(|| non_weak_thread(existing_description))
                .or_else(|| thread_from_title(existing_title))
                .or(workspace_thread)
        } else {
            summary_thread
                .or(prompt_thread)
                .or_else(|| non_weak_thread(existing_description))
                .or_else(|| thread_from_title(existing_title))
                .or(workspace_thread)
        };
        chain.unwrap_or_else(|| "?".into())
    } else if !command_descriptor.is_empty() {
        command_descriptor
    } else {
        let chain = if prefer_prompt {
            prompt_thread
                .or_else(|| non_weak_thread(existing_description))
                .or_else(|| thread_from_title(existing_title))
                .or(workspace_thread)
        } else {
            non_weak_thread(existing_description)
                .or_else(|| thread_from_title(existing_title))
                .or(workspace_thread)
        };
        chain.unwrap_or_else(|| default_thread_name(cwd, home))
    };

    if let Some(agent) = agent_app.clone() {
        if thread == agent && !has_summary_thread && bare_agent_prompt {
            return (agent.clone(), agent.clone(), agent);
        }
    }

    let app = if let Some(agent) = agent_app {
        agent
    } else if is_console_label(&thread) || (is_shell_pane && !has_known_agent_context) {
        String::new()
    } else {
        project.clone()
    };

    let (title, description) = session_names(&app, &thread);
    (title, description, app)
}

pub fn normalize_agent_label(
    session: &mut crate::model::Session,
    workspace: Option<WorkspaceRef<'_>>,
    runtime_agent: Option<&str>,
    _agent_session_id: Option<&str>,
) {
    if session.title_manual {
        return;
    }
    let current_app = parse_app(&session.title);
    let command_agent = workspace.and_then(|w| agent_from_command(w.command));
    let runtime_agent = runtime_agent
        .map(|agent| agent.trim().to_ascii_lowercase())
        .filter(|agent| is_agent_app(agent));
    let needs_fix =
        current_app.as_deref() == Some("cursor") || session.project.eq_ignore_ascii_case("cursor");
    if !needs_fix && command_agent.is_none() && runtime_agent.is_none() {
        return;
    }
    let app = runtime_agent.or(command_agent.or(current_app.filter(|name| is_agent_app(name))));
    let Some(app) = app else {
        return;
    };
    let thread = parse_description(&session.title);
    if is_weak_thread_name(&thread) {
        return;
    }
    let (title, description) = session_names(&app, &thread);
    session.title = title;
    session.description = description;
    session.project = app;
}

pub fn session_names_from_window_name(
    name: &str,
    cwd: &str,
    home: &Path,
    workspace: Option<WorkspaceRef<'_>>,
) -> (String, String, String) {
    resolve_session_names(home, cwd, None, None, name, "", "", workspace, false)
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn non_weak_thread(value: &str) -> Option<String> {
    non_empty(value).filter(|thread| !is_weak_thread_name(thread))
}

fn thread_from_title(title: &str) -> Option<String> {
    let thread = parse_description(title);
    (!is_weak_thread_name(&thread)).then_some(thread)
}
