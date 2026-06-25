use crate::bar::ui::{self, NotepadScrollbar};

pub fn thumb_grab_offset(mouse_row: u16, thumb_y: u16) -> u16 {
    mouse_row.saturating_sub(thumb_y)
}

pub fn thumb_hit(scrollbar: &NotepadScrollbar, mouse_row: u16) -> bool {
    ui::notepad_scrollbar_thumb_hit(scrollbar, mouse_row)
}

pub fn scroll_from_track_click(
    mouse_row: u16,
    scrollbar: &NotepadScrollbar,
    max_scroll: usize,
) -> Option<usize> {
    let target = ui::notepad_scroll_from_track_click(mouse_row, scrollbar, max_scroll);
    (target != usize::MAX).then_some(target)
}

pub fn scroll_from_thumb_drag(
    mouse_row: u16,
    scrollbar: &NotepadScrollbar,
    max_scroll: usize,
    grab_offset: u16,
) -> usize {
    ui::notepad_scroll_from_thumb_drag(mouse_row, scrollbar, max_scroll, grab_offset)
}