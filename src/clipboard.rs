use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};

const COPY_NOTICE_MS: u32 = 1500;

/// Copy text to the system clipboard (pbcopy, xclip, or wl-copy).
pub fn copy(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    if let Some(mut child) = spawn_copy_writer()? {
        child
            .stdin
            .as_mut()
            .context("clipboard copy stdin")?
            .write_all(text.as_bytes())
            .context("write clipboard")?;
        let status = child.wait().context("wait for clipboard copy")?;
        if !status.success() {
            anyhow::bail!("clipboard copy failed with status {status}");
        }
        return Ok(());
    }
    anyhow::bail!("no system clipboard tool found (pbcopy, xclip, or wl-copy)")
}

/// Read paste text: OS clipboard first, then tmux paste buffer.
pub fn paste() -> Result<String> {
    if let Ok(os) = paste_os_clipboard() {
        if !os.is_empty() {
            return Ok(os);
        }
    }
    paste_tmux_buffer()
}

/// Read plain text from the system clipboard only.
pub fn paste_os_clipboard() -> Result<String> {
    if Path::new("/usr/bin/pbpaste").is_file() {
        let output = Command::new("/usr/bin/pbpaste")
            .output()
            .context("run pbpaste")?;
        if !output.status.success() {
            anyhow::bail!("pbpaste failed");
        }
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    if Path::new("/usr/bin/xclip").is_file() {
        let output = Command::new("/usr/bin/xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
            .context("run xclip")?;
        if !output.status.success() {
            anyhow::bail!("xclip read failed");
        }
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    if Path::new("/usr/bin/wl-paste").is_file() {
        let output = Command::new("/usr/bin/wl-paste")
            .args(["--no-newline"])
            .output()
            .context("run wl-paste")?;
        if !output.status.success() {
            anyhow::bail!("wl-paste failed");
        }
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    anyhow::bail!("no system clipboard tool found (pbpaste, xclip, or wl-paste)")
}

/// Read tmux's paste buffer (`tmux save-buffer -`).
pub fn paste_tmux_buffer() -> Result<String> {
    let output = Command::new("tmux")
        .args(["save-buffer", "-"])
        .output()
        .context("run tmux save-buffer")?;
    if !output.status.success() {
        anyhow::bail!(
            "tmux save-buffer failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Load OS clipboard into the tmux paste buffer when non-empty (does not paste).
/// Used by key bindings that then run `paste-buffer -p` in the key's pane context.
pub fn load_os_clipboard_into_tmux_buffer() -> Result<()> {
    if let Ok(os) = paste_os_clipboard() {
        if !os.is_empty() {
            load_tmux_buffer(&os)?;
        }
    }
    Ok(())
}

/// Load OS clipboard into the tmux paste buffer (when non-empty) and paste.
///
/// `target_pane` should be a pane id (e.g. `%42`). **Required** when multiple
/// clients are attached — bare `paste-buffer` follows the server's "current"
/// pane, which is often a different client than the one that pressed the key.
pub fn paste_into_tmux_pane(target_pane: Option<&str>) -> Result<()> {
    load_os_clipboard_into_tmux_buffer()?;
    let mut args = vec!["paste-buffer".to_string(), "-p".to_string()];
    if let Some(pane) = target_pane.map(str::trim).filter(|p| !p.is_empty()) {
        args.push("-t".into());
        args.push(pane.to_string());
    }
    let status = Command::new("tmux")
        .args(&args)
        .status()
        .context("run tmux paste-buffer")?;
    if !status.success() {
        anyhow::bail!("tmux paste-buffer failed with status {status}");
    }
    Ok(())
}

/// Backward-compatible entry: paste into server current pane (prefer `-t`).
pub fn paste_into_active_tmux_pane() -> Result<()> {
    paste_into_tmux_pane(None)
}

fn load_tmux_buffer(text: &str) -> Result<()> {
    let mut child = Command::new("tmux")
        .args(["load-buffer", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .context("spawn tmux load-buffer")?;
    child
        .stdin
        .as_mut()
        .context("tmux load-buffer stdin")?
        .write_all(text.as_bytes())
        .context("write tmux load-buffer")?;
    let status = child.wait().context("wait tmux load-buffer")?;
    if !status.success() {
        anyhow::bail!("tmux load-buffer failed with status {status}");
    }
    Ok(())
}

fn quote_shell_path(path: &Path) -> String {
    let bin = path.display().to_string();
    if bin.is_empty() {
        return "''".into();
    }
    if bin
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        return bin;
    }
    format!("'{}'", bin.replace('\'', "'\\''"))
}

/// Load-only shell fragment: key bindings run this then `paste-buffer -p` so
/// paste targets the pane that received the key (not another attached client).
pub fn tmux_paste_load_shell_command(sessions_bin: &Path) -> String {
    format!(
        "{} paste-tmux --load-only </dev/null >/dev/null 2>&1",
        quote_shell_path(sessions_bin)
    )
}

/// Full paste via subprocess, targeting the key's pane (`#{pane_id}` expanded by tmux).
pub fn tmux_paste_binding_command(sessions_bin: &Path) -> String {
    // `#{{pane_id}}` → `#{pane_id}` after Rust format; tmux expands it at keypress.
    format!(
        "{} paste-tmux -t #{{pane_id}} </dev/null >/dev/null 2>&1",
        quote_shell_path(sessions_bin)
    )
}

/// tmux `copy-pipe-and-cancel` target: read selection from stdin, copy, flash a notice.
pub fn copy_pipe_command() -> Option<String> {
    if Path::new("/usr/bin/pbcopy").is_file() {
        return Some(copy_pipe_shell("pbcopy"));
    }
    if Path::new("/usr/bin/xclip").is_file() {
        return Some(copy_pipe_shell("xclip -selection clipboard"));
    }
    if Path::new("/usr/bin/wl-copy").is_file() {
        return Some(copy_pipe_shell("wl-copy"));
    }
    None
}

fn copy_pipe_shell(copy_cmd: &str) -> String {
    format!("bash -c '{copy_cmd}; tmux display-message -d {COPY_NOTICE_MS} \"Copied\"'")
}

fn spawn_copy_writer() -> Result<Option<Child>> {
    if Path::new("/usr/bin/pbcopy").is_file() {
        return Command::new("/usr/bin/pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .context("spawn pbcopy")
            .map(Some);
    }
    if Path::new("/usr/bin/xclip").is_file() {
        return Command::new("/usr/bin/xclip")
            .args(["-selection", "clipboard"])
            .stdin(Stdio::piped())
            .spawn()
            .context("spawn xclip")
            .map(Some);
    }
    if Path::new("/usr/bin/wl-copy").is_file() {
        return Command::new("/usr/bin/wl-copy")
            .stdin(Stdio::piped())
            .spawn()
            .context("spawn wl-copy")
            .map(Some);
    }
    Ok(None)
}

/// Keep printable characters; allow newlines and tabs for notepad paste.
pub fn sanitize_paste_text(text: &str, allow_newlines: bool) -> String {
    text.chars()
        .filter(|ch| {
            if *ch == '\n' || *ch == '\t' {
                return allow_newlines;
            }
            !ch.is_control()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_paste_text_strips_control_chars() {
        assert_eq!(sanitize_paste_text("hello\x07world", true), "helloworld");
        assert_eq!(sanitize_paste_text("a\nb", true), "a\nb");
        assert_eq!(sanitize_paste_text("a\nb", false), "ab");
    }

    #[test]
    fn copy_pipe_shell_includes_display_message() {
        let cmd = copy_pipe_shell("pbcopy");
        assert!(cmd.contains("pbcopy"));
        assert!(cmd.contains("display-message"));
        assert!(cmd.contains("Copied"));
    }

    #[test]
    fn tmux_paste_binding_command_is_run_shell_safe() {
        let cmd = tmux_paste_binding_command(Path::new("/home/testuser/.local/bin/sessions"));
        assert!(cmd.contains("paste-tmux"));
        assert!(cmd.contains("-t #{pane_id}"));
        assert!(cmd.contains("/home/testuser/.local/bin/sessions"));
        // Nested bash -lc '…' broke bind-key; keep the fragment free of that pattern.
        assert!(!cmd.contains("bash -lc"));
        assert!(!cmd.contains("pbpaste"));
        assert!(!cmd.contains('\''));
    }

    #[test]
    fn tmux_paste_load_shell_command_is_load_only() {
        let cmd = tmux_paste_load_shell_command(Path::new("/home/testuser/.local/bin/sessions"));
        assert!(cmd.contains("paste-tmux --load-only"));
        assert!(!cmd.contains("-t "));
    }

    #[test]
    fn tmux_paste_binding_command_quotes_paths_with_spaces() {
        let cmd = tmux_paste_binding_command(Path::new("/opt/my tools/sessions"));
        assert!(cmd.starts_with("'/opt/my tools/sessions'"));
        assert!(cmd.contains("paste-tmux"));
        assert!(cmd.contains("-t #{pane_id}"));
    }
}
