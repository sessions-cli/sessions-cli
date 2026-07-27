//! Automations panel rendering — New Session–level polish.

use super::state::{
    human_run_status, human_status, schedule_summary, AutomationsState, EditorFocus, ListFilter,
    Mode, PanelHover, CLOSE_BUTTON_COLS, CLOSE_BUTTON_LABEL, FIELD_INNER_HEIGHT,
    MAX_DROPDOWN_VISIBLE, PROMPT_INNER_HEIGHT, SECTION_GAP, TITLE_ROWS,
};
use crate::automation::AutomationStatus;
use crate::bar::overlay::panel_content_rect;
use crate::bar::path_picker::{
    PathGhostHint, PathPickerState, PathPopupEntry, PathPopupKind, HEADER_ROWS,
};
use crate::bar::settings::point_in_rect;
use crate::bar::ui::{
    BG_BASE, BG_HIGHLIGHT, BG_SELECTED, CLOSE_HOVER_FG, DONE_GREEN, PATH_FG, TEXT_PRIMARY,
    TEXT_SELECTED, WARM_ACCENT,
};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

const BG_FIELD: ratatui::style::Color = BG_BASE;

#[derive(Debug, Clone, Default)]
pub struct ClickTargets {
    pub form: Rect,
    pub close: Rect,
    pub filters: Vec<(ListFilter, Rect)>,
    pub rows: Vec<Rect>,
    pub new_btn: Rect,
    pub run_btn: Rect,
    pub pause_btn: Rect,
    pub edit_btn: Rect,
    pub name_field: Rect,
    pub cwd_field: Rect,
    pub cwd_popup: Rect,
    pub agent_field: Rect,
    pub agent_popup: Rect,
    pub model_field: Rect,
    pub model_popup: Rect,
    pub schedule_field: Rect,
    pub schedule_popup: Rect,
    pub prompt_field: Rect,
    pub save_btn: Rect,
    pub save_run_btn: Rect,
    pub cancel_btn: Rect,
}

fn field_block_height(inner: u16) -> u16 {
    inner + 2
}

fn dropdown_field_height() -> u16 {
    field_block_height(FIELD_INNER_HEIGHT)
}

fn fill_rect(frame: &mut Frame<'_>, area: Rect, bg: ratatui::style::Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
}

fn paint_opaque(frame: &mut Frame<'_>, area: Rect, bg: ratatui::style::Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Clear, area);
    fill_rect(frame, area, bg);
}

/// Content column for Automations — same Grok outer margins as MCP / Skills.
///
/// Unlike New Session (centered 4/6 form), this uses the full content width so
/// the three management panels share identical left/right breathing room.
fn form_rect(pane: Rect, content_h: u16) -> Rect {
    let content = panel_content_rect(pane);
    let form_height = content_h.min(content.height.max(1));
    // Vertically center only when the form is shorter than the content area.
    let top_extra = if content.height > form_height + 2 {
        (content.height.saturating_sub(form_height)) / 2
    } else {
        0
    };
    Rect {
        x: content.x,
        y: content.y.saturating_add(top_extra),
        width: content.width,
        height: form_height,
    }
}

fn render_close_button(frame: &mut Frame<'_>, row: Rect, hovered: bool) -> Rect {
    if row.width == 0 || row.height == 0 {
        return Rect::default();
    }
    let style = if hovered {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(BG_BASE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(PATH_FG)
            .bg(BG_BASE)
            .add_modifier(Modifier::BOLD)
    };
    let label_area = Rect {
        x: row
            .x
            .saturating_add(row.width.saturating_sub(CLOSE_BUTTON_COLS)),
        y: row.y.saturating_add(row.height.saturating_sub(1) / 2),
        width: CLOSE_BUTTON_COLS,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(CLOSE_BUTTON_LABEL, style)),
        label_area,
    );
    label_area
}

fn render_field_label(frame: &mut Frame<'_>, area: Rect, label: &str, focused: bool) {
    let style = if focused {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(BG_BASE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(BG_BASE)
    };
    frame.render_widget(Paragraph::new(Span::styled(label, style)), area);
}

fn field_block(focused: bool) -> Block<'static> {
    let border = if focused {
        Style::default().fg(TEXT_SELECTED)
    } else {
        Style::default().fg(PATH_FG)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(Style::default().bg(BG_FIELD))
}

/// List-row card: selection/hover raise the border (same language as form fields), never fill.
fn list_item_block(selected: bool, hovered: bool) -> Block<'static> {
    let border = if selected || hovered {
        Style::default().fg(TEXT_SELECTED)
    } else {
        Style::default().fg(PATH_FG)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(Style::default().bg(BG_BASE))
}

/// Title + meta inside borders (top/bottom take 2 rows).
const LIST_ROW_CONTENT_H: u16 = 2;
const LIST_ROW_H: u16 = LIST_ROW_CONTENT_H + 2;
/// Vertical gap between bordered cards so adjacent borders do not double up.
const LIST_ROW_GAP: u16 = 1;
const LIST_ROW_STRIDE: u16 = LIST_ROW_H + LIST_ROW_GAP;

fn render_text_field(frame: &mut Frame<'_>, area: Rect, value: &str, focused: bool) {
    let block = field_block(focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let style = if focused {
        Style::default().fg(TEXT_SELECTED).bg(BG_FIELD)
    } else {
        Style::default().fg(PATH_FG).bg(BG_FIELD)
    };
    let display = if value.is_empty() && focused {
        " "
    } else if value.is_empty() {
        ""
    } else {
        value
    };
    frame.render_widget(Paragraph::new(Span::styled(display, style)), inner);
}

fn render_dropdown_collapsed(frame: &mut Frame<'_>, area: Rect, value: &str, focused: bool) {
    let block = field_block(focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let style = if focused {
        Style::default().fg(TEXT_SELECTED).bg(BG_FIELD)
    } else {
        Style::default().fg(PATH_FG).bg(BG_FIELD)
    };
    let chevron = if focused { " ▾" } else { "  " };
    let mut text = value.to_string();
    text.push_str(chevron);
    frame.render_widget(Paragraph::new(Span::styled(text, style)), inner);
}

fn dropdown_window(count: usize, selected: usize) -> (usize, usize) {
    if count == 0 {
        return (0, 0);
    }
    if count <= MAX_DROPDOWN_VISIBLE {
        return (0, count);
    }
    let start = selected
        .saturating_sub(MAX_DROPDOWN_VISIBLE / 2)
        .min(count.saturating_sub(MAX_DROPDOWN_VISIBLE));
    (start, MAX_DROPDOWN_VISIBLE)
}

fn truncate_to_width(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    if max == 1 {
        return "…".into();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

fn render_path_field_collapsed(
    frame: &mut Frame<'_>,
    area: Rect,
    value: &str,
    focused: bool,
    confirmed: bool,
    error: bool,
) {
    let border = if error {
        Style::default().fg(CLOSE_HOVER_FG)
    } else if confirmed && !focused {
        Style::default().fg(PATH_FG)
    } else if focused {
        Style::default().fg(TEXT_SELECTED)
    } else {
        Style::default().fg(PATH_FG)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(Style::default().bg(BG_FIELD));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let style = if error {
        Style::default().fg(CLOSE_HOVER_FG).bg(BG_FIELD)
    } else if focused {
        Style::default().fg(TEXT_SELECTED).bg(BG_FIELD)
    } else if confirmed {
        Style::default().fg(PATH_FG).bg(BG_FIELD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(BG_FIELD)
    };
    let width = inner.width.saturating_sub(1) as usize;
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {}", truncate_to_width(value, width.saturating_sub(1))),
            style,
        )),
        inner,
    );
}

fn path_list_window(count: usize, selected: usize, max_visible: usize) -> (usize, usize) {
    if count == 0 || max_visible == 0 {
        return (0, 0);
    }
    let visible = count.min(max_visible);
    if count <= visible {
        return (0, count);
    }
    let start = selected
        .saturating_sub(visible / 2)
        .min(count.saturating_sub(visible));
    (start, visible)
}

fn render_path_menu(
    frame: &mut Frame<'_>,
    anchor: Rect,
    picker: &PathPickerState,
    entries: &[PathPopupEntry],
    hover_idx: Option<usize>,
    header_error: bool,
    ghost: Option<&PathGhostHint>,
    frame_area: Rect,
) -> Rect {
    let frame_bottom = frame_area.y.saturating_add(frame_area.height);
    let max_list_rows = frame_bottom.saturating_sub(anchor.y + 2 + HEADER_ROWS) as usize;
    let (start, visible) = path_list_window(entries.len(), picker.highlight, max_list_rows);
    let menu = Rect {
        x: anchor.x,
        y: anchor.y,
        width: anchor.width,
        height: HEADER_ROWS
            .saturating_add(visible as u16)
            .saturating_add(2)
            .max(anchor.height),
    };
    paint_opaque(frame, menu, BG_FIELD);
    let border = if header_error {
        Style::default().fg(CLOSE_HOVER_FG)
    } else {
        Style::default().fg(TEXT_SELECTED)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(Style::default().bg(BG_FIELD));
    let inner = block.inner(menu);
    frame.render_widget(block, menu);

    // Sticky header with typed / highlighted path + ghost
    let header_rect = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: HEADER_ROWS,
    };
    fill_rect(frame, header_rect, BG_FIELD);
    let header_value = if picker.is_typing() {
        picker.input.clone()
    } else {
        picker.header_display()
    };
    let header_width = inner.width.saturating_sub(2) as usize;
    let ghost_style = Style::default().fg(PATH_FG).bg(BG_FIELD);
    let header_line = if picker.is_typing() && header_value.is_empty() {
        Line::from(Span::styled(
            "~/  (or type a path)",
            Style::default().fg(PATH_FG).bg(BG_FIELD),
        ))
    } else if let Some(hint) = ghost.filter(|_| picker.is_typing() && !header_error) {
        let typed_display = truncate_to_width(&header_value, header_width);
        let typed_len = typed_display.chars().count();
        let remaining = header_width.saturating_sub(typed_len);
        let mut spans = vec![Span::styled(
            typed_display,
            if header_error {
                Style::default()
                    .fg(CLOSE_HOVER_FG)
                    .bg(BG_FIELD)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT_SELECTED).bg(BG_FIELD)
            },
        )];
        if remaining > 0 {
            let ghost_text = match hint {
                PathGhostHint::Suffix(s) => s.clone(),
                PathGhostHint::FullPath(p) => format!("  {p}"),
            };
            spans.push(Span::styled(
                truncate_to_width(&ghost_text, remaining),
                ghost_style,
            ));
        }
        Line::from(spans)
    } else {
        let style = if header_error {
            Style::default().fg(CLOSE_HOVER_FG).bg(BG_FIELD)
        } else if picker.is_typing() {
            Style::default().fg(TEXT_SELECTED).bg(BG_FIELD)
        } else {
            Style::default().fg(PATH_FG).bg(BG_FIELD)
        };
        Line::from(Span::styled(
            truncate_to_width(&header_value, header_width),
            style,
        ))
    };
    frame.render_widget(
        Paragraph::new(header_line),
        Rect {
            x: inner.x.saturating_add(1),
            y: inner.y,
            width: inner.width.saturating_sub(1),
            height: 1,
        },
    );

    for i in 0..visible {
        let idx = start + i;
        let Some(entry) = entries.get(idx) else {
            break;
        };
        let y = inner.y.saturating_add(HEADER_ROWS + i as u16);
        let selected = idx == picker.highlight && entry.kind != PathPopupKind::Section;
        let hovered = hover_idx == Some(idx) && !selected && entry.kind != PathPopupKind::Section;
        let is_section = entry.kind == PathPopupKind::Section;
        let bg = if selected {
            BG_SELECTED
        } else if hovered {
            BG_HIGHLIGHT
        } else {
            BG_FIELD
        };
        let style = if is_section {
            Style::default()
                .fg(PATH_FG)
                .bg(BG_FIELD)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else if hovered {
            Style::default().fg(TEXT_SELECTED).bg(bg)
        } else {
            Style::default().fg(TEXT_PRIMARY).bg(bg)
        };
        let label = PathPickerState::row_label(entry);
        let row = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        let width = inner.width.saturating_sub(1) as usize;
        let text = format!(
            " {:<width$}",
            truncate_to_width(&label, width.saturating_sub(1)),
            width = width
        );
        frame.render_widget(Paragraph::new(Span::styled(text, style)), row);
    }
    menu
}

fn place_path_cursor(frame: &mut Frame<'_>, menu: Rect, picker: &PathPickerState, pane: Rect) {
    if menu.width < 3 || menu.height == 0 {
        return;
    }
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(menu);
    if inner.width < 2 || inner.height == 0 {
        return;
    }
    let text_x = inner.x.saturating_add(1);
    let text_y = inner.y;
    let content_w = (inner.width.saturating_sub(2)) as usize;
    let shown = truncate_to_width(&picker.input, content_w);
    let cursor_col = picker
        .cursor
        .min(picker.input.chars().count())
        .min(shown.chars().count()) as u16;
    let mut cx = text_x.saturating_add(cursor_col);
    let max_cx = inner.x.saturating_add(inner.width.saturating_sub(1));
    if cx > max_cx {
        cx = max_cx;
    }
    let pos = Position::new(cx, text_y);
    if pos.x < pane.x.saturating_add(pane.width) && pos.y < pane.y.saturating_add(pane.height) {
        frame.set_cursor_position(pos);
    }
}

fn render_dropdown_menu(
    frame: &mut Frame<'_>,
    anchor: Rect,
    options: &[String],
    selected_idx: usize,
    hover_idx: Option<usize>,
    open: bool,
    frame_area: Rect,
) -> Rect {
    if !open || options.is_empty() {
        let value = options.get(selected_idx).map(String::as_str).unwrap_or("");
        render_dropdown_collapsed(frame, anchor, value, open);
        return anchor;
    }
    let max_rows = frame_area
        .y
        .saturating_add(frame_area.height)
        .saturating_sub(anchor.y + 2) as usize;
    let (start, visible) = {
        let (s, v) = dropdown_window(options.len(), selected_idx);
        (s, v.min(max_rows).min(options.len()))
    };
    if visible == 0 {
        render_dropdown_collapsed(frame, anchor, "", true);
        return anchor;
    }
    let menu = Rect {
        x: anchor.x,
        y: anchor.y,
        width: anchor.width,
        height: (visible as u16).saturating_add(2),
    };
    paint_opaque(frame, menu, BG_FIELD);
    let block = field_block(true);
    let inner = block.inner(menu);
    frame.render_widget(block, menu);
    for i in 0..visible {
        let idx = start + i;
        let y = inner.y.saturating_add(i as u16);
        let selected = idx == selected_idx;
        let hovered = hover_idx == Some(idx) && !selected;
        let bg = if selected {
            BG_SELECTED
        } else if hovered {
            BG_HIGHLIGHT
        } else {
            BG_FIELD
        };
        let style = if selected {
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else if hovered {
            Style::default().fg(TEXT_SELECTED).bg(bg)
        } else {
            Style::default().fg(TEXT_PRIMARY).bg(bg)
        };
        let label = options.get(idx).map(String::as_str).unwrap_or("");
        let row = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        let width = inner.width.saturating_sub(1) as usize;
        let text = format!(" {:<width$}", label, width = width);
        frame.render_widget(Paragraph::new(Span::styled(text, style)), row);
    }
    menu
}

fn render_submit_button(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    focused: bool,
    hovered: bool,
) {
    let block = field_block(focused || hovered);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let style = if focused || hovered {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(BG_FIELD)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(PATH_FG).bg(BG_FIELD)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(label, style)).alignment(ratatui::layout::Alignment::Center),
        inner,
    );
}

fn render_chip(frame: &mut Frame<'_>, area: Rect, label: &str, active: bool, hovered: bool) {
    let style = if active {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(BG_SELECTED)
            .add_modifier(Modifier::BOLD)
    } else if hovered {
        Style::default().fg(TEXT_SELECTED).bg(BG_HIGHLIGHT)
    } else {
        Style::default().fg(PATH_FG).bg(BG_BASE)
    };
    let text = format!(" {label} ");
    frame.render_widget(Paragraph::new(Span::styled(text, style)), area);
}

pub fn draw_screen(
    frame: &mut Frame<'_>,
    state: &mut AutomationsState,
    hover: &PanelHover,
) -> ClickTargets {
    let area = frame.area();
    fill_rect(frame, area, BG_BASE);

    match state.mode {
        Mode::List => draw_list(frame, area, state, hover),
        Mode::Editor => draw_editor(frame, area, state, hover),
    }
}

fn draw_list(
    frame: &mut Frame<'_>,
    pane: Rect,
    state: &mut AutomationsState,
    hover: &PanelHover,
) -> ClickTargets {
    let mut targets = ClickTargets::default();
    // Full content height within Grok margins (no extra pad — matches MCP/Skills).
    let form = form_rect(pane, pane.height);
    targets.form = form;
    let inner = form;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TITLE_ROWS),
            Constraint::Length(1), // filters
            Constraint::Length(SECTION_GAP),
            Constraint::Min(4), // list
            Constraint::Length(SECTION_GAP),
            Constraint::Length(dropdown_field_height()), // actions
            Constraint::Length(1),                       // hint
        ])
        .split(inner);

    // Title + close
    let title = if state.unread > 0 {
        format!("Automations  ·  {} unread", state.unread)
    } else {
        "Automations".into()
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            title,
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        )),
        Rect {
            x: chunks[0].x,
            y: chunks[0].y,
            width: chunks[0].width.saturating_sub(CLOSE_BUTTON_COLS),
            height: 1,
        },
    );
    targets.close = render_close_button(frame, chunks[0], hover.close);

    // Filter chips
    let filters = ListFilter::all();
    let chip_w: u16 = 10;
    let mut x = chunks[1].x;
    for f in filters {
        let rect = Rect {
            x,
            y: chunks[1].y,
            width: chip_w.min(
                chunks[1]
                    .width
                    .saturating_sub(x.saturating_sub(chunks[1].x)),
            ),
            height: 1,
        };
        if rect.width >= 4 {
            render_chip(
                frame,
                rect,
                f.label(),
                state.filter == *f,
                hover.filter == Some(*f),
            );
            targets.filters.push((*f, rect));
        }
        x = x.saturating_add(chip_w);
    }

    // List body — bordered cards; selection/hover raise the border (not fill).
    let list_area = chunks[3];
    let visible_rows = (list_area.height / LIST_ROW_STRIDE).max(1) as usize;
    state.ensure_list_scroll(visible_rows);

    match state.filter {
        ListFilter::Runs => {
            if state.runs.is_empty() {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "No runs yet — create an automation and press Run.",
                        Style::default().fg(PATH_FG).bg(BG_BASE),
                    )),
                    list_area,
                );
            } else {
                let end = (state.list_scroll + visible_rows).min(state.runs.len());
                for (vis_i, idx) in (state.list_scroll..end).enumerate() {
                    let run = &state.runs[idx];
                    let y = list_area.y.saturating_add(vis_i as u16 * LIST_ROW_STRIDE);
                    let avail = list_area
                        .height
                        .saturating_sub(vis_i as u16 * LIST_ROW_STRIDE);
                    let row = Rect {
                        x: list_area.x,
                        y,
                        width: list_area.width,
                        height: LIST_ROW_H.min(avail),
                    };
                    if row.height < 3 {
                        break;
                    }
                    let selected = idx == state.selected;
                    let hovered = hover.row == Some(idx);
                    let block = list_item_block(selected, hovered);
                    let inner = block.inner(row);
                    frame.render_widget(block, row);
                    let title_style = if selected || run.unread {
                        Style::default()
                            .fg(TEXT_SELECTED)
                            .bg(BG_BASE)
                            .add_modifier(Modifier::BOLD)
                    } else if hovered {
                        Style::default().fg(TEXT_SELECTED).bg(BG_BASE)
                    } else {
                        Style::default().fg(TEXT_PRIMARY).bg(BG_BASE)
                    };
                    let meta_style = Style::default().fg(PATH_FG).bg(BG_BASE);
                    let mark = if run.unread { "■ " } else { "  " };
                    frame.render_widget(
                        Paragraph::new(Span::styled(
                            format!("{mark}{}  ·  {}", run.automation_id, human_run_status(run)),
                            title_style,
                        )),
                        Rect {
                            x: inner.x,
                            y: inner.y,
                            width: inner.width,
                            height: 1,
                        },
                    );
                    if inner.height > 1 {
                        frame.render_widget(
                            Paragraph::new(Span::styled(
                                format!(
                                    "  {}  ·  {}  ·  {}",
                                    run.agent,
                                    run.started_at
                                        .with_timezone(&chrono::Local)
                                        .format("%b %d %H:%M"),
                                    run.cwd
                                ),
                                meta_style,
                            )),
                            Rect {
                                x: inner.x,
                                y: inner.y + 1,
                                width: inner.width,
                                height: 1,
                            },
                        );
                    }
                    targets.rows.push(row);
                }
            }
        }
        _ => {
            let items = state.filtered_items();
            if items.is_empty() {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "No automations yet — press n or click New.",
                        Style::default().fg(PATH_FG).bg(BG_BASE),
                    )),
                    list_area,
                );
            } else {
                let end = (state.list_scroll + visible_rows).min(items.len());
                for (vis_i, idx) in (state.list_scroll..end).enumerate() {
                    let a = items[idx];
                    let y = list_area.y.saturating_add(vis_i as u16 * LIST_ROW_STRIDE);
                    let avail = list_area
                        .height
                        .saturating_sub(vis_i as u16 * LIST_ROW_STRIDE);
                    let row = Rect {
                        x: list_area.x,
                        y,
                        width: list_area.width,
                        height: LIST_ROW_H.min(avail),
                    };
                    if row.height < 3 {
                        break;
                    }
                    let selected = idx == state.selected;
                    let hovered = hover.row == Some(idx);
                    let block = list_item_block(selected, hovered);
                    let inner = block.inner(row);
                    frame.render_widget(block, row);
                    let status_mark = match a.status {
                        AutomationStatus::Active => {
                            Span::styled("■ ", Style::default().fg(DONE_GREEN).bg(BG_BASE))
                        }
                        AutomationStatus::Paused => {
                            Span::styled("□ ", Style::default().fg(PATH_FG).bg(BG_BASE))
                        }
                    };
                    let name_style = if selected {
                        Style::default()
                            .fg(TEXT_SELECTED)
                            .bg(BG_BASE)
                            .add_modifier(Modifier::BOLD)
                    } else if hovered {
                        Style::default().fg(TEXT_SELECTED).bg(BG_BASE)
                    } else {
                        Style::default().fg(TEXT_PRIMARY).bg(BG_BASE)
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            status_mark,
                            Span::styled(a.name.clone(), name_style),
                            Span::styled(
                                format!("  ·  {}", a.agent),
                                Style::default().fg(PATH_FG).bg(BG_BASE),
                            ),
                        ])),
                        Rect {
                            x: inner.x,
                            y: inner.y,
                            width: inner.width,
                            height: 1,
                        },
                    );
                    if inner.height > 1 {
                        frame.render_widget(
                            Paragraph::new(Span::styled(
                                format!(
                                    "  {}  ·  next {}  ·  {}",
                                    schedule_summary(a),
                                    state.next_run_label(a),
                                    human_status(a)
                                ),
                                Style::default().fg(PATH_FG).bg(BG_BASE),
                            )),
                            Rect {
                                x: inner.x,
                                y: inner.y + 1,
                                width: inner.width,
                                height: 1,
                            },
                        );
                    }
                    targets.rows.push(row);
                }
            }
        }
    }

    // Action buttons
    let action_row = chunks[5];
    let btn_w: u16 = 12;
    let gap: u16 = 1;
    let labels = ["New  n", "Run  r", "Pause  p", "Edit  e"];
    let total = (btn_w + gap) * labels.len() as u16 - gap;
    let mut bx = action_row
        .x
        .saturating_add(action_row.width.saturating_sub(total) / 2);
    let rects: Vec<Rect> = (0..4)
        .map(|_| {
            let r = Rect {
                x: bx,
                y: action_row.y,
                width: btn_w.min(action_row.width),
                height: action_row.height,
            };
            bx = bx.saturating_add(btn_w + gap);
            r
        })
        .collect();
    let hovers = [
        hover.new_btn,
        hover.run_btn,
        hover.pause_btn,
        hover.edit_btn,
    ];
    for (i, label) in labels.iter().enumerate() {
        if rects[i].width > 0 {
            render_submit_button(frame, rects[i], label, false, hovers[i]);
        }
    }
    targets.new_btn = rects[0];
    targets.run_btn = rects[1];
    targets.pause_btn = rects[2];
    targets.edit_btn = rects[3];

    let hint = if !state.status.is_empty() {
        state.status.clone()
    } else if state.filter == ListFilter::Runs {
        "↵ open run  ·  m mark all read  ·  Tab filters  ·  esc close".into()
    } else {
        "↵ edit  ·  n new  ·  r run  ·  p pause  ·  Tab filters  ·  esc close".into()
    };
    let hint_err = !state.status.is_empty();
    frame.render_widget(
        Paragraph::new(Span::styled(
            hint,
            Style::default()
                .fg(if hint_err { CLOSE_HOVER_FG } else { PATH_FG })
                .bg(BG_BASE),
        )),
        chunks[6],
    );

    targets
}

fn draw_editor(
    frame: &mut Frame<'_>,
    pane: Rect,
    state: &mut AutomationsState,
    hover: &PanelHover,
) -> ClickTargets {
    let mut targets = ClickTargets::default();
    let name_h = dropdown_field_height();
    let cwd_h = dropdown_field_height();
    let agent_h = dropdown_field_height();
    let model_h = dropdown_field_height();
    let schedule_h = dropdown_field_height();
    let prompt_h = field_block_height(PROMPT_INNER_HEIGHT);
    let button_h = dropdown_field_height();

    let content_h = TITLE_ROWS
        + 1
        + name_h
        + SECTION_GAP
        + 1
        + cwd_h
        + SECTION_GAP
        + 1
        + agent_h
        + SECTION_GAP
        + 1
        + model_h
        + SECTION_GAP
        + 1
        + schedule_h
        + SECTION_GAP
        + 1
        + prompt_h
        + SECTION_GAP
        + button_h
        + 2;

    let form = form_rect(pane, content_h.min(pane.height));
    targets.form = form;
    // Outer margins already applied via form_rect (Grok-matching); no extra pad.
    let inner = form;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TITLE_ROWS),
            Constraint::Length(1),
            Constraint::Length(name_h),
            Constraint::Length(SECTION_GAP),
            Constraint::Length(1),
            Constraint::Length(cwd_h),
            Constraint::Length(SECTION_GAP),
            Constraint::Length(1),
            Constraint::Length(agent_h),
            Constraint::Length(SECTION_GAP),
            Constraint::Length(1),
            Constraint::Length(model_h),
            Constraint::Length(SECTION_GAP),
            Constraint::Length(1),
            Constraint::Length(schedule_h),
            Constraint::Length(SECTION_GAP),
            Constraint::Length(1),
            Constraint::Length(prompt_h),
            Constraint::Length(SECTION_GAP),
            Constraint::Length(button_h),
            Constraint::Length(1),
        ])
        .split(inner);

    let title = if state.editing_id.is_some() {
        "Edit automation"
    } else {
        "New automation"
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            title,
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        )),
        Rect {
            x: chunks[0].x,
            y: chunks[0].y,
            width: chunks[0].width.saturating_sub(CLOSE_BUTTON_COLS),
            height: 1,
        },
    );
    targets.close = render_close_button(frame, chunks[0], hover.close);

    // Base layer — always paint so open menus don't leave black voids.
    render_field_label(
        frame,
        chunks[1],
        "Name",
        state.editor_focus == EditorFocus::Name,
    );
    targets.name_field = chunks[2];
    render_text_field(
        frame,
        chunks[2],
        &state.name,
        state.editor_focus == EditorFocus::Name && !state.dropdown_open,
    );

    render_field_label(
        frame,
        chunks[4],
        "Project path",
        state.editor_focus == EditorFocus::Cwd,
    );
    targets.cwd_field = chunks[5];
    let path_open = state.dropdown_open && state.editor_focus == EditorFocus::Cwd;
    let path_error = state.path.path_input_error().is_some();
    if !path_open {
        render_path_field_collapsed(
            frame,
            chunks[5],
            &state.path.display_value(),
            state.editor_focus == EditorFocus::Cwd,
            state.path.confirmed,
            path_error,
        );
    }

    let agent_labels: Vec<String> = state
        .agent_choices()
        .iter()
        .map(|a| a.label.to_string())
        .collect();
    let model_labels: Vec<String> = state
        .selected_agent()
        .models
        .iter()
        .map(|m| m.label.to_string())
        .collect();
    let schedule_labels: Vec<String> = AutomationsState::schedule_presets()
        .iter()
        .map(|p| p.label().to_string())
        .collect();

    render_field_label(
        frame,
        chunks[7],
        "Agent",
        state.editor_focus == EditorFocus::Agent,
    );
    targets.agent_field = chunks[8];
    render_dropdown_collapsed(
        frame,
        chunks[8],
        state.selected_agent().label,
        state.editor_focus == EditorFocus::Agent && !state.dropdown_open,
    );

    render_field_label(
        frame,
        chunks[10],
        "Model",
        state.editor_focus == EditorFocus::Model,
    );
    targets.model_field = chunks[11];
    render_dropdown_collapsed(
        frame,
        chunks[11],
        state.selected_model_label(),
        state.editor_focus == EditorFocus::Model && !state.dropdown_open,
    );

    render_field_label(
        frame,
        chunks[13],
        "Schedule",
        state.editor_focus == EditorFocus::Schedule,
    );
    targets.schedule_field = chunks[14];
    render_dropdown_collapsed(
        frame,
        chunks[14],
        &state.selected_schedule_label(),
        state.editor_focus == EditorFocus::Schedule && !state.dropdown_open,
    );

    render_field_label(
        frame,
        chunks[16],
        "Prompt",
        state.editor_focus == EditorFocus::Prompt,
    );
    targets.prompt_field = chunks[17];
    let block = field_block(state.editor_focus == EditorFocus::Prompt && !state.dropdown_open);
    let prompt_inner = block.inner(chunks[17]);
    frame.render_widget(block, chunks[17]);
    let style = if state.editor_focus == EditorFocus::Prompt && !state.dropdown_open {
        Style::default().fg(TEXT_SELECTED).bg(BG_FIELD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(BG_FIELD)
    };
    frame.render_widget(
        Paragraph::new(state.prompt.as_str())
            .style(style)
            .wrap(Wrap { trim: false })
            .scroll((state.prompt_scroll, 0)),
        prompt_inner,
    );

    let button_row = chunks[19];
    let btn_w: u16 = 16;
    let gap: u16 = 2;
    let total = btn_w * 3 + gap * 2;
    let start_x = button_row
        .x
        .saturating_add(button_row.width.saturating_sub(total) / 2);
    let save = Rect {
        x: start_x,
        y: button_row.y,
        width: btn_w,
        height: button_row.height,
    };
    let save_run = Rect {
        x: start_x + btn_w + gap,
        y: button_row.y,
        width: btn_w,
        height: button_row.height,
    };
    let cancel = Rect {
        x: start_x + (btn_w + gap) * 2,
        y: button_row.y,
        width: btn_w,
        height: button_row.height,
    };
    render_submit_button(
        frame,
        save,
        "Save",
        state.editor_focus == EditorFocus::Save,
        hover.save_btn,
    );
    render_submit_button(
        frame,
        save_run,
        "Save & run",
        state.editor_focus == EditorFocus::SaveRun,
        hover.save_run_btn,
    );
    render_submit_button(
        frame,
        cancel,
        "Cancel",
        state.editor_focus == EditorFocus::Cancel,
        hover.cancel_btn,
    );
    targets.save_btn = save;
    targets.save_run_btn = save_run;
    targets.cancel_btn = cancel;

    // Overlay open menus last.
    let agent_open = state.dropdown_open && state.editor_focus == EditorFocus::Agent;
    let model_open = state.dropdown_open && state.editor_focus == EditorFocus::Model;
    let schedule_open = state.dropdown_open && state.editor_focus == EditorFocus::Schedule;
    let hover_item = hover.dropdown_item;

    if path_open {
        let entries = state.path.build_popup();
        let ghost = state.path.ghost_hint();
        targets.cwd_popup = render_path_menu(
            frame,
            chunks[5],
            &state.path,
            &entries,
            hover_item,
            path_error,
            ghost.as_ref(),
            pane,
        );
        // Cursor in header when typing
        if state.path.is_typing() {
            place_path_cursor(frame, targets.cwd_popup, &state.path, pane);
        }
    } else {
        targets.cwd_popup = chunks[5];
    }
    if agent_open {
        targets.agent_popup = render_dropdown_menu(
            frame,
            chunks[8],
            &agent_labels,
            state.agent_idx,
            hover_item,
            true,
            pane,
        );
    } else {
        targets.agent_popup = chunks[8];
    }
    if model_open {
        targets.model_popup = render_dropdown_menu(
            frame,
            chunks[11],
            &model_labels,
            state.model_idx,
            hover_item,
            true,
            pane,
        );
    } else {
        targets.model_popup = chunks[11];
    }
    if schedule_open {
        targets.schedule_popup = render_dropdown_menu(
            frame,
            chunks[14],
            &schedule_labels,
            state.schedule_idx,
            hover_item,
            true,
            pane,
        );
    } else {
        targets.schedule_popup = chunks[14];
    }

    let hint = if !state.status.is_empty() {
        state.status.clone()
    } else if path_open {
        "↑↓ pick · ←→ edit · Tab complete · ↵ confirm · type ~/path".into()
    } else if state.dropdown_open {
        "↑↓ select  ·  ↵ confirm  ·  esc close menu".into()
    } else {
        match state.editor_focus {
            EditorFocus::Cwd => "type to search  ·  ↵ open path menu  ·  Tab complete".into(),
            EditorFocus::Agent | EditorFocus::Model | EditorFocus::Schedule => {
                "↵ open menu  ·  Tab next  ·  esc back to list".into()
            }
            EditorFocus::Prompt => "↵ newline  ·  Tab buttons  ·  ⌘S save".into(),
            EditorFocus::Save | EditorFocus::SaveRun | EditorFocus::Cancel => {
                "↵ activate  ·  Tab cycle".into()
            }
            _ => "Tab next  ·  ↵ menus  ·  esc back".into(),
        }
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            hint,
            Style::default()
                .fg(if !state.status.is_empty() {
                    CLOSE_HOVER_FG
                } else if state.editor_focus == EditorFocus::SaveRun {
                    WARM_ACCENT
                } else {
                    PATH_FG
                })
                .bg(BG_BASE),
        )),
        chunks[20],
    );

    targets
}

pub fn dropdown_click_index(
    popup: Rect,
    col: u16,
    row: u16,
    count: usize,
    selected: usize,
) -> Option<usize> {
    if count == 0 || popup.height < 3 {
        return None;
    }
    if !point_in_rect(col, row, popup) {
        return None;
    }
    let inner_y = popup.y.saturating_add(1);
    let row_idx = row.saturating_sub(inner_y) as usize;
    let visible = popup.height.saturating_sub(2) as usize;
    let (start, _) = dropdown_window(count, selected);
    if row_idx >= visible {
        return None;
    }
    let idx = start + row_idx;
    if idx < count {
        Some(idx)
    } else {
        None
    }
}
