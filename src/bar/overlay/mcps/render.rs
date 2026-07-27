//! MCP management panel rendering.

use super::state::{
    ActionButton, FocusZone, McpsState, PanelHover, CLOSE_BUTTON_COLS, CLOSE_BUTTON_LABEL,
};
use crate::bar::overlay::panel_content_rect;
use crate::bar::settings::point_in_rect;
use crate::bar::ui::{
    BG_BASE, BG_HIGHLIGHT, BG_SELECTED, CLOSE_HOVER_FG, DONE_GREEN, PATH_FG, TEXT_PRIMARY,
    TEXT_SELECTED, WARM_ACCENT,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

#[derive(Debug, Clone, Default)]
pub struct ClickTargets {
    pub close: Rect,
    pub open_obot: Rect,
    pub search: Rect,
    pub refresh: Rect,
    pub sync_all: Rect,
    pub dry_run: Rect,
    /// (row_index, agent_col, rect) for enablement checkboxes
    pub cells: Vec<(usize, usize, Rect)>,
    pub rows: Vec<Rect>,
    /// (absolute search_results index, rect)
    pub search_rows: Vec<(usize, Rect)>,
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

fn render_button(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    focused: bool,
    hovered: bool,
) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }
    let border = if focused || hovered {
        Style::default().fg(TEXT_SELECTED)
    } else {
        Style::default().fg(PATH_FG)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(Style::default().bg(BG_BASE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let style = if focused || hovered {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(BG_BASE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(BG_BASE)
    };
    let text = format!(" {label} ");
    frame.render_widget(Paragraph::new(Span::styled(text, style)), inner);
    area
}

fn truncate(s: &str, max: usize) -> String {
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

pub fn draw_screen(frame: &mut Frame<'_>, state: &McpsState, hover: &PanelHover) -> ClickTargets {
    let mut targets = ClickTargets::default();
    let pane = frame.area();
    // Pane can briefly report 0×0 during respawn; never index into a zero buffer.
    if pane.width < 20 || pane.height < 8 {
        paint_opaque(frame, pane, BG_BASE);
        if pane.width > 0 && pane.height > 0 {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "MCPs…",
                    Style::default().fg(TEXT_PRIMARY).bg(BG_BASE),
                )),
                pane,
            );
        }
        return targets;
    }
    paint_opaque(frame, pane, BG_BASE);
    // Grok-matching outer margins (sessions list / New Session keep their own layout).
    let content = panel_content_rect(pane);

    if state.search_open {
        return draw_search_screen(frame, state, hover, content, targets);
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title + status
            Constraint::Length(2), // agents line + buttons
            Constraint::Min(6),    // table
            Constraint::Length(5), // drift
            Constraint::Length(4), // actions + hint
            Constraint::Length(1), // status line
        ])
        .split(content);

    // ── Header ──────────────────────────────────────────────────────────
    let title_row = chunks[0];
    render_header(frame, state, hover, title_row, &mut targets);

    // ── Agents + header buttons ─────────────────────────────────────────
    let meta = chunks[1];
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Agents detected: ",
                Style::default().fg(PATH_FG).bg(BG_BASE),
            ),
            Span::styled(
                state.agents_summary(),
                Style::default().fg(TEXT_PRIMARY).bg(BG_BASE),
            ),
        ])),
        Rect {
            x: meta.x,
            y: meta.y,
            width: meta.width.saturating_sub(40),
            height: 1,
        },
    );

    let btn_y = meta.y;
    let btn_h = 3u16.min(meta.height.max(1));
    let open_w = 11u16;
    let search_w = 10u16;
    let refresh_w = 11u16;
    let btn_gap = 1u16;
    let total_btn = open_w + search_w + refresh_w + btn_gap * 2;
    let open_x = meta
        .x
        .saturating_add(meta.width.saturating_sub(total_btn + 1));
    targets.open_obot = render_button(
        frame,
        Rect {
            x: open_x,
            y: btn_y,
            width: open_w,
            height: btn_h.min(3),
        },
        ActionButton::OpenObot.label(),
        state.focus == FocusZone::Actions && state.action_focus == ActionButton::OpenObot,
        hover.open_obot,
    );
    targets.search = render_button(
        frame,
        Rect {
            x: open_x.saturating_add(open_w + btn_gap),
            y: btn_y,
            width: search_w,
            height: btn_h.min(3),
        },
        ActionButton::Search.label(),
        state.focus == FocusZone::Actions && state.action_focus == ActionButton::Search,
        hover.search,
    );
    targets.refresh = render_button(
        frame,
        Rect {
            x: open_x.saturating_add(open_w + btn_gap + search_w + btn_gap),
            y: btn_y,
            width: refresh_w,
            height: btn_h.min(3),
        },
        ActionButton::Refresh.label(),
        state.focus == FocusZone::Actions && state.action_focus == ActionButton::Refresh,
        hover.refresh,
    );

    // ── Table ───────────────────────────────────────────────────────────
    let table_area = chunks[2];
    let table_focused = state.focus == FocusZone::Table;
    let border = if table_focused {
        Style::default().fg(TEXT_SELECTED)
    } else {
        Style::default().fg(PATH_FG)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::styled(
            " SERVERS ",
            Style::default().fg(TEXT_PRIMARY).bg(BG_BASE),
        ))
        .style(Style::default().bg(BG_BASE));
    let inner = block.inner(table_area);
    frame.render_widget(block, table_area);

    if state.servers.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "No MCP servers yet. Press / to search catalog, or open Catalog.",
                Style::default().fg(PATH_FG).bg(BG_BASE),
            )])),
            Rect {
                x: inner.x.saturating_add(1),
                y: inner.y.saturating_add(1),
                width: inner.width.saturating_sub(1),
                height: 1,
            },
        );
    } else {
        render_table(frame, inner, state, hover, &mut targets);
    }

    // ── Drift ───────────────────────────────────────────────────────────
    let drift_area = chunks[3];
    let drift_focused = state.focus == FocusZone::Drift;
    let drift_border = if drift_focused {
        Style::default().fg(TEXT_SELECTED)
    } else {
        Style::default().fg(PATH_FG)
    };
    let drift_block = Block::default()
        .borders(Borders::ALL)
        .border_style(drift_border)
        .title(Span::styled(
            " DRIFT ",
            Style::default().fg(TEXT_PRIMARY).bg(BG_BASE),
        ))
        .style(Style::default().bg(BG_BASE));
    let drift_inner = drift_block.inner(drift_area);
    frame.render_widget(drift_block, drift_area);
    if state.drift.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No drift detected.",
                Style::default().fg(PATH_FG).bg(BG_BASE),
            )),
            Rect {
                x: drift_inner.x.saturating_add(1),
                y: drift_inner.y,
                width: drift_inner.width.saturating_sub(1),
                height: drift_inner.height,
            },
        );
    } else {
        let max_lines = drift_inner.height as usize;
        for (i, item) in state.drift.iter().take(max_lines).enumerate() {
            let selected = drift_focused && i == state.selected_drift;
            let style = if selected {
                Style::default()
                    .fg(TEXT_SELECTED)
                    .bg(BG_SELECTED)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(WARM_ACCENT).bg(BG_BASE)
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!(
                        " ⚠ {}",
                        truncate(&item.detail, drift_inner.width.saturating_sub(2) as usize)
                    ),
                    style,
                )),
                Rect {
                    x: drift_inner.x.saturating_add(1),
                    y: drift_inner.y.saturating_add(i as u16),
                    width: drift_inner.width.saturating_sub(1),
                    height: 1,
                },
            );
        }
    }

    // ── Actions ─────────────────────────────────────────────────────────
    let actions = chunks[4];
    let actions_focused = state.focus == FocusZone::Actions;
    let sync_w = 12u16;
    let dry_w = 11u16;
    targets.sync_all = render_button(
        frame,
        Rect {
            x: actions.x,
            y: actions.y,
            width: sync_w,
            height: 3,
        },
        ActionButton::SyncAll.label(),
        actions_focused && state.action_focus == ActionButton::SyncAll,
        hover.sync_all,
    );
    targets.dry_run = render_button(
        frame,
        Rect {
            x: actions.x.saturating_add(sync_w + 1),
            y: actions.y,
            width: dry_w,
            height: 3,
        },
        ActionButton::DryRun.label(),
        actions_focused && state.action_focus == ActionButton::DryRun,
        hover.dry_run,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(
                "last sync: {} · {} enablement on · not written until Sync",
                state.last_sync, state.staged_changes
            ),
            Style::default().fg(PATH_FG).bg(BG_BASE),
        )),
        Rect {
            x: actions.x.saturating_add(sync_w + 1 + dry_w + 2),
            y: actions.y.saturating_add(1),
            width: actions.width.saturating_sub(sync_w + 1 + dry_w + 2),
            height: 1,
        },
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "After sync, restart agents.  / search · j/k · space · o catalog · r · s sync · d dry-run · esc",
            Style::default().fg(PATH_FG).bg(BG_BASE),
        )),
        Rect {
            x: actions.x,
            y: actions
                .y
                .saturating_add(3)
                .min(actions.y.saturating_add(actions.height.saturating_sub(1))),
            width: actions.width,
            height: 1,
        },
    );

    // ── Status ──────────────────────────────────────────────────────────
    frame.render_widget(
        Paragraph::new(Span::styled(
            state.status.as_str(),
            Style::default().fg(TEXT_PRIMARY).bg(BG_BASE),
        )),
        chunks[5],
    );

    if let Some(setup) = state.setup.as_ref() {
        crate::bar::overlay::setup_dialog::draw(frame, setup);
    }

    targets
}

fn render_header(
    frame: &mut Frame<'_>,
    state: &McpsState,
    hover: &PanelHover,
    title_row: Rect,
    targets: &mut ClickTargets,
) {
    let title = if state.search_open {
        "MCPs · Search"
    } else {
        "MCPs"
    };
    let title_style = Style::default()
        .fg(TEXT_SELECTED)
        .bg(BG_BASE)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(title, title_style)])),
        Rect {
            x: title_row.x,
            y: title_row.y,
            width: title_row.width.saturating_sub(CLOSE_BUTTON_COLS + 1),
            height: 1,
        },
    );
    targets.close = render_close_button(
        frame,
        Rect {
            x: title_row.x,
            y: title_row.y,
            width: title_row.width,
            height: 1,
        },
        hover.close,
    );

    let status_dot = if state.obot_up { "●" } else { "○" };
    let status_color = if state.obot_up {
        DONE_GREEN
    } else {
        CLOSE_HOVER_FG
    };
    let manager_line = Line::from(vec![
        Span::styled(
            "sessions · MCP portal",
            Style::default().fg(PATH_FG).bg(BG_BASE),
        ),
        Span::styled("   ", Style::default().bg(BG_BASE)),
        Span::styled("manager ", Style::default().fg(PATH_FG).bg(BG_BASE)),
        Span::styled(
            format!("{status_dot} {}", state.obot_status),
            Style::default().fg(status_color).bg(BG_BASE),
        ),
        Span::styled(
            format!("  {}", state.obot_url),
            Style::default().fg(PATH_FG).bg(BG_BASE),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(manager_line),
        Rect {
            x: title_row.x,
            y: title_row.y.saturating_add(1),
            width: title_row.width,
            height: 1,
        },
    );
}

fn draw_search_screen(
    frame: &mut Frame<'_>,
    state: &McpsState,
    hover: &PanelHover,
    content: Rect,
    mut targets: ClickTargets,
) -> ClickTargets {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Length(3), // search box
            Constraint::Min(6),    // results
            Constraint::Length(2), // hint
            Constraint::Length(1), // status
        ])
        .split(content);

    render_header(frame, state, hover, chunks[0], &mut targets);

    // Search input
    let query_area = chunks[1];
    let q_border = Style::default().fg(TEXT_SELECTED);
    let q_block = Block::default()
        .borders(Borders::ALL)
        .border_style(q_border)
        .title(Span::styled(
            " SEARCH ",
            Style::default().fg(TEXT_PRIMARY).bg(BG_BASE),
        ))
        .style(Style::default().bg(BG_BASE));
    let q_inner = q_block.inner(query_area);
    frame.render_widget(q_block, query_area);
    let cursor = if state.search_busy { "…" } else { "▌" };
    let query_line = format!(" /{}{cursor}", state.search_query);
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate(&query_line, q_inner.width as usize),
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        )),
        q_inner,
    );

    // Results
    let results_area = chunks[2];
    let r_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(TEXT_SELECTED))
        .title(Span::styled(
            format!(" CATALOG ({} matches) ", state.search_results.len()),
            Style::default().fg(TEXT_PRIMARY).bg(BG_BASE),
        ))
        .style(Style::default().bg(BG_BASE));
    let r_inner = r_block.inner(results_area);
    frame.render_widget(r_block, results_area);

    if !state.catalog_error.is_empty() && state.search_results.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                truncate(
                    &state.catalog_error,
                    r_inner.width.saturating_sub(2) as usize,
                ),
                Style::default().fg(WARM_ACCENT).bg(BG_BASE),
            )),
            Rect {
                x: r_inner.x.saturating_add(1),
                y: r_inner.y.saturating_add(1),
                width: r_inner.width.saturating_sub(1),
                height: 2,
            },
        );
    } else if state.search_results.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                if state.search_query.is_empty() {
                    "No catalog entries. Open Catalog in browser or set up the MCP manager."
                } else {
                    "No matches."
                },
                Style::default().fg(PATH_FG).bg(BG_BASE),
            )),
            Rect {
                x: r_inner.x.saturating_add(1),
                y: r_inner.y.saturating_add(1),
                width: r_inner.width.saturating_sub(1),
                height: 1,
            },
        );
    } else {
        render_search_results(frame, r_inner, state, hover, &mut targets);
    }

    frame.render_widget(
        Paragraph::new(Span::styled(
            "Type to filter · ↑↓/Ctrl-n/p select · Enter add · Ctrl-r reload catalog · esc back",
            Style::default().fg(PATH_FG).bg(BG_BASE),
        )),
        chunks[3],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            state.status.as_str(),
            Style::default().fg(TEXT_PRIMARY).bg(BG_BASE),
        )),
        chunks[4],
    );

    targets
}

fn render_search_results(
    frame: &mut Frame<'_>,
    inner: Rect,
    state: &McpsState,
    hover: &PanelHover,
    targets: &mut ClickTargets,
) {
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let visible = inner.height as usize;
    let start = state
        .search_selected
        .saturating_sub(visible.saturating_sub(1) / 2)
        .min(state.search_results.len().saturating_sub(visible));
    let end = (start + visible).min(state.search_results.len());

    for (vis_i, abs_idx) in (start..end).enumerate() {
        let row = &state.search_results[abs_idx];
        let y = inner.y.saturating_add(vis_i as u16);
        let selected = abs_idx == state.search_selected;
        let hovered = hover.search_row == Some(abs_idx);
        let bg = if selected {
            BG_SELECTED
        } else if hovered {
            BG_HIGHLIGHT
        } else {
            BG_BASE
        };
        let row_rect = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        fill_rect(frame, row_rect, bg);
        targets.search_rows.push((abs_idx, row_rect));

        let badge = if row.installed {
            "installed"
        } else if !row.entry.oauth_configured {
            "needs config"
        } else {
            "add"
        };
        let summary = row.entry.summary();
        let name = truncate(&row.entry.name, 22);
        let line = format!(
            " {:<22}  {:<12}  {}",
            name,
            badge,
            truncate(summary, inner.width.saturating_sub(40) as usize)
        );
        let style = if selected {
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT_PRIMARY).bg(bg)
        };
        frame.render_widget(
            Paragraph::new(Span::styled(truncate(&line, inner.width as usize), style)),
            row_rect,
        );
    }
}

fn render_table(
    frame: &mut Frame<'_>,
    inner: Rect,
    state: &McpsState,
    hover: &PanelHover,
    targets: &mut ClickTargets,
) {
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let agent_w = 7u16;
    let agent_count = state.agents.len() as u16;
    let agents_total = agent_count.saturating_mul(agent_w);
    let auth_w = 8u16;
    let source_w = 8u16;
    let name_w = inner
        .width
        .saturating_sub(agents_total + auth_w + source_w + 2)
        .max(10);

    // Header
    let mut header_spans = vec![
        Span::styled(
            format!(" {:<w$}", "SERVER", w = name_w as usize),
            Style::default()
                .fg(PATH_FG)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<w$}", "SOURCE", w = source_w as usize),
            Style::default()
                .fg(PATH_FG)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<w$}", "AUTH", w = auth_w as usize),
            Style::default()
                .fg(PATH_FG)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    for agent in &state.agents {
        header_spans.push(Span::styled(
            format!(
                "{:<w$}",
                truncate(&agent.label, agent_w as usize),
                w = agent_w as usize
            ),
            Style::default()
                .fg(PATH_FG)
                .bg(BG_BASE)
                .add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(header_spans)),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );

    let body_y = inner.y.saturating_add(1);
    let body_h = inner.height.saturating_sub(1);
    let visible = body_h as usize;
    if visible == 0 {
        return;
    }

    let start = state
        .selected_row
        .saturating_sub(visible.saturating_sub(1) / 2)
        .min(state.servers.len().saturating_sub(visible));
    let end = (start + visible).min(state.servers.len());

    for (vis_i, row_idx) in (start..end).enumerate() {
        let server = &state.servers[row_idx];
        let y = body_y.saturating_add(vis_i as u16);
        let selected = state.focus == FocusZone::Table && row_idx == state.selected_row;
        let hovered = hover.row == Some(row_idx);
        let bg = if selected {
            BG_SELECTED
        } else if hovered {
            BG_HIGHLIGHT
        } else {
            BG_BASE
        };
        let row_rect = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        fill_rect(frame, row_rect, bg);
        targets.rows.push(row_rect);

        let fg = if selected {
            TEXT_SELECTED
        } else {
            TEXT_PRIMARY
        };
        let base = Style::default().fg(fg).bg(bg);

        let mut x = inner.x;
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    " {:<w$}",
                    truncate(&server.display_name, name_w as usize),
                    w = name_w as usize
                ),
                base,
            )),
            Rect {
                x,
                y,
                width: name_w,
                height: 1,
            },
        );
        x = x.saturating_add(name_w);

        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    "{:<w$}",
                    truncate(&server.source, source_w as usize),
                    w = source_w as usize
                ),
                base,
            )),
            Rect {
                x,
                y,
                width: source_w,
                height: 1,
            },
        );
        x = x.saturating_add(source_w);

        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    "{:<w$}",
                    truncate(&server.auth, auth_w as usize),
                    w = auth_w as usize
                ),
                base,
            )),
            Rect {
                x,
                y,
                width: auth_w,
                height: 1,
            },
        );
        x = x.saturating_add(auth_w);

        for (col, enabled) in server.enabled.iter().enumerate() {
            let cell_focused = selected && state.selected_agent == col;
            let cell_hovered = hover.row == Some(row_idx) && hover.agent_col == Some(col);
            let mark = if *enabled { "[x]" } else { "[ ]" };
            let cell_style = if cell_focused || cell_hovered {
                Style::default()
                    .fg(TEXT_SELECTED)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(fg).bg(bg)
            };
            let cell_rect = Rect {
                x,
                y,
                width: agent_w,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!("{:<w$}", mark, w = agent_w as usize),
                    cell_style,
                )),
                cell_rect,
            );
            targets.cells.push((row_idx, col, cell_rect));
            x = x.saturating_add(agent_w);
        }
    }
}

#[allow(dead_code)]
pub fn cell_at(targets: &ClickTargets, col: u16, row: u16) -> Option<(usize, usize)> {
    for (r, c, rect) in &targets.cells {
        if point_in_rect(col, row, *rect) {
            return Some((*r, *c));
        }
    }
    None
}
