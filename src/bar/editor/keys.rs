use crate::bar::notepad;

pub fn insert_char(text: &mut String, cursor: &mut usize, ch: char) {
    let pos = notepad::clamp_cursor(text, *cursor);
    let byte_idx = text
        .char_indices()
        .nth(pos)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    text.insert(byte_idx, ch);
    *cursor = pos + 1;
}

pub fn insert_str(text: &mut String, cursor: &mut usize, insert: &str) {
    if insert.is_empty() {
        return;
    }
    let pos = notepad::clamp_cursor(text, *cursor);
    let byte_idx = text
        .char_indices()
        .nth(pos)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    text.insert_str(byte_idx, insert);
    *cursor = pos + insert.chars().count();
}

pub fn backspace(text: &mut String, cursor: &mut usize) -> bool {
    let pos = notepad::clamp_cursor(text, *cursor);
    if pos == 0 {
        return false;
    }
    let start = text
        .char_indices()
        .nth(pos - 1)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let end = text
        .char_indices()
        .nth(pos)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    text.replace_range(start..end, "");
    *cursor = pos - 1;
    true
}

pub fn move_cursor(text: &str, cursor: &mut usize, delta: i32) -> bool {
    let len = text.chars().count();
    let pos = notepad::clamp_cursor(text, *cursor) as i32;
    let next = (pos + delta).clamp(0, len as i32) as usize;
    if next != *cursor {
        *cursor = next;
        true
    } else {
        false
    }
}

pub fn scroll_lines(scroll: &mut usize, delta: i32, max_scroll: usize) -> bool {
    let next = (*scroll as i32 + delta).clamp(0, max_scroll as i32) as usize;
    if next != *scroll {
        *scroll = next;
        true
    } else {
        false
    }
}
