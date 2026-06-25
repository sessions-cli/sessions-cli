use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Clone, Copy)]
pub struct AppProfile {
    pub binary_name: &'static str,
    pub display_name: &'static str,
    pub prompt_extractor: Option<fn(&str) -> Option<String>>,
    pub name_formatter: Option<fn(&str, &str) -> String>,
    pub state_file_pattern: Option<&'static str>,
    pub known_subcommands: &'static [&'static str],
}

impl AppProfile {
    pub const fn new(binary_name: &'static str, display_name: &'static str) -> Self {
        Self {
            binary_name,
            display_name,
            prompt_extractor: None,
            name_formatter: None,
            state_file_pattern: None,
            known_subcommands: &[],
        }
    }

    pub const fn with_prompt_extractor(mut self, f: fn(&str) -> Option<String>) -> Self {
        self.prompt_extractor = Some(f);
        self
    }

    pub const fn with_name_formatter(mut self, f: fn(&str, &str) -> String) -> Self {
        self.name_formatter = Some(f);
        self
    }

    pub const fn with_state_file(mut self, pattern: &'static str) -> Self {
        self.state_file_pattern = Some(pattern);
        self
    }

    pub const fn with_subcommands(mut self, cmds: &'static [&'static str]) -> Self {
        self.known_subcommands = cmds;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneKind {
    Shell {
        name: String,
    },
    Tool {
        app: String,
        thread: String,
        command: String,
        cwd: String,
    },
}

static APP_REGISTRY: LazyLock<Mutex<HashMap<&'static str, AppProfile>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register_app_profile(profile: AppProfile) {
    APP_REGISTRY
        .lock()
        .expect("app registry lock")
        .insert(profile.binary_name, profile);
}

pub fn get_app_profile(binary: &str) -> Option<AppProfile> {
    APP_REGISTRY
        .lock()
        .expect("app registry lock")
        .get(binary)
        .copied()
}

pub fn is_known_agent(binary: &str) -> bool {
    get_app_profile(binary).is_some()
}

pub fn is_shell_binary(binary: &str) -> bool {
    matches!(
        binary.trim().to_ascii_lowercase().as_str(),
        "zsh" | "bash" | "sh" | "fish" | "nu"
    )
}

pub fn extract_natural_language_arg(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .filter(|a| a.trim_matches(&['"', '\''][..]).len() > 5)
        .find(|a| looks_like_prompt(a))
        .map(|a| a.trim_matches(&['"', '\''][..]).to_string())
}

fn looks_like_prompt(s: &str) -> bool {
    let has_vowels = s.chars().filter(|c| "aeiouAEIOU".contains(*c)).count() >= 3;
    let has_space = s.contains(' ');
    let seems_like_path = s.starts_with('/') || s.starts_with('~') || s.starts_with('.');
    (has_space || has_vowels) && !seems_like_path && !s.contains("://")
}

pub fn shorten_command(raw: &str) -> String {
    let text = raw.trim().lines().next().unwrap_or("").trim();
    if text.is_empty() {
        return String::new();
    }
    let re_ws = regex::Regex::new(r"\s+").unwrap();
    let text = re_ws.replace_all(text, " ").to_string();
    if text.len() > 42 {
        format!("{}...", &text[..39].trim_end())
    } else {
        text
    }
}

pub fn classify_pane(binary: &str, full_command: &str, cwd: &str) -> PaneKind {
    let binary_lower = binary.trim().to_ascii_lowercase();
    if is_shell_binary(&binary_lower) {
        let cwd_leaf = cwd.split('/').next_back().unwrap_or(cwd);
        return PaneKind::Shell {
            name: cwd_leaf.to_string(),
        };
    }

    if let Some(profile) = get_app_profile(binary) {
        let prompt = profile
            .prompt_extractor
            .and_then(|f| f(full_command))
            .or_else(|| extract_natural_language_arg(full_command));
        let thread = prompt.unwrap_or_else(|| shorten_command(full_command));
        let display_name = profile.display_name.to_string();
        return PaneKind::Tool {
            app: display_name,
            thread,
            command: full_command.to_string(),
            cwd: cwd.to_string(),
        };
    }

    if let Some(agent) = crate::agents::agent_for_binary(binary) {
        let prompt = agent
            .extract_thread(full_command)
            .or_else(|| extract_natural_language_arg(full_command));
        let thread = prompt.unwrap_or_else(|| "?".into());
        return PaneKind::Tool {
            app: agent.id().to_string(),
            thread,
            command: full_command.to_string(),
            cwd: cwd.to_string(),
        };
    }

    let prompt = extract_natural_language_arg(full_command);
    let thread = prompt.unwrap_or_else(|| shorten_command(full_command));
    let app = binary.split('/').next_back().unwrap_or(binary).to_string();

    PaneKind::Tool {
        app,
        thread,
        command: full_command.to_string(),
        cwd: cwd.to_string(),
    }
}

fn init_app_registry() {
    let extract = extract_natural_language_arg as fn(&str) -> Option<String>;

    for provider in crate::agents::registry::PROVIDERS {
        register_app_profile(
            AppProfile::new(provider.id, provider.adapter.display_name())
                .with_prompt_extractor(extract),
        );
    }
    register_app_profile(AppProfile::new("cursor", "cursor"));
    register_app_profile(AppProfile::new("windsurf", "windsurf"));
}

pub fn ensure_app_registry() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(init_app_registry);
}
