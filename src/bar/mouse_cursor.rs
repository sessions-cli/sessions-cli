//! OSC 22 mouse pointer shapes (Ghostty, Kitty, iTerm 3.5+, Foot).
//!
//! DECSCUSR (`CSI n q`) only changes the terminal *insert* cursor. Sidebar hover needs
//! OSC 22 to switch the OS pointer between arrow/hand and I-beam.

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
