//! Skills panel rendering.

use super::state::{
    ActionId, FocusSection, LibraryRow, PanelHover, SkillsState, CLOSE_BUTTON_COLS,
    CLOSE_BUTTON_LABEL,
};
use crate::bar::overlay::panel_content_rect;
use crate::bar::ui::{
    BG_BASE, BG_PANEL, BG_SELECTED, DONE_FG, PATH_FG, TEXT_PRIMARY, TEXT_SECONDARY, TEXT_SELECTED,
    WARM_ACCENT,
};
use crate::skills::SkillAgent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

#[derive(Debug, Clone, Default)]
pub struct ClickTargets {
    pub close: Rect,
    pub actions: Vec<(ActionId, Rect)>,
    pub rows: Vec<Rect>,
}

pub fn draw_screen(frame: &mut Frame, state: &SkillsState, hover: &PanelHover) -> ClickTargets {
    let area = frame.area();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(BG_BASE).fg(TEXT_PRIMARY)),
        area,
    );

    // Grok-matching outer margins (sessions list / New Session keep their own layout).
    let content = panel_content_rect(area);

    let mut targets = ClickTargets::default();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title + subtitle + close
            Constraint::Length(4), // manager status
            Constraint::Length(3), // actions
            Constraint::Min(6),    // library
            Constraint::Length(6), // drift
            Constraint::Length(2), // footer
        ])
        .split(content);

    draw_title(frame, chunks[0], hover, &mut targets);
    draw_status(frame, chunks[1], state);
    draw_actions(frame, chunks[2], state, hover, &mut targets);
    draw_library(frame, chunks[3], state, hover, &mut targets);
    draw_drift(frame, chunks[4], state);
    draw_footer(frame, chunks[5], state);

    if let Some(setup) = state.setup.as_ref() {
        crate::bar::overlay::setup_dialog::draw(frame, setup);
    }

    targets
}

fn render_close_button(frame: &mut Frame, row: Rect, hovered: bool) -> Rect {
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

fn draw_title(frame: &mut Frame, area: Rect, hover: &PanelHover, targets: &mut ClickTargets) {
    // Title row: "Skills" left, [esc] right (matches MCPs / Automations).
    let title_row = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Skills",
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        )])),
        Rect {
            x: title_row.x,
            y: title_row.y,
            width: title_row.width.saturating_sub(CLOSE_BUTTON_COLS + 1),
            height: 1,
        },
    );
    targets.close = render_close_button(frame, title_row, hover.close);

    // sessions-themed subtitle (manager detail lives in the status block).
    if area.height > 1 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "sessions · Skills portal",
                Style::default().fg(TEXT_SECONDARY).bg(BG_BASE),
            )),
            Rect {
                x: area.x,
                y: area.y.saturating_add(1),
                width: area.width,
                height: 1,
            },
        );
    }
}

fn draw_status(frame: &mut Frame, area: Rect, state: &SkillsState) {
    let ss = &state.skillshare;
    let (dot, color) = if ss.installed {
        ("●", DONE_FG)
    } else {
        ("○", WARM_ACCENT)
    };
    let version = ss
        .version
        .as_deref()
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    let bin = ss
        .binary
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "not found".into());
    let store = format!(
        "store: {}  ({} skills)",
        ss.store_dir.display(),
        state.store_skill_count()
    );
    // One-col pad inside the border (outer Grok margins are on the panel content rect).
    let lines = vec![
        Line::from(vec![
            Span::styled(format!(" {dot} manager"), Style::default().fg(color)),
            Span::styled(
                if ss.installed {
                    format!("{version}  {bin}")
                } else {
                    "  not installed".into()
                },
                Style::default().fg(TEXT_SECONDARY),
            ),
        ]),
        Line::from(Span::styled(
            format!("   {store}"),
            Style::default().fg(TEXT_SECONDARY),
        )),
        Line::from(Span::styled(
            format!(
                "   {}",
                truncate(&state.status, area.width.saturating_sub(4) as usize)
            ),
            Style::default().fg(if state.busy {
                WARM_ACCENT
            } else {
                TEXT_PRIMARY
            }),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(TEXT_SECONDARY))
                .title(" Manager "),
        ),
        area,
    );
}

fn draw_actions(
    frame: &mut Frame,
    area: Rect,
    state: &SkillsState,
    hover: &PanelHover,
    targets: &mut ClickTargets,
) {
    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: 1,
    };
    let mut spans = Vec::new();
    let mut x = inner.x;
    for (idx, action) in ActionId::ALL.iter().enumerate() {
        let focused = state.focus == FocusSection::Actions && state.action_idx == idx;
        let hovered = hover.action == Some(*action);
        let style = if focused || hovered {
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(BG_SELECTED)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT_PRIMARY).bg(BG_PANEL)
        };
        let label = format!(" [{}] {} ", action.key(), action.label());
        let w = label.chars().count() as u16;
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        targets.actions.push((
            *action,
            Rect {
                x,
                y: inner.y,
                width: w,
                height: 1,
            },
        ));
        x = x.saturating_add(w + 1);
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(TEXT_SECONDARY))
                .title(" Actions "),
        ),
        area,
    );
}

fn draw_library(
    frame: &mut Frame,
    area: Rect,
    state: &SkillsState,
    hover: &PanelHover,
    targets: &mut ClickTargets,
) {
    let rows = state.library_rows();
    let header = agent_header();
    let mut lines = vec![Line::from(Span::styled(
        header,
        Style::default().fg(TEXT_SECONDARY),
    ))];

    let inner_h = area.height.saturating_sub(2) as usize;
    let visible = inner_h.saturating_sub(1).max(1);
    let scroll = state.list_scroll.min(rows.len().saturating_sub(visible));

    targets.rows.clear();
    for (i, row) in rows.iter().enumerate().skip(scroll).take(visible) {
        let abs = i;
        let selected = state.focus == FocusSection::Library && state.selected == abs;
        let hovered = hover.row == Some(abs);
        let style = if selected || hovered {
            Style::default().fg(TEXT_SELECTED).bg(BG_SELECTED)
        } else {
            Style::default().fg(TEXT_PRIMARY)
        };
        lines.push(Line::from(Span::styled(format_row(row, area.width), style)));
        let y = area.y + 2 + (abs.saturating_sub(scroll) as u16);
        if y < area.y + area.height {
            targets.rows.push(Rect {
                x: area.x + 1,
                y,
                width: area.width.saturating_sub(2),
                height: 1,
            });
        }
    }
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            " No skills found in store or agent dirs.",
            Style::default().fg(TEXT_SECONDARY),
        )));
    }

    let title = format!(" Library ({}) ", rows.len());
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(
                    Style::default().fg(if state.focus == FocusSection::Library {
                        WARM_ACCENT
                    } else {
                        TEXT_SECONDARY
                    }),
                )
                .title(title),
        ),
        area,
    );
}

fn agent_header() -> String {
    let mut s = format!(" {:<22} ", "skill");
    for agent in SkillAgent::ALL {
        s.push_str(&format!("{:>3} ", agent.short()));
    }
    s.push_str("  src");
    s
}

fn format_row(row: &LibraryRow, width: u16) -> String {
    let mut s = format!(" {:<22} ", truncate(&row.name, 22));
    for (_, present) in &row.presence {
        s.push_str(if *present { " ■  " } else { " ·  " });
    }
    s.push_str(if row.in_store { " store" } else { " local" });
    if !row.description.is_empty() && width > 70 {
        s.push_str("  ");
        s.push_str(&truncate(&row.description, 40));
    }
    s
}

fn draw_drift(frame: &mut Frame, area: Rect, state: &SkillsState) {
    let missing = state.missing_drift();
    let mut lines = Vec::new();
    if missing.is_empty() && !state.inventory.store_skills.is_empty() {
        lines.push(Line::from(Span::styled(
            " ✓ store skills present on scanned agents (or no targets)",
            Style::default().fg(DONE_FG),
        )));
    } else if state.inventory.store_skills.is_empty() {
        lines.push(Line::from(Span::styled(
            " Store empty — run Init or install skills, then Sync",
            Style::default().fg(TEXT_SECONDARY),
        )));
    }
    for item in missing.iter().take(4) {
        lines.push(Line::from(Span::styled(
            format!(" ⚠ {}", item.detail),
            Style::default().fg(WARM_ACCENT),
        )));
    }
    let extra = missing.len().saturating_sub(4);
    if extra > 0 {
        lines.push(Line::from(Span::styled(
            format!(" … +{extra} more"),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }
    let only_agent = state
        .drift
        .iter()
        .filter(|d| d.kind == crate::skills::DriftKind::OnlyOnAgent)
        .count();
    if only_agent > 0 {
        lines.push(Line::from(Span::styled(
            format!(" {only_agent} skill(s) only on agents (not in store)"),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if state.focus == FocusSection::Drift {
                    WARM_ACCENT
                } else {
                    TEXT_SECONDARY
                }))
                .title(" Drift "),
        ),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, _state: &SkillsState) {
    let line = Line::from(Span::styled(
        "q/Esc close · Tab focus · ↑↓ select · i init · s sync · u ui · a audit · r reload · U setup",
        Style::default().fg(TEXT_SECONDARY),
    ));
    frame.render_widget(Paragraph::new(line), area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
