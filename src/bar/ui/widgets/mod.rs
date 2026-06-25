use super::theme::*;
use super::notepad::NOTEPAD_NOTE_TITLE_OFFSET;
use super::SESSION_BLOCK_PAD_RIGHT;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use crate::model::AgentState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuAction {
    Rename,
    Delete,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuTarget {
    Session { session_id: String },
    Group { cwd_label: String },
    Note { note_id: String },
    Notepad { has_selection: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenu {
    pub target: ContextMenuTarget,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameTarget {
    Session { session_id: String },
    Note { note_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameState {
    pub target: RenameTarget,
    pub row_idx: usize,
    pub buffer: String,
    /// True on entry and after Ctrl+A — first keystroke replaces the whole title.
    pub select_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteNoteConfirmState {
    pub note_id: String,
    pub title: String,
    pub buffer: String,
}

pub const DELETE_NOTE_CONFIRM_WORD: &str = "yes";

pub fn delete_note_confirm_ready(state: &DeleteNoteConfirmState) -> bool {
    state.buffer == DELETE_NOTE_CONFIRM_WORD
}

pub fn delete_note_confirm_apply_char(state: &mut DeleteNoteConfirmState, c: char) {
    if state.buffer.chars().count() < DELETE_NOTE_CONFIRM_WORD.len() {
        state.buffer.push(c);
    }
}

pub fn delete_note_confirm_apply_backspace(state: &mut DeleteNoteConfirmState) {
    state.buffer.pop();
}

pub fn rename_apply_char(rename: &mut RenameState, c: char) {
    if rename.select_all {
        rename.buffer = c.to_string();
        rename.select_all = false;
        return;
    }
    if rename.buffer.chars().count() < 80 {
        rename.buffer.push(c);
    }
}

pub fn rename_apply_backspace(rename: &mut RenameState) {
    if rename.select_all {
        rename.buffer.clear();
    } else {
        rename.buffer.pop();
    }
    rename.select_all = false;
}

pub fn rename_deselect(rename: &mut RenameState) {
    rename.select_all = false;
}

pub fn rename_apply_paste(rename: &mut RenameState, text: &str) {
    if rename.select_all {
        rename.buffer.clear();
        rename.select_all = false;
    }
    for ch in text.chars() {
        if rename.buffer.chars().count() >= 80 {
            break;
        }
        rename.buffer.push(ch);
    }
}

pub(crate) const CONTEXT_MENU_ITEM_HEIGHT: u16 = 1;

const NOTEPAD_MENU_WIDTH: u16 = 26;

const NOTEPAD_MENU_INNER_WIDTH: usize = 24;

const NOTEPAD_MENU_BORDER_FG: Color = Color::Rgb(56, 56, 56);

pub(crate) fn notepad_scrollbar_track_x(terminal_area: Rect) -> u16 {
    terminal_area
        .x
        .saturating_add(terminal_area.width)
        .saturating_add(SESSION_BLOCK_PAD_RIGHT.saturating_sub(1))
}

const NOTEPAD_SCROLL_THUMB_FG: Color = PATH_FG;

const NOTEPAD_SCROLL_THUMB_ACTIVE_FG: Color = TEXT_SECONDARY;

pub(crate) const TRAILING_SLOT_WIDTH: usize = 3;

pub(crate) const ROW_LABEL_OFFSET: usize = 6;

pub(crate) const CHROME_ROW_PREFIX: &str = " ";

pub(crate) const ROW_PRE_TRAILING_GAP: usize = 2;

pub(crate) const GROUP_ADD_ICON: &str = "[+]";

const SCROLLBAR_THUMB_GLYPH: &str = "▌";

fn render_vertical_bar_overlay(frame: &mut Frame, rect: Rect, glyph: &str, style: Style) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let buf = frame.buffer_mut();
    let area = buf.area;
    for row in 0..rect.height {
        let y = rect.y.saturating_add(row);
        if y >= area.height {
            break;
        }
        for col in 0..rect.width {
            let x = rect.x.saturating_add(col);
            if x < area.width {
                buf[(x, y)].set_symbol(glyph).set_style(style);
            }
        }
    }
}

pub(crate) fn render_notepad_scrollbar(frame: &mut Frame, scrollbar: NotepadScrollbar, focused: bool) {
    let thumb_style = Style::default()
        .fg(if focused {
            NOTEPAD_SCROLL_THUMB_ACTIVE_FG
        } else {
            NOTEPAD_SCROLL_THUMB_FG
        })
        .bg(NOTEPAD_EDIT_BG);
    render_vertical_bar_overlay(frame, scrollbar.thumb, SCROLLBAR_THUMB_GLYPH, thumb_style);
}

pub fn render_full_width_row_backdrop(frame: &mut Frame, pane_area: Rect, y: u16, bg: Color) {
    frame.render_widget(
        Block::default().style(Style::default().bg(bg)),
        Rect {
            x: pane_area.x,
            y,
            width: pane_area.width,
            height: 1,
        },
    );
}

pub(crate) fn row_label_width(line_width: usize) -> usize {
    row_label_width_after_prefix(line_width, ROW_LABEL_OFFSET)
}

pub(crate) fn row_label_width_after_prefix(line_width: usize, prefix_width: usize) -> usize {
    line_width
        .saturating_sub(prefix_width)
        .saturating_sub(ROW_PRE_TRAILING_GAP)
        .saturating_sub(TRAILING_SLOT_WIDTH)
}

pub(crate) fn chrome_row_prefix() -> String {
    CHROME_ROW_PREFIX.to_string()
}

pub(crate) fn row_prefix(lead: &str, index: Option<&str>) -> String {
    let lead_part = if lead.chars().count() >= 2 {
        lead.chars().take(2).collect::<String>()
    } else {
        format!("{lead} ")
    };
    let index_part = match index {
        Some(text) => format!("{text:>2}  "),
        None => format!("{:>2}  ", ""),
    };
    format!("{lead_part}{index_part}")
}

pub(crate) fn row_with_trailing_slot(
    prefix: String,
    label: &str,
    trailing: String,
    line_width: usize,
    row_style: Style,
    trailing_style: Style,
) -> Line<'static> {
    row_with_trailing_slot_width(
        prefix,
        label,
        trailing,
        line_width,
        TRAILING_SLOT_WIDTH,
        row_style,
        trailing_style,
    )
}

fn row_with_trailing_slot_width(
    prefix: String,
    label: &str,
    trailing: String,
    line_width: usize,
    trailing_width: usize,
    row_style: Style,
    trailing_style: Style,
) -> Line<'static> {
    let prefix_width = prefix.chars().count();
    let label_width = line_width
        .saturating_sub(prefix_width)
        .saturating_sub(ROW_PRE_TRAILING_GAP)
        .saturating_sub(trailing_width);
    let title = truncate(label, label_width);
    full_width_spans(
        vec![
            Span::styled(prefix, row_style),
            Span::styled(format!("{:<width$}", title, width = label_width), row_style),
            Span::styled("  ", row_style),
            Span::styled(trailing, trailing_style),
        ],
        line_width,
        row_style,
    )
}

pub(crate) fn empty_trailing_slot(row_style: Style) -> (String, Style) {
    (format_trailing_slot(""), trailing_badge_style(row_style))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotepadScrollbar {
    pub track: Rect,
    pub thumb: Rect,
}

pub fn notepad_scrollbar_thumb_hit(scrollbar: &NotepadScrollbar, y: u16) -> bool {
    y >= scrollbar.thumb.y && y < scrollbar.thumb.y.saturating_add(scrollbar.thumb.height)
}

pub fn notepad_scroll_from_track_click(
    click_y: u16,
    scrollbar: &NotepadScrollbar,
    max_scroll: usize,
) -> usize {
    if max_scroll == 0 {
        return 0;
    }
    let track = scrollbar.track;
    if notepad_scrollbar_thumb_hit(scrollbar, click_y) {
        return usize::MAX;
    }
    let rel = click_y.saturating_sub(track.y) as usize;
    let travel = track.height.saturating_sub(scrollbar.thumb.height) as usize;
    if travel == 0 {
        return 0;
    }
    let centered = rel.saturating_sub(scrollbar.thumb.height as usize / 2);
    ((centered * max_scroll + travel / 2) / travel).min(max_scroll)
}

pub fn notepad_scroll_from_thumb_drag(
    drag_y: u16,
    scrollbar: &NotepadScrollbar,
    max_scroll: usize,
    thumb_grab_offset: u16,
) -> usize {
    if max_scroll == 0 {
        return 0;
    }
    let track = scrollbar.track;
    let travel = track.height.saturating_sub(scrollbar.thumb.height) as usize;
    if travel == 0 {
        return 0;
    }
    let thumb_top = drag_y
        .saturating_sub(thumb_grab_offset)
        .saturating_sub(track.y) as usize;
    let clamped = thumb_top.min(travel);
    ((clamped * max_scroll + travel / 2) / travel).min(max_scroll)
}

pub(crate) fn agent_state_trailing_badge(
    state: AgentState,
    row_style: Style,
    anim_frame: usize,
) -> (String, Style) {
    if state == AgentState::Done {
        return (
            format_completion_square_slot(),
            completion_badge_style(row_style),
        );
    }
    if matches!(state, AgentState::Working | AgentState::Approval) {
        return (
            format_spinner_slot(run_spinner_glyph(anim_frame)),
            spinner_badge_style(row_style),
        );
    }
    empty_trailing_slot(row_style)
}

pub(crate) fn rename_terminal_cursor_position(
    terminal_area: Rect,
    scroll: usize,
    body_height: usize,
    rename: &RenameState,
) -> Option<Position> {
    let row_idx = rename.row_idx;
    if row_idx < scroll || row_idx >= scroll.saturating_add(body_height) {
        return None;
    }
    let line_width = terminal_area.width as usize;
    let (title_offset, label_width) = match &rename.target {
        RenameTarget::Note { .. } => (
            NOTEPAD_NOTE_TITLE_OFFSET,
            row_label_width_after_prefix(line_width, NOTEPAD_NOTE_TITLE_OFFSET),
        ),
        RenameTarget::Session { .. } => (ROW_LABEL_OFFSET, row_label_width(line_width)),
    };
    let title_len = truncate(&rename.buffer, label_width).chars().count();
    let visible_idx = row_idx.saturating_sub(scroll);
    Some(Position::new(
        terminal_area
            .x
            .saturating_add(title_offset as u16 + title_len as u16),
        terminal_area.y.saturating_add(visible_idx as u16),
    ))
}

pub fn rename_targets_session(rename: &RenameState, session_id: &str) -> bool {
    matches!(
        &rename.target,
        RenameTarget::Session {
            session_id: id
        } if id == session_id
    )
}

pub(crate) fn rename_targets_note(rename: &RenameState, note_id: &str) -> bool {
    matches!(
        &rename.target,
        RenameTarget::Note { note_id: id } if id == note_id
    )
}

pub(crate) fn full_width_line(text: String, width: usize, style: Style) -> Line<'static> {
    full_width_spans(vec![Span::styled(text, style)], width, style)
}

pub(crate) fn full_width_spans(
    mut spans: Vec<Span<'static>>,
    width: usize,
    pad_style: Style,
) -> Line<'static> {
    let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    let pad = width.saturating_sub(used);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), pad_style));
    }
    Line::from(spans)
}

pub(crate) fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 1 {
        return text.chars().take(width).collect();
    }
    let mut s: String = text.chars().take(width - 1).collect();
    s.push('…');
    s
}

pub fn run_spinner_glyph(frame: usize) -> &'static str {
    let len = RUN_SPINNER_FRAMES.len();
    RUN_SPINNER_FRAMES[len - 1 - (frame % len)]
}

pub(crate) fn format_trailing_slot(text: &str) -> String {
    format!("{text:>width$}", width = TRAILING_SLOT_WIDTH)
}

pub(crate) fn format_spinner_slot(glyph: &str) -> String {
    format!("{glyph:^width$}", width = TRAILING_SLOT_WIDTH)
}

pub(crate) fn format_completion_square_slot() -> String {
    format!("{:^width$}", "■", width = TRAILING_SLOT_WIDTH)
}

pub(crate) fn group_add_badge_style(row_style: Style) -> Style {
    Style::default()
        .fg(row_style.fg.unwrap_or(TEXT_SECONDARY))
        .bg(row_style.bg.unwrap_or(BG_BASE))
}

pub(crate) fn spinner_badge_style(row_style: Style) -> Style {
    Style::default()
        .fg(row_style.fg.unwrap_or(TEXT_SELECTED))
        .bg(row_style.bg.unwrap_or(BG_BASE))
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn completion_badge_style(row_style: Style) -> Style {
    Style::default()
        .fg(DONE_FG)
        .bg(row_style.bg.unwrap_or(BG_BASE))
        .add_modifier(Modifier::BOLD)
}

pub fn context_menu_items(target: &ContextMenuTarget) -> &'static [ContextMenuAction] {
    match target {
        ContextMenuTarget::Session { .. } => {
            &[ContextMenuAction::Rename, ContextMenuAction::Delete]
        }
        ContextMenuTarget::Group { .. } => &[ContextMenuAction::Delete],
        ContextMenuTarget::Note { .. } => {
            &[ContextMenuAction::Rename, ContextMenuAction::Delete]
        }
        ContextMenuTarget::Notepad { .. } => &[
            ContextMenuAction::Cut,
            ContextMenuAction::Copy,
            ContextMenuAction::Paste,
            ContextMenuAction::SelectAll,
        ],
    }
}

pub fn context_menu_item_enabled(target: &ContextMenuTarget, action: ContextMenuAction) -> bool {
    match (target, action) {
        (ContextMenuTarget::Notepad { has_selection }, ContextMenuAction::Cut)
        | (ContextMenuTarget::Notepad { has_selection }, ContextMenuAction::Copy) => *has_selection,
        (ContextMenuTarget::Notepad { .. }, ContextMenuAction::Paste)
        | (ContextMenuTarget::Notepad { .. }, ContextMenuAction::SelectAll) => true,
        _ => true,
    }
}

pub fn context_menu_height(target: &ContextMenuTarget) -> u16 {
    let items = context_menu_items(target).len() as u16;
    match target {
        ContextMenuTarget::Notepad { .. } => items + 2,
        _ => items,
    }
}

pub fn context_menu_label(target: &ContextMenuTarget, action: ContextMenuAction) -> &'static str {
    match (target, action) {
        (ContextMenuTarget::Session { .. }, ContextMenuAction::Rename) => " Rename ",
        (ContextMenuTarget::Session { .. }, ContextMenuAction::Delete) => " End session ",
        (ContextMenuTarget::Group { .. }, ContextMenuAction::Delete) => " End all sessions ",
        (ContextMenuTarget::Group { .. }, ContextMenuAction::Rename) => " Rename ",
        (ContextMenuTarget::Note { .. }, ContextMenuAction::Rename) => " Rename ",
        (ContextMenuTarget::Note { .. }, ContextMenuAction::Delete) => " Delete note ",
        (ContextMenuTarget::Notepad { .. }, ContextMenuAction::Cut) => " Cut ",
        (ContextMenuTarget::Notepad { .. }, ContextMenuAction::Copy) => " Copy ",
        (ContextMenuTarget::Notepad { .. }, ContextMenuAction::Paste) => " Paste ",
        (ContextMenuTarget::Notepad { .. }, ContextMenuAction::SelectAll) => " Select All ",
        _ => " ",
    }
}

pub fn context_menu_width(target: &ContextMenuTarget) -> u16 {
    match target {
        ContextMenuTarget::Notepad { .. } => NOTEPAD_MENU_WIDTH,
        _ => context_menu_items(target)
            .iter()
            .map(|action| context_menu_label(target, *action).chars().count() as u16)
            .max()
            .unwrap_or(1),
    }
}

fn context_menu_notepad_line(action: ContextMenuAction, enabled: bool) -> Line<'static> {
    let (label, shortcut) = match action {
        ContextMenuAction::Cut => ("Cut", "⌘X"),
        ContextMenuAction::Copy => ("Copy", "⌘C"),
        ContextMenuAction::Paste => ("Paste", "⌘V"),
        ContextMenuAction::SelectAll => ("Select All", "⌘A"),
        _ => ("", ""),
    };
    let label_part = format!("  {label}");
    let label_len = label_part.chars().count();
    let shortcut_len = shortcut.chars().count();
    let pad = NOTEPAD_MENU_INNER_WIDTH.saturating_sub(label_len + shortcut_len);
    let label_style = Style::default()
        .fg(if enabled { TEXT_SELECTED } else { PATH_FG })
        .bg(BG_PANEL);
    let shortcut_style = Style::default()
        .fg(if enabled { TEXT_SECONDARY } else { PATH_FG })
        .bg(BG_PANEL);
    Line::from(vec![
        Span::styled(label_part, label_style),
        Span::styled(" ".repeat(pad), label_style),
        Span::styled(shortcut.to_string(), shortcut_style),
    ])
}

pub(crate) fn render_notepad_context_menu(frame: &mut Frame, menu: &ContextMenu, area: Rect) {
    let rect = context_menu_rect(menu, area);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(NOTEPAD_MENU_BORDER_FG))
        .style(Style::default().bg(BG_PANEL));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    for (idx, action) in context_menu_items(&menu.target).iter().enumerate() {
        let enabled = context_menu_item_enabled(&menu.target, *action);
        let item_rect = Rect {
            x: inner.x,
            y: inner.y.saturating_add(idx as u16),
            width: inner.width,
            height: CONTEXT_MENU_ITEM_HEIGHT,
        };
        frame.render_widget(
            Paragraph::new(context_menu_notepad_line(*action, enabled))
                .style(Style::default().bg(BG_PANEL)),
            item_rect,
        );
    }
}

pub fn context_menu_rect(menu: &ContextMenu, area: Rect) -> Rect {
    let height = context_menu_height(&menu.target);
    let width = context_menu_width(&menu.target);
    let x = menu.x.min(area.width.saturating_sub(width));
    let y = menu.y.min(area.height.saturating_sub(height));
    Rect {
        x: area.x + x,
        y: area.y + y,
        width,
        height,
    }
}

pub fn delete_note_confirm_rect(area: Rect, title: &str) -> Rect {
    let prompt = format!("Delete \"{title}\"?");
    let width = prompt
        .chars()
        .count()
        .max("type yes · enter confirm · esc cancel".len())
        .saturating_add(6)
        .clamp(32, area.width as usize) as u16;
    // prompt + hint + input, plus top/bottom border rows
    let height = 5u16;
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area.y.saturating_add(area.height / 2).saturating_sub(1),
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

pub(crate) fn render_delete_note_confirm(
    frame: &mut Frame,
    confirm: &DeleteNoteConfirmState,
    area: Rect,
) {
    let rect = delete_note_confirm_rect(area, &confirm.title);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(NOTEPAD_MENU_BORDER_FG))
        .style(Style::default().bg(BG_PANEL));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let prompt = format!("Delete \"{}\"?", truncate(&confirm.title, inner.width as usize));
    let prompt_style = Style::default().fg(CLOSE_HOVER_FG).bg(BG_PANEL);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(prompt, prompt_style))),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );
    let hint_style = Style::default().fg(TEXT_SECONDARY).bg(BG_PANEL);
    if inner.height >= 2 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "type yes · enter confirm · esc cancel",
                hint_style,
            ))),
            Rect {
                x: inner.x,
                y: inner.y.saturating_add(1),
                width: inner.width,
                height: 1,
            },
        );
    }
    if inner.height >= 3 {
        let ready = delete_note_confirm_ready(confirm);
        let input_style = Style::default()
            .fg(if ready { RENAME_SAVED_FG } else { TEXT_SELECTED })
            .bg(BG_PANEL);
        let input = format!("> {}", confirm.buffer);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(input, input_style))),
            Rect {
                x: inner.x,
                y: inner.y.saturating_add(2),
                width: inner.width,
                height: 1,
            },
        );
    }
}

pub fn context_menu_action_at(
    menu: &ContextMenu,
    column: u16,
    row: u16,
    area: Rect,
) -> Option<ContextMenuAction> {
    let rect = context_menu_rect(menu, area);
    if column < rect.x
        || column >= rect.x + rect.width
        || row < rect.y
        || row >= rect.y + rect.height
    {
        return None;
    }
    let item_idx = (row - rect.y) as usize;
    let (action_idx, action) = match &menu.target {
        ContextMenuTarget::Notepad { .. } => {
            if item_idx == 0 || item_idx > context_menu_items(&menu.target).len() {
                return None;
            }
            let action = *context_menu_items(&menu.target).get(item_idx - 1)?;
            (item_idx - 1, action)
        }
        _ => {
            let action = *context_menu_items(&menu.target).get(item_idx)?;
            (item_idx, action)
        }
    };
    if context_menu_item_enabled(&menu.target, action) {
        Some(action)
    } else {
        let _ = action_idx;
        None
    }
}
pub fn trailing_badge_style(row_style: Style) -> Style {
    Style::default()
        .fg(PATH_FG)
        .bg(row_style.bg.unwrap_or(BG_BASE))
}

