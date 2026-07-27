//! New-session Ratatui rendering.

use super::state::{
    union_rect, Focus, NewSessionState, PanelHover, PathGhostHint, WorkspacePopupEntry,
    WorkspacePopupKind, CLOSE_BUTTON_COLS, CLOSE_BUTTON_LABEL, FIELD_INNER_HEIGHT,
    MAX_DROPDOWN_VISIBLE, NEW_WORKSPACE_HEADER, PROMPT_INNER_HEIGHT, SECTION_GAP, TITLE_ROWS,
    WORKSPACE_HEADER_ROWS,
};
use crate::agents;
use crate::bar::art_canvas;
use crate::bar::mouse_cursor::{self, MouseCursorShape};
use crate::bar::notepad;
use crate::bar::settings::point_in_rect;
use crate::bar::ui::{
    BG_BASE, BG_HIGHLIGHT, BG_HOVER_SELECTED, BG_PANEL, BG_SELECTED, CLOSE_HOVER_FG, PATH_FG,
    TEXT_PRIMARY, TEXT_SELECTED,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::block::Padding;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

pub(crate) const BG_FIELD: ratatui::style::Color = BG_BASE;

#[derive(Debug, Clone, Default)]
pub(crate) struct ClickTargets {
    pub form: Rect,
    pub workspace: Rect,
    pub workspace_field: Rect,
    pub workspace_popup: Rect,
    pub agent: Rect,
    pub agent_field: Rect,
    pub agent_popup: Rect,
    pub model: Rect,
    pub model_field: Rect,
    pub model_popup: Rect,
    pub prompt: Rect,
    pub prompt_field: Rect,
    pub foreground_button: Rect,
    pub background_button: Rect,
    pub close: Rect,
}

pub(crate) fn field_block_height(inner: u16) -> u16 {
    inner + 2
}

fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = a.x.saturating_add(a.width).min(b.x.saturating_add(b.width));
    let bottom =
        a.y.saturating_add(a.height)
            .min(b.y.saturating_add(b.height));
    if right <= x || bottom <= y {
        None
    } else {
        Some(Rect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }
}

fn rects_overlap(a: Rect, b: Rect) -> bool {
    intersect_rect(a, b).is_some()
}

fn fill_rect(frame: &mut ratatui::Frame, area: Rect, bg: ratatui::style::Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
}

/// Erase underlying cells then paint an opaque backdrop (dropdown popups overlap fields below).
fn paint_opaque_rect(frame: &mut ratatui::Frame, area: Rect, bg: ratatui::style::Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Clear, area);
    fill_rect(frame, area, bg);
}

fn split_field_area(area: Rect, _show_enter: bool) -> (Rect, Rect) {
    // Enter affordance removed; always use full area for the field.
    (area, Rect::default())
}

fn compute_dropdown_menu_rect(
    anchor: Rect,
    option_count: usize,
    selected_idx: usize,
    open: bool,
    frame_area: Rect,
) -> Rect {
    if !open || option_count == 0 {
        return anchor;
    }
    let max_inner = max_dropdown_inner_rows(anchor, frame_area);
    let (start, visible) = popup_window(option_count, selected_idx, max_inner);
    if visible == 0 {
        return anchor;
    }
    let _ = start;
    Rect {
        x: anchor.x,
        y: anchor.y,
        width: anchor.width,
        height: visible as u16 + 2,
    }
}

fn compute_workspace_menu_rect(
    anchor: Rect,
    entry_count: usize,
    highlight_idx: usize,
    frame_area: Rect,
) -> Rect {
    if entry_count == 0 {
        return anchor;
    }
    let frame_bottom = frame_area.y.saturating_add(frame_area.height);
    let max_list_rows = frame_bottom.saturating_sub(anchor.y + 2 + WORKSPACE_HEADER_ROWS) as usize;
    let (_, visible) = workspace_list_window(entry_count, highlight_idx, max_list_rows);
    if visible == 0 {
        return anchor;
    }
    Rect {
        x: anchor.x,
        y: anchor.y,
        width: anchor.width,
        height: WORKSPACE_HEADER_ROWS
            .saturating_add(visible as u16)
            .saturating_add(2)
            .max(anchor.height),
    }
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

fn dropdown_field_height() -> u16 {
    field_block_height(FIELD_INNER_HEIGHT)
}

pub(crate) fn modal_content_height() -> u16 {
    // Full form height (for tests and default sizing). Order: Agent, Model, Session Path, Prompt.
    let workspace_h = dropdown_field_height();
    let agent_h = dropdown_field_height();
    let model_h = dropdown_field_height();
    let prompt_h = field_block_height(PROMPT_INNER_HEIGHT);
    let button_h = field_block_height(FIELD_INNER_HEIGHT);
    TITLE_ROWS
        + 1
        + 1
        + agent_h
        + SECTION_GAP
        + 1
        + model_h
        + SECTION_GAP
        + 1
        + workspace_h
        + SECTION_GAP
        + 1
        + prompt_h
        + SECTION_GAP
        + button_h
        + 2
        + 1
}

pub(crate) fn modal_content_height_for_state(state: &NewSessionState) -> u16 {
    let workspace_h = dropdown_field_height();
    let agent_h = dropdown_field_height();
    let button_h = field_block_height(FIELD_INNER_HEIGHT);
    let is_console = state.selected_agent().id == "console";
    // Agent + Session Path always present; Model + Prompt hidden for console.
    let mut h = TITLE_ROWS + 1 + 1 + agent_h + SECTION_GAP + 1 + workspace_h + SECTION_GAP;
    if !is_console {
        let model_h = dropdown_field_height();
        let prompt_h = field_block_height(PROMPT_INNER_HEIGHT);
        h += 1 + model_h + SECTION_GAP + 1 + prompt_h + SECTION_GAP;
    }
    h += button_h + 2 + 1;
    h
}

pub(crate) struct NewSessionLayout {
    pub(crate) form: Rect,
}

pub(crate) fn new_session_layout(pane: Rect, state: &NewSessionState) -> NewSessionLayout {
    let form_width = art_canvas::pane_fraction_width(pane.width).max(40);
    let form_x = pane.x + pane.width.saturating_sub(form_width) / 2;
    let desired_h = modal_content_height_for_state(state);
    let form_height = desired_h.min(pane.height.max(1));
    let top_margin = if pane.height > form_height + 4 {
        (pane.height.saturating_sub(form_height)) / 2
    } else {
        2
    };
    let form_y = pane.y.saturating_add(top_margin);
    NewSessionLayout {
        form: Rect {
            x: form_x,
            y: form_y,
            width: form_width,
            height: form_height,
        },
    }
}

fn paint_panel_background(frame: &mut ratatui::Frame, area: Rect) {
    frame.render_widget(Block::default().style(Style::default().bg(BG_BASE)), area);
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
    // Align with the right border column of the bordered fields below.
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
}

pub fn draw_screen(
    frame: &mut ratatui::Frame,
    state: &mut NewSessionState,
    panel_hover: &PanelHover,
) -> ClickTargets {
    let area = frame.area();
    let layout = new_session_layout(area, state);
    paint_panel_background(frame, area);

    let section = Block::default()
        .borders(Borders::NONE)
        .padding(Padding::new(1, 1, 0, 1))
        .style(Style::default().bg(BG_BASE));
    let inner = section.inner(layout.form);
    frame.render_widget(section, layout.form);

    let is_console = state.selected_agent().id == "console";

    let viewport = inner;
    let content_origin = Rect {
        x: viewport.x,
        y: viewport.y,
        width: viewport.width,
        height: modal_content_height(),
    };

    // Field order: Agent → Model → Session Path → Prompt → buttons.
    let agent_h = dropdown_field_height();
    let model_h = if is_console {
        0
    } else {
        dropdown_field_height()
    };
    let workspace_h = dropdown_field_height();
    let prompt_h = if is_console {
        0
    } else {
        field_block_height(PROMPT_INNER_HEIGHT)
    };
    let button_h = field_block_height(FIELD_INNER_HEIGHT);
    let model_gap = if is_console { 0 } else { SECTION_GAP };
    let prompt_gap = if is_console { 0 } else { SECTION_GAP };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TITLE_ROWS),                     // 0 title
            Constraint::Length(1),                              // 1 agent label
            Constraint::Length(agent_h),                        // 2 agent field
            Constraint::Length(SECTION_GAP),                    // 3
            Constraint::Length(if is_console { 0 } else { 1 }), // 4 model label
            Constraint::Length(model_h),                        // 5 model field
            Constraint::Length(model_gap),                      // 6
            Constraint::Length(1),                              // 7 path label
            Constraint::Length(workspace_h),                    // 8 path field
            Constraint::Length(SECTION_GAP),                    // 9
            Constraint::Length(if is_console { 0 } else { 1 }), // 10 prompt label
            Constraint::Length(prompt_h),                       // 11 prompt field
            Constraint::Length(prompt_gap),                     // 12
            Constraint::Length(button_h),                       // 13 buttons
            Constraint::Length(2),                              // 14
            Constraint::Length(1),                              // 15 hint
        ])
        .split(content_origin);

    let workspace_focused = state.focus == Focus::Workspace;
    let workspace_display = if workspace_focused {
        state.workspace_header_display()
    } else {
        state.workspace_committed_display()
    };
    let workspace_typing = workspace_focused && state.is_typing_path();
    let workspace_popup_entries = state.build_workspace_popup();
    // Live feedback: is the highlighted row the user's typed path that doesn't exist?
    let on_bad_path_row = workspace_popup_entries
        .get(state.workspace_popup_highlight)
        .is_some_and(|e| matches!(e.kind, WorkspacePopupKind::Path) && e.cwd.is_none());
    // Also surface error on the compact field when not focused but committed value is bad.
    let path_field_error = on_bad_path_row
        || (!workspace_focused && state.uses_custom_path() && state.path_input_error().is_some());
    let agent_labels: Vec<String> = agents::AGENTS
        .iter()
        .map(|agent| {
            let mut l = agent.label.to_string();
            // Clean any erroneous single letter prefix next to agent name (e.g. "O cursor").
            if l.len() > 2 && l.as_bytes()[1] == b' ' && l.as_bytes()[0].is_ascii_alphabetic() {
                l = l[2..].to_string();
            }
            l
        })
        .collect();
    let agent_open = state.focus == Focus::Agent;
    let model_labels: Vec<String> = state
        .selected_agent()
        .models
        .iter()
        .map(|model| model.label.to_string())
        .collect();
    let model_open = state.focus == Focus::Model && !is_console;

    let (agent_value_area, _) = split_field_area(chunks[2], agent_open);
    let model_value_area = if !is_console {
        split_field_area(chunks[5], model_open).0
    } else {
        Rect::default()
    };
    let (workspace_value_area, _) = split_field_area(chunks[8], workspace_focused);
    let workspace_popup_rect = if workspace_focused {
        compute_workspace_menu_rect(
            workspace_value_area,
            workspace_popup_entries.len(),
            state.workspace_popup_highlight,
            area,
        )
    } else {
        Rect::default()
    };
    let agent_popup_rect = if agent_open {
        compute_dropdown_menu_rect(chunks[2], agent_labels.len(), state.agent_idx, true, area)
    } else {
        Rect::default()
    };
    let model_popup_rect = if model_open {
        compute_dropdown_menu_rect(chunks[5], model_labels.len(), state.model_idx, true, area)
    } else {
        Rect::default()
    };

    let overlays = [workspace_popup_rect, agent_popup_rect, model_popup_rect]
        .into_iter()
        .filter(|rect| rect.width > 0 && rect.height > 0)
        .collect::<Vec<_>>();
    let covered =
        |rect: Rect| -> bool { overlays.iter().any(|overlay| rects_overlap(rect, *overlay)) };

    let render_if_visible =
        |chunk: Rect, viewport: Rect| -> Option<Rect> { intersect_rect(chunk, viewport) };

    let mut close_target = Rect::default();
    if let Some(visible) = render_if_visible(chunks[0], viewport) {
        let close_width = CLOSE_BUTTON_COLS.min(visible.width);
        close_target = Rect {
            x: visible
                .x
                .saturating_add(visible.width.saturating_sub(close_width)),
            y: visible.y,
            width: close_width,
            height: visible.height,
        };
        let title_width = close_target.x.saturating_sub(visible.x);
        let title_area = Rect {
            x: visible.x,
            y: visible.y,
            width: title_width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                NEW_WORKSPACE_HEADER,
                Style::default()
                    .fg(TEXT_SELECTED)
                    .bg(BG_BASE)
                    .add_modifier(Modifier::BOLD),
            )),
            title_area,
        );
        render_close_button(frame, visible, panel_hover.close);
    }

    let agent_tag = if state.focus == Focus::Agent && state.is_default_focus() {
        " *"
    } else if state.agent_confirmed {
        " ✓"
    } else {
        ""
    };
    if let Some(visible) = render_if_visible(chunks[1], viewport) {
        if !covered(visible) {
            render_field_label(
                frame,
                visible,
                &format!("Agent{agent_tag}"),
                state.focus == Focus::Agent,
                state.agent_confirmed,
            );
        }
    }

    if !is_console {
        let model_tag = if state.focus == Focus::Model && state.is_default_focus() {
            " *"
        } else if state.model_confirmed {
            " ✓"
        } else {
            ""
        };
        if let Some(visible) = render_if_visible(chunks[4], viewport) {
            if !covered(visible) {
                render_field_label(
                    frame,
                    visible,
                    &format!("Model{model_tag}"),
                    state.focus == Focus::Model,
                    state.model_confirmed,
                );
            }
        }
    }

    let session_tag = if state.is_default_focus() && state.focus == Focus::Workspace {
        " *"
    } else if state.workspace_confirmed {
        " ✓"
    } else {
        ""
    };
    if let Some(visible) = render_if_visible(chunks[7], viewport) {
        if !covered(visible) {
            render_field_label(
                frame,
                visible,
                &format!("Session Path{session_tag}"),
                state.focus == Focus::Workspace,
                state.workspace_confirmed,
            );
        }
    }

    if !agent_open {
        if let Some(visible) = render_if_visible(chunks[2], viewport) {
            if !covered(visible) {
                let (value_area, _) = split_field_area(visible, false);
                render_dropdown_collapsed(
                    frame,
                    value_area,
                    agent_labels
                        .get(state.agent_idx)
                        .map(String::as_str)
                        .unwrap_or(""),
                    false,
                    state.agent_confirmed,
                );
            }
        }
    }

    if !is_console && !model_open {
        if let Some(visible) = render_if_visible(chunks[5], viewport) {
            if !covered(visible) {
                let (value_area, _) = split_field_area(visible, false);
                render_dropdown_collapsed(
                    frame,
                    value_area,
                    model_labels
                        .get(state.model_idx)
                        .map(String::as_str)
                        .unwrap_or(""),
                    false,
                    state.model_confirmed,
                );
            }
        }
    }

    if let Some(visible) = render_if_visible(workspace_value_area, viewport) {
        if !covered(visible) {
            render_session_path_header(
                frame,
                visible,
                &workspace_display,
                workspace_focused,
                state.workspace_confirmed,
                path_field_error,
                workspace_typing,
            );
        }
    }

    if !is_console {
        if let Some(visible) = render_if_visible(chunks[10], viewport) {
            if !covered(visible) {
                render_field_label(
                    frame,
                    visible,
                    "Prompt",
                    state.focus == Focus::Prompt,
                    false,
                );
            }
        }
        if let Some(visible) = render_if_visible(chunks[11], viewport) {
            if !covered(visible) {
                let block = dropdown_block(state.focus == Focus::Prompt, false, "");
                let inner = block.inner(visible);
                state.prompt_field_width = state.prompt_content_width(inner.width);
                render_prompt_field(
                    frame,
                    visible,
                    &state.prompt,
                    state.focus == Focus::Prompt,
                    state.prompt_cursor,
                    state.prompt_scroll,
                    state.prompt_selection,
                );
            }
        }
    }
    let prompt_target = if !is_console {
        union_rect(chunks[10], chunks[11])
    } else {
        Rect::default()
    };

    let button_row = chunks[13];
    let button_width: u16 = 18;
    let button_gap: u16 = 2;
    let total_buttons = button_width.saturating_mul(2).saturating_add(button_gap);
    let button_x = button_row
        .x
        .saturating_add(button_row.width.saturating_sub(total_buttons) / 2);
    let foreground_button = Rect {
        x: button_x,
        y: button_row.y,
        width: button_width.min(button_row.width),
        height: button_row.height,
    };
    let background_button = Rect {
        x: foreground_button
            .x
            .saturating_add(foreground_button.width.saturating_add(button_gap)),
        y: button_row.y,
        width: button_width.min(
            button_row
                .width
                .saturating_sub(foreground_button.width.saturating_add(button_gap)),
        ),
        height: button_row.height,
    };

    if let Some(visible) = intersect_rect(foreground_button, viewport) {
        if !covered(visible) {
            render_submit_button(
                frame,
                visible,
                "Foreground  ⌘F",
                state.focus == Focus::ForegroundButton,
                panel_hover.foreground_button,
            );
        }
    }
    if let Some(visible) = intersect_rect(background_button, viewport) {
        if !covered(visible) {
            render_submit_button(
                frame,
                visible,
                "Background  ⌘B",
                state.focus == Focus::BackgroundButton,
                panel_hover.background_button,
            );
        }
    }

    // Show live path error in the hint only when the highlighted row is the bad typed path.
    let path_err = if workspace_focused && on_bad_path_row {
        state.path_input_error()
    } else {
        None
    };
    let hint = if let Some(ref e) = path_err {
        e.clone()
    } else if !state.status.is_empty() {
        state.status.clone()
    } else if matches!(
        state.focus,
        Focus::ForegroundButton | Focus::BackgroundButton
    ) {
        if let Some(cmd) = state.preview_launch_command() {
            format!("{cmd}  ·  ↵ or d save default")
        } else {
            "↵ launch · d save default".into()
        }
    } else {
        match state.focus {
            Focus::Workspace if state.is_typing_path() => {
                "↑↓ pick · ←→ edit path · Tab complete · type to search · ↵ confirm".into()
            }
            Focus::Workspace => {
                "↵ confirm · Tab cycle · type to search · pick active session or directory".into()
            }
            Focus::Prompt => "↵ newline · ⌘⌫ clear · ⌘A select all · Tab buttons".into(),
            _ => "↵ confirm field · d save default".into(),
        }
    };
    let hint_is_error = path_err.is_some() || !state.status.is_empty();
    if let Some(visible) = render_if_visible(chunks[15], viewport) {
        frame.render_widget(
            Paragraph::new(Span::styled(
                hint.as_str(),
                Style::default()
                    .fg(if hint_is_error {
                        CLOSE_HOVER_FG
                    } else {
                        PATH_FG
                    })
                    .bg(BG_BASE),
            )),
            visible,
        );
    }

    let workspace_ghost_hint = if workspace_focused {
        state.workspace_path_ghost_hint()
    } else {
        None
    };
    let workspace_popup = if workspace_focused {
        render_workspace_menu(
            frame,
            workspace_value_area,
            &workspace_display,
            &workspace_popup_entries,
            state.workspace_popup_highlight,
            panel_hover.workspace_popup_row,
            workspace_typing,
            state.workspace_confirmed,
            on_bad_path_row,
            workspace_ghost_hint.as_ref(),
        )
    } else {
        Rect::default()
    };

    if state.focus == Focus::Prompt && !is_console {
        if let Some(visible) = render_if_visible(chunks[11], viewport) {
            if !covered(visible) {
                let block = dropdown_block(true, false, "");
                let inner = block.inner(visible);
                if inner.height > 0 && inner.width > 2 {
                    if let Some(pos) = prompt_terminal_cursor_position(
                        inner,
                        &state.prompt,
                        state.prompt_cursor,
                        state.prompt_scroll,
                        state.prompt_field_width,
                    ) {
                        if pos.x < area.x.saturating_add(area.width)
                            && pos.y < area.y.saturating_add(area.height)
                        {
                            frame.set_cursor_position(pos);
                        }
                    }
                }
            }
        }
    }

    // Cursor in the session-path header while typing (preserves trailing `/`).
    if workspace_focused && state.is_typing_path() {
        let on_path_row = workspace_popup_entries
            .get(state.workspace_popup_highlight)
            .is_some_and(|e| e.kind == WorkspacePopupKind::Path);
        if on_path_row {
            let anchor = workspace_value_area;
            if anchor.width > 2 && anchor.height > 0 {
                let frame_bottom = area.y.saturating_add(area.height);
                let max_list_rows =
                    frame_bottom.saturating_sub(anchor.y + 2 + WORKSPACE_HEADER_ROWS) as usize;
                let (_start, visible) = if workspace_popup_entries.is_empty() {
                    (0, 0)
                } else {
                    workspace_list_window(
                        workspace_popup_entries.len(),
                        state.workspace_popup_highlight,
                        max_list_rows,
                    )
                };
                let menu_height = WORKSPACE_HEADER_ROWS
                    .saturating_add(visible as u16)
                    .saturating_add(2)
                    .max(anchor.height);
                let menu = Rect {
                    x: anchor.x,
                    y: anchor.y,
                    width: anchor.width,
                    height: menu_height,
                };
                let blk = dropdown_block(true, state.workspace_confirmed, "");
                let inner = blk.inner(menu);
                if inner.height > 0 && inner.width > 2 {
                    let text_x = inner.x.saturating_add(1);
                    let text_y = inner.y;
                    let content_w = (inner.width.saturating_sub(2)) as usize;
                    let typed = &state.workspace_path_input;
                    let shown = truncate_to_width(typed, content_w);
                    let cursor_col = state
                        .workspace_path_cursor
                        .min(typed.chars().count())
                        .min(shown.chars().count()) as u16;
                    let mut cx = text_x.saturating_add(cursor_col);
                    let max_cx = inner.x.saturating_add(inner.width.saturating_sub(1));
                    if cx > max_cx {
                        cx = max_cx;
                    }
                    let pos = Position::new(cx, text_y);
                    if pos.x < area.x.saturating_add(area.width)
                        && pos.y < area.y.saturating_add(area.height)
                    {
                        frame.set_cursor_position(pos);
                    }
                }
            }
        }
    }

    let agent_menu = if agent_open {
        render_dropdown_menu(
            frame,
            agent_value_area,
            &agent_labels,
            state.agent_idx,
            true,
        )
    } else {
        Rect::default()
    };
    let model_menu = if !is_console && model_open {
        render_dropdown_menu(
            frame,
            model_value_area,
            &model_labels,
            state.model_idx,
            true,
        )
    } else {
        Rect::default()
    };

    let agent_target = if agent_open {
        union_rect(chunks[1], agent_menu)
    } else {
        union_rect(chunks[1], chunks[2])
    };
    let model_target = if model_open {
        union_rect(chunks[4], model_menu)
    } else {
        Rect::default()
    };
    let workspace_target = if workspace_focused && workspace_popup.width > 0 {
        union_rect(chunks[7], workspace_popup)
    } else {
        union_rect(chunks[7], chunks[8])
    };

    ClickTargets {
        form: layout.form,
        workspace: workspace_target,
        workspace_field: chunks[8],
        workspace_popup,
        agent: agent_target,
        agent_field: if agent_open { agent_menu } else { chunks[2] },
        agent_popup: if agent_open {
            agent_menu
        } else {
            Rect::default()
        },
        model: model_target,
        model_field: if model_open {
            model_menu
        } else {
            Rect::default()
        },
        model_popup: if model_open {
            model_menu
        } else {
            Rect::default()
        },
        prompt: prompt_target,
        prompt_field: if !is_console {
            chunks[11]
        } else {
            Rect::default()
        },
        foreground_button,
        background_button,
        close: close_target,
    }
}

fn render_field_label(
    frame: &mut ratatui::Frame,
    area: Rect,
    label: &str,
    focused: bool,
    confirmed: bool,
) {
    let style = if confirmed && !focused {
        Style::default().fg(PATH_FG).bg(BG_BASE)
    } else if focused {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(BG_BASE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(BG_BASE)
    };
    frame.render_widget(Paragraph::new(Span::styled(label, style)), area);
}

fn dropdown_block(focused: bool, confirmed: bool, title: &str) -> Block<'_> {
    let border_style = if confirmed && !focused {
        Style::default().fg(PATH_FG)
    } else if focused {
        Style::default().fg(TEXT_SELECTED)
    } else {
        Style::default().fg(PATH_FG)
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(BG_FIELD))
}

fn settled_value_style(bg: ratatui::style::Color) -> Style {
    Style::default().fg(PATH_FG).bg(bg)
}

fn popup_window(count: usize, selected: usize, max_visible_rows: usize) -> (usize, usize) {
    if count == 0 || max_visible_rows == 0 {
        return (0, 0);
    }
    let (start, visible) = dropdown_window(count, selected);
    (start, visible.min(max_visible_rows).min(count))
}

/// Workspace directory picker: use available terminal height instead of the
/// compact agent/model dropdown cap.
pub(crate) fn workspace_list_window(
    count: usize,
    selected: usize,
    max_visible_rows: usize,
) -> (usize, usize) {
    if count == 0 || max_visible_rows == 0 {
        return (0, 0);
    }
    let visible = max_visible_rows.min(count);
    if count <= visible {
        return (0, count);
    }
    let start = selected
        .saturating_sub(visible / 2)
        .min(count.saturating_sub(visible));
    (start, visible)
}

pub(in crate::bar::overlay::new_session) fn dropdown_popup_click_index(
    popup_area: Rect,
    col: u16,
    row: u16,
    count: usize,
    selected: usize,
) -> Option<usize> {
    if count == 0 || popup_area.width == 0 || popup_area.height < 3 {
        return None;
    }
    if !point_in_rect(col, row, popup_area) {
        return None;
    }
    let inner_y = popup_area.y.saturating_add(1);
    let row_idx = row.saturating_sub(inner_y) as usize;
    let visible = popup_area.height.saturating_sub(2) as usize;
    let (start, _) = popup_window(count, selected, visible);
    if row_idx >= visible {
        return None;
    }
    Some(start + row_idx)
}

fn max_dropdown_inner_rows(anchor: Rect, frame_area: Rect) -> usize {
    frame_area
        .y
        .saturating_add(frame_area.height)
        .saturating_sub(anchor.y + 2) as usize
}

fn render_dropdown_menu(
    frame: &mut ratatui::Frame,
    anchor: Rect,
    options: &[String],
    selected_idx: usize,
    open: bool,
) -> Rect {
    if options.is_empty() {
        render_dropdown_collapsed(frame, anchor, "no options", open, false);
        return anchor;
    }

    if !open {
        let value = options.get(selected_idx).map(String::as_str).unwrap_or("");
        render_dropdown_collapsed(frame, anchor, value, false, false);
        return anchor;
    }

    let max_inner = max_dropdown_inner_rows(anchor, frame.area());
    let (start, visible) = popup_window(options.len(), selected_idx, max_inner);
    if visible == 0 {
        let value = options.get(selected_idx).map(String::as_str).unwrap_or("");
        render_dropdown_collapsed(frame, anchor, value, true, false);
        return anchor;
    }

    let menu = Rect {
        x: anchor.x,
        y: anchor.y,
        width: anchor.width,
        height: visible as u16 + 2,
    };
    paint_opaque_rect(frame, menu, BG_PANEL);
    let block = dropdown_block(true, false, "");
    let inner = block.inner(menu);
    frame.render_widget(block, menu);
    fill_rect(frame, inner, BG_PANEL);

    let value_width = inner.width.saturating_sub(2) as usize;
    for row in 0..visible {
        let idx = start + row;
        let y = inner.y.saturating_add(row as u16);
        let selected = idx == selected_idx;
        let row_bg = if selected { BG_SELECTED } else { BG_PANEL };
        fill_rect(
            frame,
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
            row_bg,
        );
        let row_style = if selected {
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(BG_SELECTED)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT_PRIMARY).bg(BG_PANEL)
        };
        let label = options.get(idx).map(String::as_str).unwrap_or("");
        let display = truncate_to_width(label, value_width);
        let lead = if selected { "▎ " } else { "  " };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{lead}{display}"), row_style)),
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
        );
    }
    menu
}

pub(crate) fn popup_row_backdrop(selected: bool, hovered: bool) -> ratatui::style::Color {
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

pub(in crate::bar::overlay::new_session) fn sync_panel_mouse_cursor(
    panel_hover: &PanelHover,
    col: u16,
    row: u16,
    targets: &ClickTargets,
) {
    let shape = if panel_hover.workspace_popup_row.is_some()
        || panel_hover.foreground_button
        || panel_hover.background_button
        || panel_hover.close
    {
        MouseCursorShape::Pointer
    } else if point_in_rect(col, row, targets.prompt_field) {
        MouseCursorShape::Text
    } else {
        MouseCursorShape::Default
    };
    let _ = mouse_cursor::set_mouse_cursor(shape);
}

pub(in crate::bar::overlay::new_session) fn workspace_popup_row_from_mouse(
    popup_area: Rect,
    col: u16,
    row: u16,
    entries: &[WorkspacePopupEntry],
    highlight_idx: usize,
) -> Option<usize> {
    workspace_popup_click_index(popup_area, col, row, entries, highlight_idx)
}

pub(in crate::bar::overlay::new_session) fn workspace_popup_header_click(
    popup_area: Rect,
    col: u16,
    row: u16,
) -> bool {
    if popup_area.width == 0 || popup_area.height < 3 {
        return false;
    }
    if !point_in_rect(col, row, popup_area) {
        return false;
    }
    let inner_y = popup_area.y.saturating_add(1);
    let row_idx = row.saturating_sub(inner_y) as usize;
    row_idx < WORKSPACE_HEADER_ROWS as usize
}

pub(in crate::bar::overlay::new_session) fn workspace_popup_click_index(
    popup_area: Rect,
    col: u16,
    row: u16,
    entries: &[WorkspacePopupEntry],
    highlight_idx: usize,
) -> Option<usize> {
    if entries.is_empty() || popup_area.width == 0 || popup_area.height < 3 {
        return None;
    }
    if !point_in_rect(col, row, popup_area) {
        return None;
    }
    let inner_y = popup_area.y.saturating_add(1);
    let row_idx = row.saturating_sub(inner_y) as usize;
    if row_idx < WORKSPACE_HEADER_ROWS as usize {
        return None;
    }
    let list_row_idx = row_idx - WORKSPACE_HEADER_ROWS as usize;
    let visible = popup_area
        .height
        .saturating_sub(2)
        .saturating_sub(WORKSPACE_HEADER_ROWS) as usize;
    let (start, _) = workspace_list_window(entries.len(), highlight_idx, visible);
    if list_row_idx >= visible {
        return None;
    }
    let entry_idx = start + list_row_idx;
    entries
        .get(entry_idx)
        .filter(|entry| entry.kind != WorkspacePopupKind::Section)
        .map(|_| entry_idx)
}

fn workspace_popup_row_label(entry: &WorkspacePopupEntry) -> String {
    match entry.kind {
        WorkspacePopupKind::Section => entry.label.clone(),
        WorkspacePopupKind::Existing(_) | WorkspacePopupKind::Path => {
            let mut label = entry.label.clone();
            // Sanitize any stray single-letter prefixes that may have leaked from
            // agent/group labels in cwd (e.g. "G downloads", "O acme", "O cursor").
            // This was observed in the session path dropdown and agent selector.
            if label.len() > 2 {
                let bytes = label.as_bytes();
                if bytes[1] == b' '
                    && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'O' || bytes[0] == b'G')
                {
                    // strip leading "X " where X is letter (agent shorthand or artifact)
                    label = label[2..].to_string();
                }
            }
            if matches!(entry.kind, WorkspacePopupKind::Path) && entry.cwd.is_none() {
                format!("> {}", label)
            } else {
                label
            }
        }
    }
}

fn render_session_path_header(
    frame: &mut ratatui::Frame,
    area: Rect,
    value: &str,
    focused: bool,
    confirmed: bool,
    error: bool,
    typing: bool,
) {
    let block = dropdown_block(focused, confirmed, "");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let value_width = inner.width.saturating_sub(2) as usize;
    let display = truncate_to_width(value, value_width);
    fill_rect(frame, inner, BG_FIELD);
    let value_style = if error {
        Style::default().fg(CLOSE_HOVER_FG).bg(BG_FIELD)
    } else if focused && !typing && !confirmed {
        Style::default().fg(PATH_FG).bg(BG_FIELD)
    } else if focused {
        Style::default().fg(TEXT_SELECTED).bg(BG_FIELD)
    } else if confirmed {
        settled_value_style(BG_FIELD)
    } else if value == "pick a session or directory" {
        Style::default().fg(PATH_FG).bg(BG_FIELD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(BG_FIELD)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(display, value_style)),
        Rect {
            x: inner.x.saturating_add(1),
            y: inner.y,
            width: inner.width.saturating_sub(1),
            height: inner.height,
        },
    );
}

fn render_workspace_menu(
    frame: &mut ratatui::Frame,
    anchor: Rect,
    header_value: &str,
    entries: &[WorkspacePopupEntry],
    highlight_idx: usize,
    hover_idx: Option<usize>,
    typing: bool,
    confirmed: bool,
    header_error: bool,
    ghost_hint: Option<&PathGhostHint>,
) -> Rect {
    let frame_bottom = frame.area().y.saturating_add(frame.area().height);
    let max_list_rows = frame_bottom.saturating_sub(anchor.y + 2 + WORKSPACE_HEADER_ROWS) as usize;
    let (start, visible) = if entries.is_empty() {
        (0, 0)
    } else {
        workspace_list_window(entries.len(), highlight_idx, max_list_rows)
    };

    let menu = Rect {
        x: anchor.x,
        y: anchor.y,
        width: anchor.width,
        height: WORKSPACE_HEADER_ROWS
            .saturating_add(visible as u16)
            .saturating_add(2)
            .max(anchor.height),
    };
    paint_opaque_rect(frame, menu, BG_PANEL);
    let block = dropdown_block(true, confirmed, "");
    let inner = block.inner(menu);
    frame.render_widget(block, menu);
    fill_rect(frame, inner, BG_PANEL);

    let header_rect = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: WORKSPACE_HEADER_ROWS,
    };
    fill_rect(frame, header_rect, BG_PANEL);

    let header_width = inner.width.saturating_sub(2) as usize;
    let ghost_style = Style::default().fg(PATH_FG).bg(BG_PANEL);
    let header_line = if typing && header_value.is_empty() {
        Line::from(Span::styled(
            "~/  (or ~ for root)",
            Style::default().fg(PATH_FG).bg(BG_PANEL),
        ))
    } else if let Some(hint) = ghost_hint.filter(|_| typing && !header_error) {
        let typed_display = truncate_to_width(header_value, header_width);
        let typed_len = typed_display.chars().count();
        let remaining = header_width.saturating_sub(typed_len);
        let mut spans = vec![Span::styled(
            typed_display,
            if header_error {
                Style::default()
                    .fg(CLOSE_HOVER_FG)
                    .bg(BG_PANEL)
                    .add_modifier(Modifier::BOLD)
            } else if confirmed {
                settled_value_style(BG_PANEL)
            } else {
                Style::default()
                    .fg(TEXT_SELECTED)
                    .bg(BG_PANEL)
                    .add_modifier(Modifier::BOLD)
            },
        )];
        if remaining > 0 {
            let ghost_text = match hint {
                PathGhostHint::Suffix(suffix) => suffix.clone(),
                PathGhostHint::FullPath(path) => {
                    if typed_len == 0 {
                        path.clone()
                    } else {
                        format!(" {path}")
                    }
                }
            };
            spans.push(Span::styled(
                truncate_to_width(&ghost_text, remaining),
                ghost_style,
            ));
        }
        Line::from(spans)
    } else {
        let header_display = truncate_to_width(header_value, header_width);
        let header_style = if header_error {
            Style::default()
                .fg(CLOSE_HOVER_FG)
                .bg(BG_PANEL)
                .add_modifier(Modifier::BOLD)
        } else if confirmed {
            settled_value_style(BG_PANEL)
        } else if !typing {
            Style::default().fg(PATH_FG).bg(BG_PANEL)
        } else {
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(BG_PANEL)
                .add_modifier(Modifier::BOLD)
        };
        Line::from(Span::styled(header_display, header_style))
    };
    frame.render_widget(
        Paragraph::new(header_line),
        Rect {
            x: inner.x.saturating_add(1),
            y: inner.y,
            width: inner.width.saturating_sub(2),
            height: WORKSPACE_HEADER_ROWS,
        },
    );

    let value_width = inner.width.saturating_sub(2) as usize;
    for row in 0..visible {
        let idx = start + row;
        let y = inner
            .y
            .saturating_add(WORKSPACE_HEADER_ROWS)
            .saturating_add(row as u16);
        let Some(entry) = entries.get(idx) else {
            break;
        };
        let is_section = entry.kind == WorkspacePopupKind::Section;
        let selected = idx == highlight_idx && !is_section;
        let hovered = hover_idx == Some(idx) && !is_section;
        let row_bg = if is_section {
            BG_PANEL
        } else {
            popup_row_backdrop(selected, hovered)
        };
        fill_rect(
            frame,
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
            row_bg,
        );
        let row_style = if is_section {
            Style::default()
                .fg(PATH_FG)
                .bg(BG_PANEL)
                .add_modifier(Modifier::BOLD)
        } else if matches!(entry.kind, WorkspacePopupKind::Path) && entry.cwd.is_none() {
            // Live UI feedback: the currently highlighted/typed path does not exist.
            // Show it in red so the user knows before pressing Enter.
            let mut s = Style::default().fg(CLOSE_HOVER_FG).bg(row_bg);
            if selected {
                s = s.add_modifier(Modifier::BOLD);
            }
            s
        } else if selected {
            Style::default()
                .fg(TEXT_SELECTED)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else if hovered {
            Style::default().fg(TEXT_SELECTED).bg(row_bg)
        } else {
            Style::default().fg(TEXT_PRIMARY).bg(row_bg)
        };
        let display = truncate_to_width(&workspace_popup_row_label(entry), value_width);
        let lead = if selected { "▎ " } else { "  " };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{lead}{display}"), row_style)),
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
        );
    }
    menu
}

fn render_dropdown_collapsed(
    frame: &mut ratatui::Frame,
    area: Rect,
    value: &str,
    focused: bool,
    confirmed: bool,
) {
    let block = dropdown_block(focused, confirmed, "");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let value_width = inner.width.saturating_sub(2) as usize;
    let display = truncate_to_width(value, value_width);
    fill_rect(frame, inner, BG_FIELD);
    let value_style = if focused {
        Style::default().fg(TEXT_SELECTED).bg(BG_FIELD)
    } else if confirmed {
        settled_value_style(BG_FIELD)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(BG_FIELD)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(display, value_style)),
        Rect {
            x: inner.x.saturating_add(1),
            y: inner.y,
            width: inner.width.saturating_sub(1),
            height: inner.height,
        },
    );
    if focused && inner.width > 2 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "▾",
                Style::default().fg(TEXT_SELECTED).bg(BG_FIELD),
            )),
            Rect {
                x: inner.x + inner.width.saturating_sub(2),
                y: inner.y,
                width: 2,
                height: 1,
            },
        );
    }
}

fn prompt_char_selected(abs: usize, selection: Option<(usize, usize)>) -> bool {
    selection.is_some_and(|(start, end)| start < end && start <= abs && abs < end)
}

fn prompt_line_spans(
    line: &str,
    line_start: usize,
    body_fg: ratatui::style::Color,
    body_bg: ratatui::style::Color,
    select_fg: ratatui::style::Color,
    select_bg: ratatui::style::Color,
    selection: Option<(usize, usize)>,
) -> Vec<Span<'static>> {
    let normal = Style::default().fg(body_fg).bg(body_bg);
    let selected = Style::default().fg(select_fg).bg(select_bg);
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_style = normal;

    let flush = |run: &mut String, style: Style, spans: &mut Vec<Span<'static>>| {
        if !run.is_empty() {
            spans.push(Span::styled(std::mem::take(run), style));
        }
    };

    for (col, ch) in line.chars().enumerate() {
        let abs = line_start + col;
        let style = if prompt_char_selected(abs, selection) {
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

fn prompt_terminal_cursor_position(
    inner: Rect,
    text: &str,
    cursor: usize,
    scroll: usize,
    content_width: usize,
) -> Option<Position> {
    let wrapped = notepad::wrapped_display_lines(text, content_width);
    let cursor_line = notepad::display_line_index(text, cursor, content_width);
    let viewport_rows = PROMPT_INNER_HEIGHT as usize;
    if cursor_line < scroll || cursor_line >= scroll.saturating_add(viewport_rows) {
        return None;
    }
    let display_line = wrapped.get(cursor_line)?;
    let col_in_line = cursor.saturating_sub(display_line.start);
    let line_in_viewport = cursor_line - scroll;
    Some(Position::new(
        inner.x.saturating_add(1).saturating_add(col_in_line as u16),
        inner.y.saturating_add(line_in_viewport as u16),
    ))
}

pub(in crate::bar::overlay::new_session) fn point_in_prompt(
    col: u16,
    row: u16,
    targets: &ClickTargets,
) -> bool {
    point_in_rect(col, row, targets.prompt) || point_in_rect(col, row, targets.prompt_field)
}

pub(crate) fn prompt_field_inner(area: Rect) -> Rect {
    dropdown_block(true, false, "").inner(area)
}

fn prompt_cursor_from_mouse(
    area: Rect,
    col: u16,
    row: u16,
    text: &str,
    scroll: usize,
    content_width: usize,
) -> Option<usize> {
    let inner = prompt_field_inner(area);
    if inner.width == 0 || inner.height == 0 || !point_in_rect(col, row, inner) {
        return None;
    }
    let rel_row = row.saturating_sub(inner.y) as usize;
    let rel_col = col.saturating_sub(inner.x.saturating_add(1)) as usize;
    let display_line_idx = scroll.saturating_add(rel_row);
    let wrapped = notepad::wrapped_display_lines(text, content_width);
    let line = wrapped.get(display_line_idx)?;
    let col_in_line = rel_col.min(line.text.chars().count());
    Some(notepad::clamp_cursor(
        text,
        line.start.saturating_add(col_in_line),
    ))
}

fn prompt_selection_cursor_from_mouse(
    area: Rect,
    col: u16,
    row: u16,
    text: &str,
    scroll: usize,
    content_width: usize,
) -> Option<usize> {
    if let Some(cursor) = prompt_cursor_from_mouse(area, col, row, text, scroll, content_width) {
        return Some(cursor);
    }
    let inner = prompt_field_inner(area);
    if inner.width == 0 || inner.height == 0 || !point_in_rect(col, row, area) {
        return None;
    }
    let wrapped = notepad::wrapped_display_lines(text, content_width);
    if row < inner.y {
        let first = wrapped.first()?;
        return Some(first.start);
    }
    if row >= inner.y.saturating_add(inner.height) {
        let last = wrapped.last()?;
        return Some(last.start.saturating_add(last.text.chars().count()));
    }
    let rel_row = if row < inner.y.saturating_add(inner.height / 2) {
        0usize
    } else {
        PROMPT_INNER_HEIGHT.saturating_sub(1) as usize
    };
    let display_line_idx = scroll.saturating_add(rel_row);
    let line = wrapped.get(display_line_idx)?;
    let rel_col = col.saturating_sub(inner.x.saturating_add(1)) as usize;
    let col_in_line = if col < inner.x.saturating_add(1) {
        0
    } else {
        rel_col.min(line.text.chars().count())
    };
    Some(notepad::clamp_cursor(
        text,
        line.start.saturating_add(col_in_line),
    ))
}

fn render_prompt_field(
    frame: &mut ratatui::Frame,
    area: Rect,
    prompt: &str,
    focused: bool,
    _cursor: usize,
    scroll: usize,
    selection: Option<(usize, usize)>,
) {
    let block = dropdown_block(focused, false, "");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    fill_rect(frame, inner, BG_FIELD);

    let width = inner.width.saturating_sub(2) as usize;
    let wrapped = notepad::wrapped_display_lines(prompt, width);
    let body_fg = if focused {
        TEXT_SELECTED
    } else if prompt.is_empty() {
        PATH_FG
    } else {
        TEXT_PRIMARY
    };
    let select_fg = BG_FIELD;
    let select_bg = TEXT_SELECTED;

    let show_placeholder = focused && prompt.is_empty();
    if show_placeholder && inner.height > 0 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "Describe the task…",
                Style::default().fg(PATH_FG).bg(BG_FIELD),
            )),
            Rect {
                x: inner.x.saturating_add(1),
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );
        return;
    }

    for (idx, line) in wrapped
        .iter()
        .skip(scroll)
        .take(PROMPT_INNER_HEIGHT as usize)
        .enumerate()
    {
        if idx as u16 >= inner.height {
            break;
        }
        let spans = prompt_line_spans(
            &line.text, line.start, body_fg, BG_FIELD, select_fg, select_bg, selection,
        );
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                x: inner.x.saturating_add(1),
                y: inner.y.saturating_add(idx as u16),
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );
    }
}

pub(crate) fn submit_button_backdrop(selected: bool) -> ratatui::style::Color {
    if selected {
        BG_SELECTED
    } else {
        BG_FIELD
    }
}

fn render_submit_button(
    frame: &mut ratatui::Frame,
    area: Rect,
    label: &str,
    focused: bool,
    hover: bool,
) {
    let bg = submit_button_backdrop(focused);
    let border_style = if focused || hover {
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

    let style = if focused || hover {
        Style::default()
            .fg(TEXT_SELECTED)
            .bg(bg)
            .add_modifier(if focused {
                Modifier::BOLD
            } else {
                Modifier::empty()
            })
    } else {
        Style::default().fg(PATH_FG).bg(bg)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(label, style)).alignment(Alignment::Center),
        inner,
    );
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars().take(width.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    out
}

pub(crate) fn prompt_display_lines(prompt: &str, width: usize, max_lines: usize) -> Vec<String> {
    notepad::wrapped_display_lines(prompt, width)
        .into_iter()
        .take(max_lines)
        .map(|line| line.text)
        .collect()
}
