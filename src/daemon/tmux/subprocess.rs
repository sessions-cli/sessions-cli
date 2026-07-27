use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{mpsc, OnceLock};
use std::thread;
use std::time::Duration;

const TMUX_INSTALL_HINT: &str = "tmux is required. Install with: brew install tmux";

/// Default wall-clock budget for a single tmux CLI call.
/// Hung tmux (or a wedged server) must not freeze the sidebar forever.
pub const TMUX_COMMAND_TIMEOUT: Duration = Duration::from_millis(1_000);

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

fn kill_process_group(pid: u32) {
    // Negative pid targets the process group. Child is put in its own group via setpgid.
    let _ = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
}

/// Run `command` with a wall-clock timeout. On timeout the process group is killed.
///
/// The child is placed in a new process group so `kill(-pid)` reaps any short-lived
/// grandchildren. No extra Cargo deps — std + libc only.
pub(crate) fn run_command_with_timeout(mut command: Command, timeout: Duration) -> Result<Output> {
    use std::os::unix::process::CommandExt;

    // Own process group so timeout can kill the whole tree.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn command")?;

    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(err)) => Err(err).context("failed to wait for command"),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_process_group(pid);
            // Reap so we don't leave a zombie; ignore result.
            let _ = rx.recv_timeout(Duration::from_millis(250));
            anyhow::bail!("command timed out after {}ms", timeout.as_millis());
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            kill_process_group(pid);
            anyhow::bail!("command waiter disconnected")
        }
    }
}

pub fn session_exists(session: &str) -> bool {
    let Ok(tmux) = resolve_tmux_binary() else {
        return false;
    };
    let mut command = Command::new(&tmux);
    command.args(["has-session", "-t", session]);
    // Fast path: same kill-on-timeout as run_tmux so a hung server cannot stall probes.
    match run_command_with_timeout(command, TMUX_COMMAND_TIMEOUT) {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

pub fn run_tmux(args: &[&str]) -> Result<Output> {
    run_tmux_with_timeout(args, TMUX_COMMAND_TIMEOUT)
}

pub fn run_tmux_with_timeout(args: &[&str], timeout: Duration) -> Result<Output> {
    let tmux = resolve_tmux_binary()?;
    let timer = crate::daemon::metrics::TmuxCommandTimer::start();
    let mut command = Command::new(&tmux);
    command.args(args);
    let output = run_command_with_timeout(command, timeout)
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

/// Play the sessions alert sound without blocking the caller.
///
/// Hook and poll paths must stay latency-sensitive, so this always spawns and
/// returns immediately. Override with `SESSIONS_SOUND_CMD` (empty disables).
pub fn play_alert_sound() -> Result<()> {
    use std::process::Stdio;

    if let Ok(cmd) = std::env::var("SESSIONS_SOUND_CMD") {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        Command::new("/bin/sh")
            .args(["-lc", trimmed])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| "spawn SESSIONS_SOUND_CMD")?;
        return Ok(());
    }

    // Prefer absolute paths so PATH-stripped daemon/bar environments still ring.
    for (program, args) in [
        ("/usr/bin/afplay", vec!["/System/Library/Sounds/Glass.aiff"]),
        ("afplay", vec!["/System/Library/Sounds/Glass.aiff"]),
        (
            "paplay",
            vec!["/usr/share/sounds/freedesktop/stereo/complete.oga"],
        ),
        ("aplay", vec!["/usr/share/sounds/alsa/Front_Center.wav"]),
    ] {
        if Command::new(program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn run_command_with_timeout_kills_hung_child() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let start = Instant::now();
        let err = run_command_with_timeout(cmd, Duration::from_millis(80))
            .expect_err("sleep should time out");
        let elapsed = start.elapsed();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("timed out"),
            "expected timeout error, got: {msg}"
        );
        // Should not wait for the full 30s sleep.
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout path took too long: {elapsed:?}"
        );
    }

    #[test]
    fn run_command_with_timeout_returns_fast_success() {
        let cmd = Command::new("true");
        let output =
            run_command_with_timeout(cmd, Duration::from_millis(500)).expect("true should succeed");
        assert!(output.status.success());
    }
}
