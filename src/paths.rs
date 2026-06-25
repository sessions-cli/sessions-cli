use std::path::{Path, PathBuf};

/// Resolve the user's home directory.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg_data_home(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
}

/// Root directory for sessions runtime data (state, spool, logs, scripts).
pub fn data_root(home: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("SESSIONS_DATA_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    xdg_data_home(home).join("sessions")
}

pub fn state_dir(home: &Path) -> PathBuf {
    data_root(home).join("state")
}

pub fn spool_dir(home: &Path) -> PathBuf {
    data_root(home).join("spool")
}

pub fn logs_dir(home: &Path) -> PathBuf {
    data_root(home).join("logs")
}

pub fn scripts_dir(home: &Path) -> PathBuf {
    data_root(home).join("scripts")
}

/// Directory containing the installed `sessions` binary.
pub fn install_dir(home: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("SESSIONS_INSTALL_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    xdg_data_home(home).join("sessions/bin")
}

pub fn binary_path(home: &Path) -> PathBuf {
    install_dir(home).join("sessions")
}

/// Candidate binary locations, most preferred first.
pub fn binary_candidates(home: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("SESSIONS_BIN") {
        let path = path.trim();
        if !path.is_empty() {
            candidates.push(PathBuf::from(path));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        candidates.push(exe);
    }
    candidates.push(home.join(".local/bin/sessions"));
    candidates.push(binary_path(home));
    candidates.push(PathBuf::from("sessions"));
    candidates
}

pub fn resolve_binary(home: &Path) -> PathBuf {
    binary_candidates(home)
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("sessions"))
}

pub fn config_dir(home: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir).join("sessions");
        }
    }
    home.join(".config/sessions")
}

pub fn config_path(home: &Path) -> PathBuf {
    config_dir(home).join("config.toml")
}

pub fn telemetry_dir(home: &Path) -> PathBuf {
    data_root(home).join("telemetry")
}

pub fn telemetry_pending_path(home: &Path) -> PathBuf {
    state_dir(home).join("telemetry-pending.json")
}

/// On-disk session storage for a provider (e.g. Grok's `~/.grok/sessions`).
pub fn provider_sessions_dir(home: &Path, provider_id: &str) -> PathBuf {
    match provider_id {
        "grok" => home.join(".grok/sessions"),
        other => home.join(format!(".{other}/sessions")),
    }
}

/// Grok agent session storage (owned by Grok, not sessions-cli).
#[inline]
pub fn grok_sessions_dir(home: &Path) -> PathBuf {
    provider_sessions_dir(home, "grok")
}

/// Legacy Grok integration path used by Ctrl+N and ~/.grok/scripts helpers.
pub fn grok_legacy_binary_path(home: &Path) -> PathBuf {
    std::env::var_os("GROK_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|root| root.join("scripts/sessions"))
        .unwrap_or_else(|| home.join(".grok/scripts/sessions"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_sessions_dir_grok_matches_legacy_alias() {
        let home = PathBuf::from("/home/testuser");
        assert_eq!(
            provider_sessions_dir(&home, "grok"),
            grok_sessions_dir(&home)
        );
        assert_eq!(
            provider_sessions_dir(&home, "grok"),
            home.join(".grok/sessions")
        );
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::sync::OnceLock;

    pub fn home() -> PathBuf {
        PathBuf::from("/home/testuser")
    }

    pub fn repo_root() -> String {
        env!("CARGO_MANIFEST_DIR").to_string()
    }

    pub fn project(name: &str) -> String {
        format!("{}/projects/{name}", home().display())
    }

    pub fn other_project(name: &str) -> String {
        project(name)
    }

    static REPO_ROOT: OnceLock<String> = OnceLock::new();

    pub fn sessions_cli_cwd() -> &'static str {
        REPO_ROOT.get_or_init(repo_root)
    }
}
