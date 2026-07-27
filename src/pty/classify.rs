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
    static RE_WS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s+").expect("valid regex"));
    let text = RE_WS.replace_all(text, " ").to_string();
    if text.len() > 42 {
        format!("{}...", text[..39].trim_end())
    } else {
        text
    }
}

/// True for language launchers where the useful identity is the script/module, not the binary.
pub fn is_language_launcher(binary: &str) -> bool {
    let base = binary
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or(binary)
        .to_ascii_lowercase();
    matches!(
        base.as_str(),
        "env"
            | "python"
            | "python2"
            | "python3"
            | "node"
            | "nodejs"
            | "deno"
            | "bun"
            | "ruby"
            | "perl"
            | "php"
            | "lua"
            | "luajit"
            | "r"
            | "rscript"
            | "julia"
            | "dotnet"
            | "java"
    ) || base.starts_with("python")
        || base.starts_with("ruby")
        || base.starts_with("perl")
        || base.starts_with("php")
        || base.starts_with("node")
}

/// Script/module identity from an interpreter command line, for sidebar titles.
///
/// Examples:
/// - `python3 ./train.py` → `./train.py`
/// - `/usr/bin/env python3 /tmp/foo.py` → `foo.py` (absolute → basename)
/// - `python3 -m http.server` → `-m http.server`
/// - `node scripts/dev.js --port 3` → `scripts/dev.js --port 3`
pub fn script_run_identity(full_command: &str, cwd: &str) -> Option<String> {
    let tokens: Vec<&str> = full_command.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    let mut i = 0usize;
    // Skip env and interpreter prefixes (shebang often surfaces as `/usr/bin/env python3 …`).
    while i < tokens.len() {
        let base = tokens[i].rsplit('/').next().unwrap_or(tokens[i]);
        if is_language_launcher(base) {
            i += 1;
            continue;
        }
        break;
    }
    if i == 0 {
        // Command does not start with a known launcher — only treat as script when
        // the first token itself looks like a script path.
        return display_script_token(tokens[0], cwd).map(|s| {
            if tokens.len() == 1 {
                s
            } else {
                shorten_command(&format!("{s} {}", tokens[1..].join(" ")))
            }
        });
    }
    if i >= tokens.len() {
        return None;
    }

    // `python -m module …`
    if tokens[i] == "-m" {
        let module = tokens.get(i + 1)?;
        let rest = if tokens.len() > i + 2 {
            format!("-m {module} {}", tokens[i + 2..].join(" "))
        } else {
            format!("-m {module}")
        };
        return Some(shorten_command(&rest));
    }

    // Skip interpreter flags until a positional script path.
    while i < tokens.len() {
        let t = tokens[i];
        if t == "--" {
            i += 1;
            break;
        }
        if t.starts_with('-') {
            // Flags that take a value: -c CODE, -W default, etc. Skip one arg for -c/-m handled above.
            if matches!(t, "-c" | "-W" | "-X" | "--check-hash-based-pycs") {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        break;
    }
    if i >= tokens.len() {
        return None;
    }

    let script = display_script_token(tokens[i], cwd)?;
    if tokens.len() == i + 1 {
        Some(script)
    } else {
        Some(shorten_command(&format!(
            "{script} {}",
            tokens[i + 1..].join(" ")
        )))
    }
}

fn display_script_token(token: &str, cwd: &str) -> Option<String> {
    let token = token.trim().trim_matches(|c| c == '"' || c == '\'');
    if token.is_empty() || token == "-" {
        return None;
    }
    // Reject bare words that don't look like scripts/paths (e.g. `runserver` alone is ok
    // only when attached to manage.py — handled by remaining tokens).
    let looks_like_script = token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token.starts_with('~')
        || token.contains('/')
        || token.contains('.');
    if !looks_like_script {
        return None;
    }

    if token.starts_with("./") || token.starts_with("../") {
        return Some(token.to_string());
    }

    let cwd = cwd.trim_end_matches('/');
    if !cwd.is_empty() {
        if let Some(rest) = token.strip_prefix(&format!("{cwd}/")) {
            if !rest.is_empty() {
                return Some(format!("./{rest}"));
            }
        }
    }
    if token.starts_with('/') || token.starts_with('~') {
        let base = token.rsplit('/').next().unwrap_or(token);
        if base.is_empty() {
            return None;
        }
        return Some(base.to_string());
    }
    Some(token.to_string())
}

pub fn classify_pane(binary: &str, full_command: &str, cwd: &str) -> PaneKind {
    let binary_lower = binary.trim().to_ascii_lowercase();
    let binary_base = binary_lower
        .rsplit('/')
        .next()
        .unwrap_or(binary_lower.as_str());
    if is_shell_binary(binary_base) {
        let cwd_leaf = cwd.split('/').next_back().unwrap_or(cwd);
        return PaneKind::Shell {
            name: cwd_leaf.to_string(),
        };
    }

    // Interpreter + script: title is the script (what the user ran), not python3.13.
    let first_token = full_command
        .split_whitespace()
        .next()
        .unwrap_or(binary)
        .rsplit('/')
        .next()
        .unwrap_or(binary);
    if is_language_launcher(binary_base) || is_language_launcher(first_token) {
        if let Some(identity) = script_run_identity(full_command, cwd) {
            return PaneKind::Tool {
                app: identity.clone(),
                thread: identity,
                command: full_command.to_string(),
                cwd: cwd.to_string(),
            };
        }
    }

    if let Some(profile) = get_app_profile(binary).or_else(|| get_app_profile(binary_base)) {
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

    if let Some(agent) = crate::agents::agent_for_binary(binary)
        .or_else(|| crate::agents::agent_for_binary(binary_base))
    {
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
