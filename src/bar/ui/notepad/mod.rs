use super::theme::*;
use super::widgets::{chrome_row_prefix, empty_trailing_slot, format_trailing_slot, full_width_line, full_width_spans, group_add_badge_style, notepad_scrollbar_track_x, rename_targets_note, row_label_width, row_label_width_after_prefix, row_with_trailing_slot, truncate, NotepadScrollbar, RenameState, CHROME_ROW_PREFIX, GROUP_ADD_ICON, ROW_LABEL_OFFSET, ROW_PRE_TRAILING_GAP, TRAILING_SLOT_WIDTH};
use super::sessions::{apply_group_highlight, GroupHighlight};
use super::{default_sidebar_line_width, is_group_add_click, notepad_note_body_row_range, pointer_in_list_body, sidebar_trail_layout, sidebar_trail_row_at, terminal_list_area, LayoutMetrics, NotepadListState, NotepadTrailRow};
use chrono::{DateTime, Utc};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use ratatui::Frame;
use std::collections::HashSet;

pub use super::layout::{MAX_VISIBLE_NOTES, NOTEPAD_BODY_ROWS};

pub(crate) const NOTEPAD_HEADER_ROWS: u16 = 1;

pub(crate) const NOTEPAD_BODY_PAD_TOP: u16 = 1;

pub(crate) const NOTEPAD_BODY_INDENT: &str = "   ";

pub use super::layout::notepad_text_viewport_rows;

fn notepad_body_pad_top(expanded: bool) -> usize {
    if expanded {
        NOTEPAD_BODY_PAD_TOP as usize
    } else {
        0
    }
}

pub(crate) const NOTEPAD_NOTE_TITLE_OFFSET: usize = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoteDragState {
    pub source: Option<String>,
    pub hover: Option<String>,
    /// True once the pointer moves during a press — distinguishes click-to-expand from drag.
    pub dragged: bool,
    pub preserved_active_note_id: Option<String>,
    pub pending_click_note_id: Option<String>,
    pub pressed_at: Option<std::time::Instant>,
    pub pressed_row: Option<u16>,
}

impl NoteDragState {
    pub fn active(&self) -> bool {
        self.source.is_some()
    }

    pub fn pending(&self) -> bool {
        self.pending_click_note_id.is_some() && self.source.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSection {
    pub note_id: String,
    pub start: usize,
    pub end: usize,
}

pub fn note_sections(state: &NotepadListState<'_>) -> Vec<NoteSection> {
    let layout = sidebar_trail_layout(state);
    let mut sections = Vec::new();
    let mut idx = 0;
    while idx < layout.len() {
        let NotepadTrailRow::NoteTitle { note_index } = layout[idx] else {
            idx += 1;
            continue;
        };
        let Some(note_id) = state.notes.get(note_index).map(|note| note.id.clone()) else {
            idx += 1;
            continue;
        };
        let start = idx;
        idx += 1;
        while idx < layout.len() {
            match &layout[idx] {
                NotepadTrailRow::NoteBodyPad { note_index: body_idx }
                | NotepadTrailRow::NoteBodySlot {
                    note_index: body_idx, ..
                } if *body_idx == note_index => idx += 1,
                _ => break,
            }
        }
        sections.push(NoteSection {
            note_id,
            start,
            end: idx.saturating_sub(1),
        });
    }
    sections
}

fn note_section_index_for_trail(sections: &[NoteSection], trail_row_idx: usize) -> Option<usize> {
    sections
        .iter()
        .position(|section| trail_row_idx >= section.start && trail_row_idx <= section.end)
}

pub fn note_drag_target(
    state: &NotepadListState<'_>,
    trail_row_idx: usize,
    source_id: &str,
) -> Option<String> {
    let sections = note_sections(state);
    let source_idx = sections
        .iter()
        .position(|section| section.note_id == source_id)?;
    let target_idx = note_section_index_for_trail(&sections, trail_row_idx)?;
    let target = &sections[target_idx];
    if target.note_id == source_id {
        return Some(source_id.to_string());
    }

    if trail_row_idx == target.start {
        return Some(target.note_id.clone());
    }

    let height = target.end.saturating_sub(target.start) + 1;
    let offset = trail_row_idx.saturating_sub(target.start);
    let in_lower_half = offset > height.saturating_sub(1) / 2;

    if source_idx < target_idx {
        if in_lower_half {
            Some(target.note_id.clone())
        } else {
            Some(source_id.to_string())
        }
    } else if in_lower_half {
        Some(source_id.to_string())
    } else {
        Some(target.note_id.clone())
    }
}

pub(crate) fn note_section_highlight(
    sections: &[NoteSection],
    trail_row_idx: usize,
    note_drag: &NoteDragState,
) -> Option<GroupHighlight> {
    if !note_drag.active() {
        return None;
    }
    let section = sections.get(note_section_index_for_trail(sections, trail_row_idx)?)?;
    if note_drag.source.as_deref() == Some(section.note_id.as_str()) {
        return Some(GroupHighlight::Source);
    }
    None
}

pub(crate) fn notepad_note_title_prefix_drag(
    editing: bool,
    is_close_target: bool,
    highlight: Option<GroupHighlight>,
) -> String {
    match highlight {
        Some(GroupHighlight::Source) => " ⠿ ".to_string(),
        Some(GroupHighlight::Target) => "│  ".to_string(),
        None => notepad_note_title_prefix(editing, is_close_target),
    }
}

pub(crate) fn notepad_note_title_row_style_drag(
    editing: bool,
    is_active: bool,
    is_hovered: bool,
    is_close_target: bool,
    close_modifier_held: bool,
    highlight: Option<GroupHighlight>,
) -> Style {
    let base = notepad_note_title_row_style_in_list(
        editing,
        is_active,
        is_hovered,
        is_close_target,
        close_modifier_held,
    );
    apply_group_highlight(base, highlight)
}

pub fn notepad_note_body_visible_rect(
    terminal_area: Rect,
    scroll: usize,
    body_height: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
    note_index: usize,
) -> Option<Rect> {
    if !state.section_expanded {
        return None;
    }
    let (body_start, body_end) =
        notepad_note_body_row_range(note_index, visible_sessions, state)?;
    let visible_end = scroll.saturating_add(body_height);
    let overlap_start = body_start.max(scroll);
    let overlap_end = body_end.min(visible_end);
    if overlap_start >= overlap_end {
        return None;
    }
    Some(Rect {
        x: terminal_area.x,
        y: terminal_area
            .y
            .saturating_add((overlap_start.saturating_sub(scroll)) as u16),
        width: terminal_area.width,
        height: (overlap_end - overlap_start) as u16,
    })
}

pub fn render_notepad_body_padding_backdrop(
    frame: &mut Frame,
    body_area: Rect,
    terminal_area: Rect,
    body: Rect,
) {
    let gutter_x = terminal_area.x.saturating_add(terminal_area.width);
    let gutter_w = body_area
        .x
        .saturating_add(body_area.width)
        .saturating_sub(gutter_x);
    if gutter_w == 0 || body.height == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(NOTEPAD_EDIT_BG)),
        Rect {
            x: gutter_x,
            y: body.y,
            width: gutter_w,
            height: body.height,
        },
    );
}

pub fn notepad_note_title_prefix(editing: bool, is_close_target: bool) -> String {
    if editing {
        format!("{}✎ ", CHROME_ROW_PREFIX)
    } else if is_close_target {
        // ✕ in column 1 — same column as session close-target rows (" ✕" lead).
        " ✕ ".to_string()
    } else {
        " ".repeat(NOTEPAD_NOTE_TITLE_OFFSET)
    }
}

pub fn notepad_note_title_row_style_in_list(
    editing: bool,
    is_active: bool,
    is_hovered: bool,
    is_close_target: bool,
    close_modifier_held: bool,
) -> Style {
    if editing {
        return notepad_note_title_row_style(editing, is_active, is_hovered);
    }
    if is_close_target {
        return Style::default()
            .fg(CLOSE_HOVER_FG)
            .bg(CLOSE_HOVER_BG)
            .add_modifier(Modifier::BOLD);
    }
    if close_modifier_held {
        return Style::default()
            .fg(CLOSE_MODE_FG)
            .bg(BG_BASE)
            .remove_modifier(Modifier::BOLD);
    }
    notepad_note_title_row_style(editing, is_active, is_hovered)
}

pub fn notepad_note_title_backdrop_bg_in_list(
    note_index: usize,
    notepad_state: &NotepadListState<'_>,
    note_hover: Option<usize>,
    rename: Option<&RenameState>,
    is_close_target: bool,
    close_modifier_held: bool,
) -> Option<Color> {
    if is_close_target {
        return Some(CLOSE_HOVER_BG);
    }
    if close_modifier_held {
        return None;
    }
    notepad_note_title_backdrop_bg(note_index, notepad_state, note_hover, rename)
}

pub fn notepad_note_title_line(
    prefix: String,
    label: &str,
    line_width: usize,
    row_style: Style,
) -> Line<'static> {
    let prefix_width = prefix.chars().count();
    let label_width = line_width.saturating_sub(prefix_width);
    let title = truncate(label, label_width);
    let title_len = title.chars().count();
    full_width_spans(
        vec![
            Span::styled(prefix, row_style),
            Span::styled(title, row_style),
            Span::styled(
                " ".repeat(label_width.saturating_sub(title_len)),
                row_style,
            ),
        ],
        line_width,
        row_style,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotepadHit {
    SectionHeader,
    SectionAdd,
    NoteTitle { note_index: usize },
    NotesToggle,
    NoteBody { note_index: usize },
    NoteBodyScrollbar { note_index: usize },
}

pub fn notepad_content_width(line_width: usize, expanded: bool) -> usize {
    let indent_len = NOTEPAD_BODY_INDENT.chars().count();
    let trailing_gap = usize::from(expanded);
    line_width.saturating_sub(indent_len + trailing_gap)
}

pub fn notepad_scrollbar_column(metrics: &LayoutMetrics) -> u16 {
    notepad_scrollbar_track_x(terminal_list_area(metrics))
}

pub fn notepad_scroll_metrics(
    text: &str,
    line_width: usize,
    expanded: bool,
) -> (usize, usize, usize) {
    let content_width = notepad_content_width(line_width, expanded);
    let total_lines = crate::bar::notepad::wrapped_display_lines(text, content_width).len();
    let viewport_rows = notepad_text_viewport_rows(expanded) as usize;
    let max_scroll = total_lines.saturating_sub(viewport_rows);
    (total_lines, viewport_rows, max_scroll)
}

pub fn notepad_scrollbar_geometry(
    terminal_area: Rect,
    list_scroll: usize,
    body_height: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
    note_index: usize,
    text: &str,
    body_scroll: usize,
    line_width: usize,
) -> Option<NotepadScrollbar> {
    let body = notepad_note_body_visible_rect(
        terminal_area,
        list_scroll,
        body_height,
        visible_sessions,
        state,
        note_index,
    )?;
    let (total_lines, viewport_rows, max_scroll) = notepad_scroll_metrics(text, line_width, true);
    if max_scroll == 0 {
        return None;
    }
    let track_x = notepad_scrollbar_track_x(terminal_area);
    let track = Rect {
        x: track_x,
        y: body.y,
        width: 1,
        height: body.height,
    };
    let thumb_height = (body.height as usize * viewport_rows)
        .div_ceil(total_lines)
        .max(1)
        .min(body.height as usize) as u16;
    let thumb_travel = body.height.saturating_sub(thumb_height);
    let thumb_y = body
        .y
        .saturating_add(((body_scroll as u32 * thumb_travel as u32) / max_scroll as u32) as u16);
    let thumb = Rect {
        x: track_x,
        y: thumb_y,
        width: 1,
        height: thumb_height,
    };
    Some(NotepadScrollbar { track, thumb })
}

pub fn notepad_header_label(expanded: bool) -> String {
    let chevron = if expanded { "▾" } else { "▸" };
    format!("{chevron} notes")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveTimeUnit {
    Minutes,
    Hours,
    Days,
    Weeks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveTimeAgoParts {
    pub value: u64,
    pub unit: SaveTimeUnit,
}

impl SaveTimeAgoParts {
    pub fn from_timestamp(last_saved_at: DateTime<Utc>) -> Self {
        let secs = (Utc::now() - last_saved_at).num_seconds().max(0);
        if secs < 3600 {
            Self {
                value: (secs / 60).max(1) as u64,
                unit: SaveTimeUnit::Minutes,
            }
        } else if secs < 86_400 {
            Self {
                value: (secs / 3600) as u64,
                unit: SaveTimeUnit::Hours,
            }
        } else if secs < 604_800 {
            Self {
                value: (secs / 86_400) as u64,
                unit: SaveTimeUnit::Days,
            }
        } else {
            Self {
                value: (secs / 604_800) as u64,
                unit: SaveTimeUnit::Weeks,
            }
        }
    }

    pub fn unit_label(self) -> &'static str {
        match self.unit {
            SaveTimeUnit::Minutes => "m",
            SaveTimeUnit::Hours => "hr",
            SaveTimeUnit::Days => "d",
            SaveTimeUnit::Weeks => "wk",
        }
    }

    pub fn format(self) -> String {
        format!("{}{} ago", self.value, self.unit_label())
    }
}

pub fn format_save_time_ago(last_saved_at: DateTime<Utc>) -> String {
    SaveTimeAgoParts::from_timestamp(last_saved_at).format()
}

pub fn notepad_save_status_text(last_saved_at: Option<DateTime<Utc>>) -> Option<String> {
    last_saved_at.map(|at| format!("saved {}", format_save_time_ago(at)))
}

pub fn notepad_save_time_label(last_saved_at: DateTime<Utc>) -> String {
    format_save_time_ago(last_saved_at)
}

pub fn notepad_section_header_line(
    line_width: usize,
    section_expanded: bool,
    section_header_hover: bool,
    section_add_hover: bool,
    last_saved_at: Option<DateTime<Utc>>,
) -> Line<'static> {
    let header_style = if section_header_hover {
        Style::default().fg(BRAND_FG).bg(BG_HIGHLIGHT)
    } else {
        Style::default().fg(BRAND_FG).bg(BG_BASE)
    };
    let prefix = chrome_row_prefix();
    let title = notepad_header_label(section_expanded);
    let show_add = section_header_hover || section_add_hover;
    let (trailing, trailing_style) = if show_add {
        (
            format_trailing_slot(GROUP_ADD_ICON),
            group_add_badge_style(header_style),
        )
    } else {
        empty_trailing_slot(header_style)
    };
    let prefix_width = prefix.chars().count();
    let title_len = title.chars().count();
    let trailing_area = ROW_PRE_TRAILING_GAP + TRAILING_SLOT_WIDTH;
    let label_width = line_width
        .saturating_sub(prefix_width)
        .saturating_sub(trailing_area);
    let mut label_spans = vec![Span::styled(title.clone(), header_style)];
    if let Some(at) = last_saved_at {
        let static_suffix = " — saved ";
        let parts = SaveTimeAgoParts::from_timestamp(at);
        let full_time_label = parts.format();
        let suffix_budget = label_width.saturating_sub(title_len);
        let static_suffix = truncate(static_suffix, suffix_budget);
        let static_suffix_len = static_suffix.chars().count();
        if static_suffix_len > 0 {
            label_spans.push(Span::styled(static_suffix, header_style));
            let time_budget = suffix_budget.saturating_sub(static_suffix_len);
            let time_label = truncate(&full_time_label, time_budget);
            if !time_label.is_empty() {
                label_spans.push(Span::styled(time_label, header_style));
            }
        }
    }
    let label_used: usize = label_spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum();
    let pad = label_width.saturating_sub(label_used);
    let mut spans = vec![Span::styled(prefix, header_style)];
    spans.extend(label_spans);
    spans.push(Span::styled(" ".repeat(pad), header_style));
    spans.push(Span::styled(" ".repeat(ROW_PRE_TRAILING_GAP), header_style));
    spans.push(Span::styled(trailing, trailing_style));
    Line::from(spans)
}

fn notepad_char_selected(abs: usize, selection: Option<(usize, usize)>) -> bool {
    selection.is_some_and(|(start, end)| start < end && start <= abs && abs < end)
}

pub fn notepad_body_line_spans(
    line: &str,
    line_start: usize,
    content_width: usize,
    body_fg: Color,
    body_bg: Color,
    select_fg: Color,
    select_bg: Color,
    selection: Option<(usize, usize)>,
) -> Vec<Span<'static>> {
    body_line_spans(
        line,
        line_start,
        content_width,
        body_fg,
        body_bg,
        select_fg,
        select_bg,
        selection,
        NOTEPAD_BODY_INDENT,
    )
}

pub(crate) fn body_line_spans(
    line: &str,
    line_start: usize,
    content_width: usize,
    body_fg: Color,
    body_bg: Color,
    select_fg: Color,
    select_bg: Color,
    selection: Option<(usize, usize)>,
    indent: &str,
) -> Vec<Span<'static>> {
    let normal = Style::default().fg(body_fg).bg(body_bg);
    let selected = Style::default().fg(select_fg).bg(select_bg);
    let mut spans = Vec::new();
    if !indent.is_empty() {
        spans.push(Span::styled(indent.to_string(), normal));
    }
    if content_width == 0 {
        return spans;
    }

    let mut run = String::new();
    let mut run_style = normal;

    let flush = |run: &mut String, style: Style, spans: &mut Vec<Span<'static>>| {
        if !run.is_empty() {
            spans.push(Span::styled(std::mem::take(run), style));
        }
    };

    for (col, ch) in line.chars().enumerate() {
        let abs = line_start + col;
        let style = if notepad_char_selected(abs, selection) {
            selected
        } else {
            normal
        };
        if style != run_style {
            flush(&mut run, run_style, &mut spans);
            run_style = style;
        }
        run.push(ch);
    }

    flush(&mut run, run_style, &mut spans);
    spans
}

pub fn notepad_terminal_cursor_position(
    terminal_area: Rect,
    scroll: usize,
    body_height: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
    note_index: usize,
    notepad_focused: bool,
    text: &str,
    cursor: usize,
    body_scroll: usize,
    line_width: usize,
) -> Option<Position> {
    if !state.section_expanded || !notepad_focused {
        return None;
    }
    let (body_start, body_end) =
        notepad_note_body_row_range(note_index, visible_sessions, state)?;
    let content_width = notepad_content_width(line_width, true);
    let wrapped = crate::bar::notepad::wrapped_display_lines(text, content_width);
    let cursor_line = crate::bar::notepad::display_line_index(text, cursor, content_width);
    let viewport_rows = notepad_text_viewport_rows(true) as usize;
    if cursor_line < body_scroll || cursor_line >= body_scroll.saturating_add(viewport_rows) {
        return None;
    }
    let display_line = wrapped.get(cursor_line)?;
    let body_slot = cursor_line
        .saturating_sub(body_scroll)
        .saturating_add(notepad_body_pad_top(true));
    let list_row_idx = body_start.saturating_add(body_slot);
    if list_row_idx >= body_end || list_row_idx < scroll || list_row_idx >= scroll.saturating_add(body_height)
    {
        return None;
    }
    let col_in_line = cursor.saturating_sub(display_line.start);
    let indent_len = NOTEPAD_BODY_INDENT.chars().count() as u16;
    let visible_idx = list_row_idx.saturating_sub(scroll);
    Some(Position::new(
        terminal_area
            .x
            .saturating_add(indent_len.saturating_add(col_in_line as u16)),
        terminal_area.y.saturating_add(visible_idx as u16),
    ))
}

pub fn notepad_scroll_for_cursor(scroll: usize, cursor_line: usize, body_rows: u16) -> usize {
    let body_rows = body_rows as usize;
    if body_rows == 0 {
        return 0;
    }
    if cursor_line < scroll {
        cursor_line
    } else if cursor_line >= scroll + body_rows {
        cursor_line + 1 - body_rows
    } else {
        scroll
    }
}

pub fn notepad_line_from_mouse(
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
    note_index: usize,
    body_scroll: usize,
) -> Option<usize> {
    let hit_note_index = match notepad_hit_from_mouse(0, y, metrics, scroll, visible_sessions, state)
    {
        Some(NotepadHit::NoteBody { note_index }) => note_index,
        _ => return None,
    };
    if hit_note_index != note_index {
        return None;
    }
    let rel = (y - metrics.list_top_y) as usize;
    let row_idx = scroll.saturating_add(rel);
    let (body_start, body_end) =
        notepad_note_body_row_range(note_index, visible_sessions, state)?;
    if row_idx < body_start || row_idx >= body_end {
        return None;
    }
    let body_slot = row_idx.saturating_sub(body_start);
    let pad_top = notepad_body_pad_top(true);
    if body_slot < pad_top {
        return Some(body_scroll);
    }
    Some(body_scroll.saturating_add(body_slot.saturating_sub(pad_top)))
}

fn notepad_col_from_mouse(column: u16, metrics: &LayoutMetrics) -> usize {
    let indent_len = NOTEPAD_BODY_INDENT.chars().count();
    column
        .saturating_sub(metrics.list_inner_x)
        .saturating_sub(indent_len as u16) as usize
}

pub fn notepad_cursor_from_mouse(
    column: u16,
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
    note_index: usize,
    text: &str,
    body_scroll: usize,
) -> Option<usize> {
    let line_idx = notepad_line_from_mouse(
        y,
        metrics,
        scroll,
        visible_sessions,
        state,
        note_index,
        body_scroll,
    )?;
    let text_col = notepad_col_from_mouse(column, metrics);
    let content_width = notepad_content_width(metrics.list_line_width, true);
    let wrapped = crate::bar::notepad::wrapped_display_lines(text, content_width);
    let display_line = wrapped.get(line_idx)?;
    let col = text_col.min(display_line.text.chars().count());
    Some(display_line.start + col)
}

pub fn notepad_selection_cursor_from_mouse(
    column: u16,
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
    note_index: usize,
    text: &str,
    body_scroll: usize,
) -> Option<usize> {
    if !metrics.notepad_expanded {
        return None;
    }
    if let Some(cursor) = notepad_cursor_from_mouse(
        column,
        y,
        metrics,
        scroll,
        visible_sessions,
        state,
        note_index,
        text,
        body_scroll,
    ) {
        return Some(cursor);
    }
    if !pointer_in_list_body(column, metrics) {
        return None;
    }
    if y < metrics.list_top_y {
        return None;
    }
    let rel = (y - metrics.list_top_y) as usize;
    if rel >= metrics.list_height {
        return None;
    }
    let row_idx = scroll.saturating_add(rel);
    let (body_start, body_end) =
        notepad_note_body_row_range(note_index, visible_sessions, state)?;
    let content_width = notepad_content_width(metrics.list_line_width, true);
    let wrapped = crate::bar::notepad::wrapped_display_lines(text, content_width);
    if row_idx < body_start {
        let first = wrapped.first()?;
        let col = notepad_col_from_mouse(column, metrics).min(first.text.chars().count());
        return Some(first.start + col);
    }
    if row_idx >= body_end {
        let last = wrapped.last()?;
        let col = notepad_col_from_mouse(column, metrics).min(last.text.chars().count());
        return Some(last.start + col);
    }
    None
}

pub fn notepad_hit_from_mouse(
    column: u16,
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    trail_base: usize,
    state: &NotepadListState<'_>,
) -> Option<NotepadHit> {
    if y < metrics.list_top_y {
        return None;
    }
    let rel = (y - metrics.list_top_y) as usize;
    if rel >= metrics.list_height {
        return None;
    }
    let row_idx = scroll.saturating_add(rel);
    if row_idx < trail_base {
        return None;
    }
    let trail_idx = row_idx.saturating_sub(trail_base);
    let trail_row = sidebar_trail_row_at(trail_idx, state)?;
    match trail_row {
        NotepadTrailRow::SectionHeader => {
            if is_group_add_click(column, metrics) {
                Some(NotepadHit::SectionAdd)
            } else {
                Some(NotepadHit::SectionHeader)
            }
        }
        NotepadTrailRow::NoteTitle { note_index } => {
            Some(NotepadHit::NoteTitle { note_index })
        }
        NotepadTrailRow::NotesToggle { .. } => Some(NotepadHit::NotesToggle),
        NotepadTrailRow::NoteBodyPad { note_index } | NotepadTrailRow::NoteBodySlot { note_index, .. } => {
            if column == notepad_scrollbar_column(metrics) {
                Some(NotepadHit::NoteBodyScrollbar { note_index })
            } else {
                Some(NotepadHit::NoteBody { note_index })
            }
        }
        NotepadTrailRow::SectionPad => None,
    }
}

pub fn notepad_scrollable_hit(
    column: u16,
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
) -> bool {
    matches!(
        notepad_hit_from_mouse(column, y, metrics, scroll, visible_sessions, state),
        Some(NotepadHit::NoteBody { .. } | NotepadHit::NoteBodyScrollbar { .. })
    )
}

pub fn notepad_section_header_hover_from_mouse(
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
) -> bool {
    matches!(
        notepad_hit_from_mouse(0, y, metrics, scroll, visible_sessions, state),
        Some(NotepadHit::SectionHeader | NotepadHit::SectionAdd)
    )
}

pub fn notepad_section_add_hover_from_mouse(
    column: u16,
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
) -> bool {
    is_group_add_click(column, metrics)
        && matches!(
            notepad_hit_from_mouse(column, y, metrics, scroll, visible_sessions, state),
            Some(NotepadHit::SectionHeader | NotepadHit::SectionAdd)
        )
}

pub fn notepad_note_hover_from_mouse(
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
) -> Option<usize> {
    match notepad_hit_from_mouse(0, y, metrics, scroll, visible_sessions, state) {
        Some(NotepadHit::NoteTitle { note_index }) => Some(note_index),
        _ => None,
    }
}

pub fn notepad_note_title_row_from_mouse(
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
) -> Option<usize> {
    notepad_note_hover_from_mouse(y, metrics, scroll, visible_sessions, state)
}

pub fn notepad_section_add_click(
    column: u16,
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    visible_sessions: usize,
    state: &NotepadListState<'_>,
) -> bool {
    matches!(
        notepad_hit_from_mouse(column, y, metrics, scroll, visible_sessions, state),
        Some(NotepadHit::SectionAdd)
    )
}

pub fn notepad_note_title_row_bg(
    editing: bool,
    is_active: bool,
    is_hovered: bool,
) -> Option<Color> {
    if editing {
        Some(BG_SELECTED)
    } else if is_active && is_hovered {
        Some(BG_HOVER_SELECTED)
    } else if is_active {
        Some(BG_SELECTED)
    } else if is_hovered {
        Some(BG_HIGHLIGHT)
    } else {
        None
    }
}

pub fn notepad_note_title_row_style(
    editing: bool,
    is_active: bool,
    is_hovered: bool,
) -> Style {
    match notepad_note_title_row_bg(editing, is_active, is_hovered) {
        Some(BG_SELECTED) if editing => Style::default().fg(TEXT_PRIMARY).bg(BG_SELECTED),
        Some(bg) => Style::default().fg(TEXT_SELECTED).bg(bg),
        None => Style::default().fg(TEXT_PRIMARY).bg(BG_BASE),
    }
}

pub fn notepad_note_title_backdrop_bg(
    note_index: usize,
    notepad_state: &NotepadListState<'_>,
    note_hover: Option<usize>,
    rename: Option<&RenameState>,
) -> Option<Color> {
    let note = notepad_state.notes.get(note_index)?;
    let editing = rename.is_some_and(|rename| rename_targets_note(rename, &note.id));
    let is_active = notepad_state.active_note_index == Some(note_index);
    let is_hovered = note_hover == Some(note_index);
    notepad_note_title_row_bg(editing, is_active, is_hovered)
}

pub fn row_is_note_title(
    row_idx: usize,
    trail_base: usize,
    note_state: &NotepadListState<'_>,
    line_width: usize,
) -> bool {
    if row_idx < trail_base {
        return false;
    }
    let trail_idx = row_idx.saturating_sub(trail_base);
    matches!(
        sidebar_trail_row_at(trail_idx, note_state),
        Some(NotepadTrailRow::NoteTitle { .. })
    )
}

pub fn note_close_target_row(
    row_idx: usize,
    trail_base: usize,
    close_modifier_held: bool,
    close_target: Option<usize>,
    note_state: &NotepadListState<'_>,
    line_width: usize,
) -> bool {
    close_modifier_held
        && close_target == Some(row_idx)
        && row_is_note_title(row_idx, trail_base, note_state, line_width)
}
