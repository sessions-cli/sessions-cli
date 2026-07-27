//! Locate and invoke the skillshare CLI.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

const INSTALL_HINT: &str =
    "Install skillshare: brew install skillshare  OR  curl -fsSL https://raw.githubusercontent.com/runkids/skillshare/main/install.sh | sh";

#[derive(Debug, Clone, Serialize)]
pub struct SkillshareStatus {
    pub installed: bool,
    pub binary: Option<PathBuf>,
    pub version: Option<String>,
    pub install_hint: String,
    pub store_exists: bool,
    pub store_dir: PathBuf,
    pub config_dir: PathBuf,
}

/// Resolve skillshare binary: `SKILLSHARE_BIN`, then PATH, then common install locations.
pub fn find_skillshare_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("SKILLSHARE_BIN") {
        let path = path.trim();
        if !path.is_empty() {
            let p = PathBuf::from(path);
            if is_executable(&p) {
                return Some(p);
            }
        }
    }
    if let Ok(path) = which("skillshare") {
        return Some(path);
    }
    let home = crate::paths::home();
    let candidates = [
        home.join(".local/bin/skillshare"),
        PathBuf::from("/opt/homebrew/bin/skillshare"),
        PathBuf::from("/usr/local/bin/skillshare"),
        home.join("go/bin/skillshare"),
    ];
    candidates.into_iter().find(|p| is_executable(p))
}

fn which(name: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Ok(candidate);
        }
    }
    Err(())
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn skillshare_version(bin: &Path) -> Option<String> {
    let output = Command::new(bin).arg("--version").output().ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        let err = String::from_utf8_lossy(&output.stderr);
        let line = text.lines().next().or_else(|| err.lines().next())?;
        return Some(line.trim().to_string());
    }
    let output = Command::new(bin).arg("version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn status(home: &Path) -> SkillshareStatus {
    let config_dir = super::paths::skillshare_config_dir(home);
    let store_dir = super::paths::skillshare_store_dir(home);
    let binary = find_skillshare_binary();
    let version = binary.as_ref().and_then(|b| skillshare_version(b));
    SkillshareStatus {
        installed: binary.is_some(),
        binary,
        version,
        install_hint: INSTALL_HINT.to_string(),
        store_exists: store_dir.is_dir(),
        store_dir,
        config_dir,
    }
}

#[derive(Debug, Clone)]
pub struct SkillshareCommandResult {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn run_skillshare(args: &[&str]) -> Result<SkillshareCommandResult, String> {
    let bin = find_skillshare_binary().ok_or_else(|| INSTALL_HINT.to_string())?;
    let output = Command::new(&bin)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run skillshare: {e}"))?;
    Ok(SkillshareCommandResult {
        ok: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub fn run_init() -> Result<SkillshareCommandResult, String> {
    run_skillshare(&["init"])
}

pub fn run_sync() -> Result<SkillshareCommandResult, String> {
    run_skillshare(&["sync"])
}

pub fn run_audit() -> Result<SkillshareCommandResult, String> {
    run_skillshare(&["audit"])
}

/// Open skillshare web UI in the background.
pub fn run_ui() -> Result<SkillshareCommandResult, String> {
    let bin = find_skillshare_binary().ok_or_else(|| INSTALL_HINT.to_string())?;
    Command::new(&bin)
        .arg("ui")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to open skillshare ui: {e}"))?;
    Ok(SkillshareCommandResult {
        ok: true,
        code: None,
        stdout: "skillshare ui started in background".into(),
        stderr: String::new(),
    })
}

pub fn install_hint() -> &'static str {
    INSTALL_HINT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_without_binary_is_honest() {
        std::env::remove_var("SKILLSHARE_BIN");
        let home = crate::paths::home();
        let s = status(&home);
        assert!(!s.install_hint.is_empty());
        assert_eq!(
            s.store_dir,
            super::super::paths::skillshare_store_dir(&home)
        );
    }
}
