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

/// Shell run by tmux `C-v` in agent/shell panes: load OS clipboard when non-empty, then paste.
pub fn tmux_paste_binding_command() -> String {
    format!("bash -lc '{}'", tmux_paste_binding_script())
}

fn tmux_paste_binding_script() -> String {
    let load = os_clipboard_load_script();
    format!("{load}; tmux paste-buffer -p")
}

fn os_clipboard_load_script() -> String {
    if Path::new("/usr/bin/pbpaste").is_file() {
        return "text=$(pbpaste 2>/dev/null || true); if [ -n \"$text\" ]; then printf %s \"$text\" | tmux load-buffer -; fi".into();
    }
    if Path::new("/usr/bin/xclip").is_file() {
        return "text=$(xclip -selection clipboard -o 2>/dev/null || true); if [ -n \"$text\" ]; then printf %s \"$text\" | tmux load-buffer -; fi".into();
    }
    if Path::new("/usr/bin/wl-paste").is_file() {
        return "text=$(wl-paste --no-newline 2>/dev/null || true); if [ -n \"$text\" ]; then printf %s \"$text\" | tmux load-buffer -; fi".into();
    }
    String::new()
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
    fn tmux_paste_binding_preserves_existing_buffer_when_os_empty() {
        if !Path::new("/usr/bin/pbpaste").is_file() {
            return;
        }
        let script = tmux_paste_binding_script();
        assert!(script.contains("if [ -n"));
        assert!(script.contains("pbpaste"));
        assert!(!script.contains("pbpaste | tmux load-buffer"));
    }
}
