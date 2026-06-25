//! Settings overlay rendering.

use super::state::*;
use crate::bar::art_canvas;
use crate::bar::ui::{BG_BASE, BG_HIGHLIGHT, BG_HOVER_SELECTED, BG_PANEL, BG_SELECTED, DONE_GREEN, PATH_FG, TEXT_PRIMARY, TEXT_SELECTED};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::block::Padding;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

fn render_overlay(frame: &mut ratatui::Frame, pane: Rect, overlay: &SettingsOverlay) {
    let rect = overlay_rect(pane, overlay);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    frame.render_widget(Clear, rect);
    let border_style = Style::default().fg(OVERLAY_BORDER_FG);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            format!(" {} ", overlay.title),
            Style::default()
                .fg(TEXT_SELECTED)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(BG_PANEL));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let body_rows = overlay.visible_body_rows(inner.height);
    let max_scroll = overlay.max_scroll(inner.height);
    let scroll = overlay.scroll.min(max_scroll);
    let width = inner.width as usize;

    for row in 0..body_rows {
        let y = inner.y.saturating_add(row as u16);
        let line = overlay
            .lines
            .get(scroll + row)
            .map(String::as_str)
            .unwrap_or("");
        let display = truncate_overlay_line(line, width);
        let pad = width.saturating_sub(display.chars().count());
        let style = if overlay.running && scroll + row + 1 == overlay.lines.len() {
            Style::default().fg(DONE_GREEN).bg(BG_PANEL)
        } else if overlay.finished && overlay.success == Some(false) {
            Style::default().fg(Color::Rgb(220, 120, 100)).bg(BG_PANEL)
        } else {
            Style::default().fg(TEXT_PRIMARY).bg(BG_PANEL)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(display, style),
                Span::styled(" ".repeat(pad), Style::default().bg(BG_PANEL)),
            ])),
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
        );
    }

    let hint_y = inner.y.saturating_add(inner.height.saturating_sub(OVERLAY_HINT_ROWS));
    frame.render_widget(
        Paragraph::new(Span::styled(
            overlay.hint(),
            Style::default().fg(PATH_FG).bg(BG_PANEL),
        )),
        Rect {
            x: inner.x,
            y: hint_y,
            width: inner.width,
            height: OVERLAY_HINT_ROWS.min(inner.height),
        },
    );
}
pub fn draw_screen(
    frame: &mut ratatui::Frame,
    rows: &[SettingsRow],
    selected: usize,
    panel_hover: &PanelHover,
    overlay: Option<&SettingsOverlay>,
) -> PanelTargets {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG_BASE)), area);

    let column = art_canvas::panel_column_rect(area);
    let section = Block::default()
        .borders(Borders::NONE)
        .padding(Padding::new(1, 1, 0, 1))
        .style(Style::default().bg(BG_BASE));
    let inner = section.inner(column);
    frame.render_widget(section, column);

    let layout = settings_layout(inner, rows);

    let close_width = CLOSE_BUTTON_COLS.min(layout.header.width);
    let close_target = Rect {
        x: layout
            .header
            .x
            .saturating_add(layout.header.width.saturating_sub(close_width)),
        y: layout.header.y,
        width: close_width,
        height: layout.header.height,
    };
    let title_area = Rect {
        x: layout.header.x,
        y: layout.header.y,
        width: close_target.x.saturating_sub(layout.header.x),
        height: layout.header.height,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            "Settings",
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        )),
        title_area,
    );
    render_close_button(frame, layout.header, panel_hover.close);

    if layout.header_list_gap.height > 0 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                " ".repeat(layout.header_list_gap.width as usize),
                Style::default().bg(BG_BASE),
            )),
            layout.header_list_gap,
        );
    }

    if layout.cta_top_gap.height > 0 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                " ".repeat(layout.cta_top_gap.width as usize),
                Style::default().bg(BG_BASE),
            )),
            layout.cta_top_gap,
        );
    }

    let list_area = layout.list;
    let list_width = list_area.width as usize;
    let mut items: Vec<ListItem> = Vec::new();
    for line in build_list_layout(rows) {
        let ListLine::Row(idx) = line else {
            items.push(ListItem::new(gap_line(list_width)));
            continue;
        };
        let row = &rows[idx];
        let is_selected = idx == selected;
        let is_hovered = panel_hover.row == Some(idx);
        let rendered = match row.kind {
            RowKind::Section => section_line(&row.label, list_width),
            RowKind::Config | RowKind::Toggle | RowKind::Action => {
                config_line(&row.label, &row.detail, list_width, is_selected, is_hovered)
            }
            RowKind::Shortcut => {
                shortcut_line(&row.label, &row.detail, list_width, is_selected, is_hovered)
            }
        };
        items.push(ListItem::new(rendered));
    }
    frame.render_widget(
        List::new(items).style(Style::default().bg(BG_BASE)),
        list_area,
    );

    render_cta_button(
        frame,
        layout.cta,
        CTA_BUTTON_LABEL,
        panel_hover.cta,
    );

    if layout.hint_top_gap.height > 0 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                " ".repeat(layout.hint_top_gap.width as usize),
                Style::default().bg(BG_BASE),
            )),
            layout.hint_top_gap,
        );
    }

    frame.render_widget(
        Paragraph::new(Span::styled(
            "j/k move · ↵ detail/action · Esc close",
            Style::default().fg(PATH_FG).bg(BG_BASE),
        )),
        layout.hint,
    );

    if let Some(overlay_state) = overlay {
        render_overlay(frame, area, overlay_state);
    }

    PanelTargets {
        cta: layout.cta,
        close: close_target,
        row_rects: list_row_rects(list_area, rows),
    }
}

fn render_close_button(frame: &mut ratatui::Frame, row: Rect, hovered: bool) {
    if row.width == 0 || row.height == 0 {
        return;
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
        x: row.x.saturating_add(row.width.saturating_sub(1)),
        y: row.y.saturating_add(row.height.saturating_sub(1) / 2),
        width: 1,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(CLOSE_BUTTON_LABEL, style)),
        label_area,
    );
}

pub(crate) fn row_backdrop(selected: bool, hovered: bool) -> ratatui::style::Color {
    if selected && hovered {
        BG_HOVER_SELECTED
    } else if selected {
        BG_SELECTED
    } else if hovered {
        BG_HIGHLIGHT
    } else {
        BG_PANEL
    }
}

fn render_cta_button(frame: &mut ratatui::Frame, area: Rect, label: &str, hover: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let bg = if hover { BG_SELECTED } else { BG_BASE };
    let border_style = if hover {
        Style::default().fg(TEXT_SELECTED)
    } else {
        Style::default().fg(PATH_FG)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let style = if hover {
        Style::default().fg(TEXT_SELECTED).bg(bg)
    } else {
        Style::default().fg(PATH_FG).bg(bg)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {label} "), style)),
        inner,
    );
}

fn gap_line(width: usize) -> Line<'static> {
    Line::from(Span::styled(
        " ".repeat(width),
        Style::default().bg(BG_BASE),
    ))
}

fn section_line(label: &str, width: usize) -> Line<'static> {
    let text = label.to_string();
    let pad = width.saturating_sub(text.chars().count());
    Line::from(vec![
        Span::styled(
            text,
            Style::default()
                .fg(PATH_FG)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(pad), Style::default().bg(BG_BASE)),
    ])
}

fn config_line(
    label: &str,
    detail: &str,
    width: usize,
    selected: bool,
    hovered: bool,
) -> Line<'static> {
    let lead = if selected { "▎ " } else { "  " };
    let row_bg = row_backdrop(selected, hovered);
    let label_style = if selected || hovered {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(row_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(row_bg)
    };
    let detail_style = if selected || hovered {
        Style::default()
            .fg(if hovered && !selected {
                PATH_FG
            } else {
                TEXT_SELECTED
            })
            .bg(row_bg)
    } else {
        Style::default().fg(PATH_FG).bg(row_bg)
    };
    let prefix = format!("{lead}{label}");
    let prefix_len = prefix.chars().count();
    let detail_len = detail.chars().count();
    let gap = width.saturating_sub(prefix_len + detail_len);
    Line::from(vec![
        Span::styled(prefix, label_style),
        Span::styled(" ".repeat(gap), Style::default().bg(row_bg)),
        Span::styled(detail.to_string(), detail_style),
    ])
}

fn shortcut_line(
    key: &str,
    desc: &str,
    width: usize,
    selected: bool,
    hovered: bool,
) -> Line<'static> {
    let lead = if selected { "▎ " } else { "  " };
    let row_bg = row_backdrop(selected, hovered);
    let key_style = if selected || hovered {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(row_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(row_bg)
    };
    let desc_style = if selected || hovered {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(row_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(row_bg)
    };
    let prefix = lead.to_string();
    let mid = key.to_string();
    let suffix = format!("  {desc}");
    let used = prefix.chars().count() + mid.chars().count() + suffix.chars().count();
    let gap = width.saturating_sub(used);
    Line::from(vec![
        Span::styled(prefix, Style::default().bg(row_bg)),
        Span::styled(mid, key_style),
        Span::styled(" ".repeat(gap), Style::default().bg(row_bg)),
        Span::styled(suffix, desc_style),
    ])
}
