//! Shared centered setup dialog for optional MCP / Skills managers.

use crate::bar::ui::{
    BG_BASE, BG_PANEL, BG_SELECTED, DONE_FG, PATH_FG, TEXT_PRIMARY, TEXT_SELECTED, WARM_ACCENT,
};
use crate::companions::{SetupDialog, SetupPhase};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

const MIN_W: u16 = 48;
const MIN_H: u16 = 12;

pub fn draw(frame: &mut Frame<'_>, dialog: &SetupDialog) {
    let pane = frame.area();
    if pane.width < 20 || pane.height < 8 {
        return;
    }
    let width = (pane.width.saturating_mul(3) / 4).clamp(MIN_W, pane.width.saturating_sub(2));
    let height = (pane.height.saturating_mul(2) / 3).clamp(MIN_H, pane.height.saturating_sub(2));
    let x = pane.x + pane.width.saturating_sub(width) / 2;
    let y = pane.y + pane.height.saturating_sub(height) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    // Dim scrim behind dialog.
    frame.render_widget(Clear, pane);
    frame.render_widget(
        Block::default().style(Style::default().bg(BG_BASE).fg(PATH_FG)),
        pane,
    );

    let border = match dialog.phase {
        SetupPhase::DoneOk => Style::default().fg(DONE_FG),
        SetupPhase::DoneFail => Style::default().fg(WARM_ACCENT),
        SetupPhase::Running => Style::default().fg(WARM_ACCENT),
        SetupPhase::Prompt => Style::default().fg(TEXT_SELECTED),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::styled(
            format!(" {} ", dialog.kind.title()),
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(BG_PANEL)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(BG_PANEL));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let body_h = inner.height.saturating_sub(2).max(1);
    let visible = body_h as usize;
    let max_scroll = dialog.lines.len().saturating_sub(visible);
    let scroll = dialog.scroll.min(max_scroll);
    let mut body: Vec<Line> = dialog
        .lines
        .iter()
        .skip(scroll)
        .take(visible)
        .map(|line| {
            let style = if line.starts_with('✓') {
                Style::default().fg(DONE_FG).bg(BG_PANEL)
            } else if line.starts_with('✗') || line.starts_with("FAIL") {
                Style::default().fg(WARM_ACCENT).bg(BG_PANEL)
            } else {
                Style::default().fg(TEXT_PRIMARY).bg(BG_PANEL)
            };
            Line::from(Span::styled(line.clone(), style))
        })
        .collect();
    if body.is_empty() {
        body.push(Line::from(Span::styled(
            "…",
            Style::default().fg(PATH_FG).bg(BG_PANEL),
        )));
    }
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(BG_PANEL)),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: body_h,
        },
    );

    let hint_y = inner.y.saturating_add(inner.height.saturating_sub(1));
    let cta = match dialog.phase {
        SetupPhase::Prompt => " [ Enter ] Set up automatically ",
        SetupPhase::Running => " Setting up… ",
        SetupPhase::DoneOk => " [ Enter ] Continue ",
        SetupPhase::DoneFail => " [ Enter ] Retry ",
    };
    let cta_style = if dialog.phase == SetupPhase::Running {
        Style::default().fg(PATH_FG).bg(BG_PANEL)
    } else {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(BG_SELECTED)
            .add_modifier(Modifier::BOLD)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(cta, cta_style),
            Span::styled(
                format!("  {}", dialog.hint()),
                Style::default().fg(PATH_FG).bg(BG_PANEL),
            ),
        ])),
        Rect {
            x: inner.x,
            y: hint_y,
            width: inner.width,
            height: 1,
        },
    );
}
