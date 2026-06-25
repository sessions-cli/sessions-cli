use crate::bar::notepad::Note;
use super::plan::{DEFAULT_PANE_WIDTH, SESSION_BLOCK_PAD_RIGHT, TOOLBAR_SECTION_PAD};

pub const NOTEPAD_BODY_ROWS: u16 = 12;
pub const MAX_VISIBLE_NOTES: usize = 3;
pub(crate) const NOTEPAD_BODY_PAD_TOP: u16 = 1;

pub fn notepad_text_viewport_rows(expanded: bool) -> u16 {
    if expanded { NOTEPAD_BODY_ROWS.saturating_sub(NOTEPAD_BODY_PAD_TOP) } else { NOTEPAD_BODY_ROWS }
}
pub(crate) fn notepad_body_pad_top(expanded: bool) -> usize {
    if expanded { NOTEPAD_BODY_PAD_TOP as usize } else { 0 }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotepadTrailRow {
    SectionPad, SectionHeader,
    NoteTitle { note_index: usize },
    NotesToggle { expanded: bool, hidden_count: usize },
    NoteBodyPad { note_index: usize },
    NoteBodySlot { note_index: usize, slot: usize },
}

#[derive(Debug, Clone, Copy)]
pub struct NotepadListState<'a> {
    pub notes: &'a [Note], pub section_expanded: bool,
    pub notes_list_expanded: bool, pub active_note_index: Option<usize>,
}
impl<'a> NotepadListState<'a> {
    pub fn trail_layout(&self) -> Vec<NotepadTrailRow> { notepad_trail_layout(self) }
}
pub fn notepad_list_state<'a>(notes: &'a [Note], section_expanded: bool, notes_list_expanded: bool, active_note_index: Option<usize>) -> NotepadListState<'a> {
    NotepadListState { notes, section_expanded, notes_list_expanded, active_note_index }
}
pub fn visible_note_indices(state: &NotepadListState<'_>) -> Vec<usize> {
    let total = state.notes.len();
    if state.notes_list_expanded || total <= MAX_VISIBLE_NOTES { return (0..total).collect(); }
    let mut visible: Vec<usize> = (0..MAX_VISIBLE_NOTES.min(total)).collect();
    if let Some(active) = state.active_note_index {
        if active < total && !visible.contains(&active) {
            if visible.len() >= MAX_VISIBLE_NOTES { visible.pop(); }
            visible.push(active); visible.sort_unstable();
        }
    }
    visible
}
pub fn hidden_note_count(state: &NotepadListState<'_>) -> usize {
    if state.notes_list_expanded || state.notes.len() <= MAX_VISIBLE_NOTES { return 0; }
    state.notes.len().saturating_sub(visible_note_indices(state).len())
}
pub fn default_sidebar_line_width() -> usize {
    (DEFAULT_PANE_WIDTH as usize).saturating_sub(SESSION_BLOCK_PAD_RIGHT as usize)
}
pub fn sidebar_trail_layout(note_state: &NotepadListState<'_>) -> Vec<NotepadTrailRow> { notepad_trail_layout(note_state) }
pub fn sidebar_trailing_row_count(note_state: &NotepadListState<'_>) -> usize { sidebar_trail_layout(note_state).len() }
pub fn sidebar_trail_row_at(trail_idx: usize, note_state: &NotepadListState<'_>) -> Option<NotepadTrailRow> {
    sidebar_trail_layout(note_state).get(trail_idx).cloned()
}
pub fn notepad_trail_layout(state: &NotepadListState<'_>) -> Vec<NotepadTrailRow> {
    let mut layout = Vec::new();
    for _ in 0..TOOLBAR_SECTION_PAD { layout.push(NotepadTrailRow::SectionPad); }
    layout.push(NotepadTrailRow::SectionHeader);
    if state.section_expanded {
        for &note_index in &visible_note_indices(state) {
            let Some(note) = state.notes.get(note_index) else { continue };
            layout.push(NotepadTrailRow::NoteTitle { note_index });
            if note.expanded {
                layout.push(NotepadTrailRow::NoteBodyPad { note_index });
                for slot in 0..NOTEPAD_BODY_ROWS as usize { layout.push(NotepadTrailRow::NoteBodySlot { note_index, slot }); }
            }
        }
        let hidden_count = hidden_note_count(state);
        if hidden_count > 0 { layout.push(NotepadTrailRow::NotesToggle { expanded: state.notes_list_expanded, hidden_count }); }
    }
    layout
}
pub fn notepad_trailing_row_count(state: &NotepadListState<'_>) -> usize { notepad_trail_layout(state).len() }
pub fn notepad_content_rows(state: &NotepadListState<'_>) -> u16 {
    notepad_trailing_row_count(state).saturating_sub(TOOLBAR_SECTION_PAD as usize) as u16
}
pub fn notepad_trail_row_at(trail_idx: usize, state: &NotepadListState<'_>) -> Option<NotepadTrailRow> {
    notepad_trail_layout(state).get(trail_idx).cloned()
}
pub fn notepad_note_title_row_index(note_index: usize, trail_base: usize, state: &NotepadListState<'_>) -> Option<usize> {
    let trail_idx = notepad_trail_layout(state).iter().position(|row| matches!(row, NotepadTrailRow::NoteTitle { note_index: idx } if *idx == note_index))?;
    Some(trail_base.saturating_add(trail_idx))
}
pub fn notepad_note_body_row_range(note_index: usize, trail_base: usize, state: &NotepadListState<'_>) -> Option<(usize, usize)> {
    let layout = notepad_trail_layout(state);
    let start_trail = layout.iter().position(|row| matches!(row, NotepadTrailRow::NoteBodyPad { note_index: idx } if *idx == note_index))?;
    let span = layout[start_trail..].iter().take_while(|row| matches!(row, NotepadTrailRow::NoteBodyPad { note_index: idx } | NotepadTrailRow::NoteBodySlot { note_index: idx, .. } if *idx == note_index)).count();
    Some((trail_base.saturating_add(start_trail), trail_base.saturating_add(start_trail.saturating_add(span))))
}
pub fn visible_session_rows(session_rows: usize, sessions_expanded: bool) -> usize { if sessions_expanded { session_rows } else { 0 } }
pub fn total_list_rows(session_rows: usize, sessions_expanded: bool, note_state: &NotepadListState<'_>) -> usize {
    visible_session_rows(session_rows, sessions_expanded).saturating_add(sidebar_trailing_row_count(note_state))
}
pub fn sidebar_trail_base_row(session_rows: usize, sessions_expanded: bool) -> usize { visible_session_rows(session_rows, sessions_expanded) }
pub fn notes_header_trail_index(note_state: &NotepadListState<'_>) -> usize {
    sidebar_trail_layout(note_state).iter().position(|row| matches!(row, NotepadTrailRow::SectionHeader)).unwrap_or(TOOLBAR_SECTION_PAD as usize)
}
pub fn notepad_header_row_index(session_rows: usize, sessions_expanded: bool, note_state: &NotepadListState<'_>) -> usize {
    sidebar_trail_base_row(session_rows, sessions_expanded).saturating_add(notes_header_trail_index(note_state))
}
pub fn clamp_list_scroll(scroll: usize, total_rows: usize, body_height: usize) -> usize {
    if total_rows <= body_height { 0 } else { scroll.min(total_rows.saturating_sub(body_height)) }
}
pub fn scroll_list_by(scroll: usize, delta: i32, total_rows: usize, body_height: usize) -> usize {
    let max_scroll = total_rows.saturating_sub(body_height); if max_scroll == 0 { return 0; }
    (scroll as i32 + delta).clamp(0, max_scroll as i32) as usize
}
pub fn ensure_range_visible(start: usize, end: usize, scroll: usize, body_height: usize) -> usize {
    if end <= start || body_height == 0 { return scroll; }
    if start < scroll { start } else if end > scroll.saturating_add(body_height) {
        if end.saturating_sub(start) > body_height { start } else { end.saturating_sub(body_height) }
    } else { scroll }
}
pub fn ensure_active_note_visible(scroll: usize, body_height: usize, trail_base: usize, note_state: &NotepadListState<'_>) -> usize {
    let Some(note_index) = note_state.active_note_index else { return scroll };
    let Some(title_row) = notepad_note_title_row_index(note_index, trail_base, note_state) else { return scroll };
    let end_row = if note_state.notes.get(note_index).is_some_and(|n| n.expanded) {
        notepad_note_body_row_range(note_index, trail_base, note_state).map(|(_, e)| e).unwrap_or_else(|| title_row.saturating_add(1))
    } else { title_row.saturating_add(1) };
    ensure_range_visible(title_row, end_row, scroll, body_height)
}
