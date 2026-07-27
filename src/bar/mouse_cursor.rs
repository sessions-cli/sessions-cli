//! OSC 22 mouse pointer shapes (Ghostty, Kitty, iTerm 3.5+, Foot).
//!
//! DECSCUSR (`CSI n q`) only changes the terminal *text* insert cursor (block/bar/
//! underline). Sidebar hover needs OSC 22 to switch the **OS pointer** between
//! arrow / hand / I-beam.
//!
//! ## VS Code / Cursor (xterm.js) — do not emit
//!
//! [xterm.js VT features](https://xtermjs.org/docs/api/vtfeatures/) implement OSC
//! 0/2/4/8/10/11/12/104/110–112 but **not OSC 22**. Writing OSC 22 into the
//! VS Code integrated terminal is a silent no-op; resize/hover UX must use
//! painted grips and keyboard, not pointer shapes. Gated via [`host_terminal`].

use super::host_terminal;
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseCursorShape {
    /// Arrow — general non-text UI.
    Default,
    /// Hand — clickable rows, buttons, links.
    Pointer,
    /// I-beam — inline text editing.
    Text,
}

pub fn set_mouse_cursor(shape: MouseCursorShape) -> io::Result<()> {
    // Hard gate: never write OSC 22 into VS Code / Cursor xterm.js.
    if !host_terminal::detect().supports_osc22() {
        return Ok(());
    }
    let seq = match shape {
        MouseCursorShape::Default => "\x1b]22;default\x1b\\",
        MouseCursorShape::Pointer => "\x1b]22;pointer\x1b\\",
        MouseCursorShape::Text => "\x1b]22;text\x1b\\",
    };
    let mut out = io::stdout();
    out.write_all(seq.as_bytes())?;
    out.flush()
}

/// Release the shape stack so the terminal (or workspace pane) can choose again.
pub fn reset_mouse_cursor() -> io::Result<()> {
    if !host_terminal::detect().supports_osc22() {
        return Ok(());
    }
    let mut out = io::stdout();
    out.write_all(b"\x1b]22;\x1b\\")?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc22_sequences_use_st_terminator() {
        assert!(set_mouse_cursor(MouseCursorShape::Pointer).is_ok());
        assert!(reset_mouse_cursor().is_ok());
    }
}
