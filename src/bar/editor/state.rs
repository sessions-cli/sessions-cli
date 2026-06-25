use crate::bar::notepad;

/// Shared text-editing state for notepad body and todo description fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextEditor {
    pub cursor: usize,
    pub scroll: usize,
    pub selection: Option<(usize, usize)>,
    pub scrollbar_thumb_offset: Option<u16>,
    pub select_anchor: Option<usize>,
    pub drag_selecting: bool,
}

impl TextEditor {
    pub fn for_text(text: &str) -> Self {
        Self {
            cursor: notepad::clamp_cursor(text, text.chars().count()),
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn clear_interaction(&mut self) {
        self.selection = None;
        self.select_anchor = None;
        self.drag_selecting = false;
        self.scrollbar_thumb_offset = None;
    }

    pub fn is_interacting(&self) -> bool {
        self.drag_selecting || self.scrollbar_thumb_offset.is_some()
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some_and(|(start, end)| start < end)
    }

    pub fn set_cursor_clamped(&mut self, text: &str, cursor: usize) {
        self.cursor = notepad::clamp_cursor(text, cursor);
    }
}