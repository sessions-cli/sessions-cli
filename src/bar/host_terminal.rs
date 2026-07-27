//! Host terminal capabilities for the sidebar bar.
//!
//! The bar process runs **inside** tmux (`TERM_PROGRAM=tmux`), so process env does
//! not reflect the outer client (Ghostty vs Cursor/VS Code). Detection order:
//! 1. Session option set at attach (`@sessions_ui_host`)
//! 2. Attached clients' `client_termname`
//! 3. Process env (tests / non-tmux)
//!
//! # OSC / VT compatibility (outer host)
//!
//! ## Full terminals (Ghostty, Kitty, iTerm, Foot)
//! - OSC 0/2 title, OSC 11 default background, OSC 22 pointer shapes
//! - SGR mouse (1000/1002/1003/1006); tmux border resize
//!
//! ## IDE hosts — VS Code / Cursor integrated terminal (xterm.js)
//! Source of truth: <https://xtermjs.org/docs/api/vtfeatures/>
//!
//! | Sequence | xterm.js | sessions use |
//! |----------|----------|--------------|
//! | OSC 0 title | Partial (title only) | yes (BEL + ST) |
//! | OSC 1 icon | ✗ ignored | emitted (harmless) |
//! | OSC 2 title | ✓ | yes (BEL + ST) |
//! | OSC 11 background | ✓ `#RRGGBB` | yes (BEL + ST) |
//! | OSC 22 pointer | **not supported** | **never emit** |
//! | OSC 52 clipboard | not in core | OS tools (`pbcopy`/…) |
//! | DECSET 1006 SGR mouse | ✓ | crossterm capture |
//! | DECSET 1003 all-motion | ✓ | hover |
//! | DECSCUSR (CSI n q) | ✓ text cursor only | not used for pointer |
//!
//! Resize / pointer UX on IDE therefore cannot depend on OSC 22: use the painted
//! in-bar edge grip + keyboard (`[`/`]`, Cursor/VS Code tasks), never pointer shapes.

use std::env;
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Session option set by attach when the outer host is known.
pub const UI_HOST_OPTION: &str = "@sessions_ui_host";
pub const UI_TERM_PROGRAM_OPTION: &str = "@sessions_ui_term_program";

/// Host is stamped once at attach and does not change mid-session. Cache aggressively
/// so focus probes stop re-running two `tmux show-options` every few hundred ms.
const HOST_DETECT_TTL: Duration = Duration::from_secs(60);

struct HostCache {
    at: Instant,
    ui_session: Option<String>,
    host: HostTerminal,
}

static HOST_CACHE: LazyLock<Mutex<Option<HostCache>>> = LazyLock::new(|| Mutex::new(None));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// Ghostty, Kitty, iTerm, Foot, or unknown native terminal.
    Full,
    /// VS Code / Cursor integrated terminal (xterm.js).
    Ide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostTerminal {
    pub kind: HostKind,
    pub term_program: Option<&'static str>,
}

impl HostTerminal {
    /// OSC 22 mouse pointer shapes (hand / I-beam / col-resize).
    ///
    /// xterm.js (VS Code + Cursor terminal view) does **not** implement OSC 22 —
    /// emitting it is a no-op at best and must never gate UX.
    pub fn supports_osc22(self) -> bool {
        matches!(self.kind, HostKind::Full)
    }

    /// OSC 0 / OSC 2 window/tab title (both BEL and ST terminators).
    /// Safe on Full and IDE (xterm.js `onTitleChange`).
    pub fn supports_osc_title(self) -> bool {
        true
    }

    /// OSC 11 default background (`#RRGGBB`). Safe on Full and IDE (xterm.js ✓).
    pub fn supports_osc11_background(self) -> bool {
        true
    }

    /// In-bar right-edge drag (IDE hosts; 1-cell tmux border is unreliable in xterm.js).
    /// Compensates for missing OSC 22 resize-cursor feedback.
    pub fn uses_edge_resize(self) -> bool {
        matches!(self.kind, HostKind::Ide)
    }

    /// Drag-to-select list text. IDE terminals deliver move/drag noise on simple
    /// clicks, so selection feels glued to the pointer and steals activate-on-up.
    pub fn allows_list_text_drag_select(self) -> bool {
        matches!(self.kind, HostKind::Full)
    }

    pub fn detail(self) -> String {
        let prog = self.term_program.unwrap_or("unknown");
        match self.kind {
            HostKind::Full => {
                format!("{prog}: OSC 0/2/11/22; border resize; list drag-select for copy")
            }
            HostKind::Ide => format!(
                "{prog}: OSC 0/2/11 only (no OSC 22); in-bar edge grip + [ / ] / ⌘⌥[ / ]; click activates"
            ),
        }
    }

    fn from_kind(kind: HostKind, term_program: Option<&str>) -> Self {
        Self {
            kind,
            term_program: term_program.map(intern_term_program),
        }
    }
}

pub fn detect() -> HostTerminal {
    detect_for_ui_session(None)
}

pub fn detect_for_ui_session(ui_session: Option<&str>) -> HostTerminal {
    if let Ok(guard) = HOST_CACHE.lock() {
        if let Some(cache) = guard.as_ref() {
            let session_matches = match (ui_session, cache.ui_session.as_deref()) {
                (None, None) => true,
                (Some(a), Some(b)) => a == b,
                // Cached for a concrete UI session; bare detect() can reuse it.
                (None, Some(_)) => true,
                (Some(_), None) => false,
            };
            if session_matches && cache.at.elapsed() < HOST_DETECT_TTL {
                return cache.host;
            }
        }
    }

    let host = detect_for_ui_session_uncached(ui_session);
    if let Ok(mut guard) = HOST_CACHE.lock() {
        *guard = Some(HostCache {
            at: Instant::now(),
            ui_session: ui_session.map(str::to_string),
            host,
        });
    }
    host
}

fn detect_for_ui_session_uncached(ui_session: Option<&str>) -> HostTerminal {
    if let Some(host) = read_tmux_host_tag(ui_session) {
        return host;
    }
    if let Some(host) = infer_from_tmux_clients(ui_session) {
        return host;
    }
    detect_from_env(
        env::var("TERM_PROGRAM").ok().as_deref(),
        env::var_os("VSCODE_INJECTION").is_some()
            || env::var_os("VSCODE_PID").is_some()
            || env::var_os("CURSOR_TRACE_ID").is_some(),
    )
}

/// Pure detector for tests.
pub fn detect_from_env(term_program: Option<&str>, vscode_env_present: bool) -> HostTerminal {
    let program = term_program.map(str::trim).filter(|s| !s.is_empty());
    let kind = if is_ide_term_program(program) || vscode_env_present {
        HostKind::Ide
    } else {
        HostKind::Full
    };
    HostTerminal::from_kind(kind, program)
}

/// Call from the **attach** process (has real `TERM_PROGRAM`) before exec attach.
pub fn mark_ui_session_host(ui_session: &str) {
    let host = detect_from_env(
        env::var("TERM_PROGRAM").ok().as_deref(),
        env::var_os("VSCODE_INJECTION").is_some()
            || env::var_os("VSCODE_PID").is_some()
            || env::var_os("CURSOR_TRACE_ID").is_some(),
    );
    let kind = match host.kind {
        HostKind::Ide => "ide",
        HostKind::Full => "full",
    };
    let _ = Command::new("tmux")
        .args(["set-option", "-t", ui_session, UI_HOST_OPTION, kind])
        .output();
    if let Some(prog) = host.term_program {
        let _ = Command::new("tmux")
            .args(["set-option", "-t", ui_session, UI_TERM_PROGRAM_OPTION, prog])
            .output();
    }
}

fn read_tmux_host_tag(ui_session: Option<&str>) -> Option<HostTerminal> {
    let target = ui_session
        .map(str::to_string)
        .or_else(current_tmux_session)?;
    let kind_raw = tmux_show_option(&target, UI_HOST_OPTION)?;
    let kind = match kind_raw.trim() {
        "ide" => HostKind::Ide,
        "full" => HostKind::Full,
        _ => return None,
    };
    let prog = tmux_show_option(&target, UI_TERM_PROGRAM_OPTION);
    Some(HostTerminal::from_kind(kind, prog.as_deref()))
}

fn infer_from_tmux_clients(ui_session: Option<&str>) -> Option<HostTerminal> {
    let session = ui_session
        .map(str::to_string)
        .or_else(current_tmux_session)?;
    let output = Command::new("tmux")
        .args([
            "list-clients",
            "-F",
            "#{client_session}\t#{client_termname}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&output.stdout);
    let mut saw_ide = false;
    let mut saw_full = false;
    let mut prog: Option<&'static str> = None;
    for line in body.lines() {
        let Some((sess, termname)) = line.split_once('\t') else {
            continue;
        };
        if sess != session {
            continue;
        }
        let t = termname.trim().to_ascii_lowercase();
        if t.contains("ghostty") || t.contains("kitty") || t.contains("iterm") {
            saw_full = true;
            prog = Some(if t.contains("ghostty") {
                "ghostty"
            } else if t.contains("kitty") {
                "kitty"
            } else {
                "iTerm.app"
            });
        } else if t == "xterm-256color" || t == "xterm" {
            saw_ide = true;
            if prog.is_none() {
                prog = Some("vscode");
            }
        }
    }
    if saw_full {
        return Some(HostTerminal::from_kind(HostKind::Full, prog));
    }
    if saw_ide {
        return Some(HostTerminal::from_kind(HostKind::Ide, prog));
    }
    None
}

fn current_tmux_session() -> Option<String> {
    env::var_os("TMUX")?;
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

fn tmux_show_option(target: &str, option: &str) -> Option<String> {
    let output = Command::new("tmux")
        .args(["show-options", "-t", target, "-v", option])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!val.is_empty()).then_some(val)
}

fn is_ide_term_program(program: Option<&str>) -> bool {
    matches!(
        program.map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("vscode") | Some("cursor")
    )
}

fn intern_term_program(s: &str) -> &'static str {
    match s {
        "vscode" => "vscode",
        "cursor" => "cursor",
        "ghostty" => "ghostty",
        "iTerm.app" => "iTerm.app",
        "kitty" => "kitty",
        "Apple_Terminal" => "Apple_Terminal",
        "tmux" => "tmux",
        other => Box::leak(other.to_string().into_boxed_str()),
    }
}

/// Right-edge grip width for IDE in-bar resize (VS Code / Cursor xterm.js).
///
/// Wider than a single cell because OSC 22 col-resize is unavailable and the
/// painted `│` is easy to miss under IDE pointer sampling. Content rows still
/// refuse edge-resize start (`mouse_over_list_content_row`), so session/group
/// clicks on the trailing status area keep winning.
pub const EDGE_RESIZE_GRIP_COLS: u16 = 4;
/// Min distance before list text-select engages on full terminals.
pub const LIST_TEXT_SELECT_MIN_DISTANCE: usize = 3;

pub fn is_edge_resize_column(column: u16, pane_width: u16) -> bool {
    if pane_width == 0 {
        return false;
    }
    column.saturating_add(EDGE_RESIZE_GRIP_COLS) >= pane_width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vscode_is_ide() {
        let h = detect_from_env(Some("vscode"), false);
        assert_eq!(h.kind, HostKind::Ide);
        // xterm.js matrix: titles + OSC 11 yes; OSC 22 no.
        assert!(h.supports_osc_title());
        assert!(h.supports_osc11_background());
        assert!(!h.supports_osc22());
        assert!(h.uses_edge_resize());
        assert!(!h.allows_list_text_drag_select());
    }

    #[test]
    fn vscode_env_without_term_program_is_ide() {
        // VS Code integrated terminal always sets VSCODE_* even if TERM_PROGRAM
        // is stripped by an intermediate shell.
        let h = detect_from_env(None, true);
        assert_eq!(h.kind, HostKind::Ide);
        assert!(!h.supports_osc22());
    }

    #[test]
    fn ghostty_is_full() {
        let h = detect_from_env(Some("ghostty"), false);
        assert_eq!(h.kind, HostKind::Full);
        assert!(h.supports_osc22());
        assert!(h.supports_osc_title());
        assert!(h.supports_osc11_background());
        assert!(!h.uses_edge_resize());
        assert!(h.allows_list_text_drag_select());
    }

    #[test]
    fn tmux_term_program_is_full() {
        let h = detect_from_env(Some("tmux"), false);
        assert_eq!(h.kind, HostKind::Full);
    }

    #[test]
    fn edge_grip_rightmost_cols() {
        // Grip is the rightmost EDGE_RESIZE_GRIP_COLS (4) columns.
        assert!(!is_edge_resize_column(48, 53));
        assert!(is_edge_resize_column(49, 53));
        assert!(is_edge_resize_column(50, 53));
        assert!(is_edge_resize_column(51, 53));
        assert!(is_edge_resize_column(52, 53));
    }
}
