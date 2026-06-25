use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const TMUX_INSTALL_HINT: &str = "tmux is required. Install with: brew install tmux";

static RESOLVED_TMUX_BIN: OnceLock<Result<PathBuf, String>> = OnceLock::new();

fn resolve_tmux_binary_uncached() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("TMUX_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        anyhow::bail!("TMUX_BIN points to missing file: {}", path.display());
    }

    if Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return Ok(PathBuf::from("tmux"));
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates = vec![
        "/opt/homebrew/bin/tmux".to_string(),
        "/usr/local/bin/tmux".to_string(),
    ];
    if !home.is_empty() {
        candidates.push(format!("{home}/.homebrew/bin/tmux"));
    }
    for candidate in candidates {
        let path = Path::new(&candidate);
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
    }

    anyhow::bail!("{TMUX_INSTALL_HINT}")
}

pub fn resolve_tmux_binary() -> Result<PathBuf> {
    RESOLVED_TMUX_BIN
        .get_or_init(|| resolve_tmux_binary_uncached().map_err(|err| err.to_string()))
        .clone()
        .map_err(|msg| anyhow::anyhow!("{msg}"))
}

pub fn ensure_tmux_available() -> Result<PathBuf> {
    resolve_tmux_binary()
}

pub fn session_exists(session: &str) -> bool {
    let Ok(tmux) = resolve_tmux_binary() else {
        return false;
    };
    Command::new(&tmux)
        .args(["has-session", "-t", session])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn run_tmux(args: &[&str]) -> Result<std::process::Output> {
    let tmux = resolve_tmux_binary()?;
    let timer = crate::daemon::metrics::TmuxCommandTimer::start();
    let output = Command::new(&tmux)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {} {}", tmux.display(), args.join(" ")))?;
    drop(timer);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("exit {}", output.status.code().unwrap_or(-1))
        };
        anyhow::bail!(
            "tmux {} failed: {detail}",
            args.first().copied().unwrap_or("command")
        );
    }
    Ok(output)
}
pub fn play_alert_sound() -> Result<()> {
    if let Ok(cmd) = std::env::var("SESSIONS_SOUND_CMD") {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        Command::new("/bin/sh")
            .args(["-lc", trimmed])
            .output()
            .with_context(|| "run SESSIONS_SOUND_CMD")?;
        return Ok(());
    }

    for (program, args) in [
        ("afplay", vec!["/System/Library/Sounds/Glass.aiff"]),
        (
            "paplay",
            vec!["/usr/share/sounds/freedesktop/stereo/complete.oga"],
        ),
        ("aplay", vec!["/usr/share/sounds/alsa/Front_Center.wav"]),
    ] {
        if let Ok(output) = Command::new(program).args(&args).output() {
            if output.status.success() {
                return Ok(());
            }
        }
    }

    Ok(())
}
