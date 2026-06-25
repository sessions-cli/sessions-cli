use crate::bar::notepad::Note;
use chrono::{DateTime, Utc};
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::block::Padding;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;
use std::collections::HashSet;

use super::chrome::{render_bottom_chrome, render_toolbar, render_update_box, UpdateBannerView};
use super::layout::UPDATE_BOX_ROWS;
use super::layout::{
    layout_plan_with_notepad, layout_regions, sidebar_trail_base_row, sidebar_trail_row_at,
    total_list_rows, NotepadListState, NotepadTrailRow,
    notepad_list_state, SESSION_BLOCK_PAD_LEFT, SESSION_BLOCK_PAD_RIGHT,
};
use super::notepad::{
    notepad_body_line_spans, notepad_content_width, notepad_note_body_visible_rect,
    notepad_note_title_backdrop_bg_in_list, notepad_note_title_line, notepad_note_title_prefix,
    notepad_note_title_prefix_drag, notepad_note_title_row_style_drag,
    notepad_note_title_row_style_in_list, notepad_scrollbar_geometry, notepad_section_header_line,
    notepad_terminal_cursor_position, note_close_target_row, note_section_highlight, note_sections,
    render_notepad_body_padding_backdrop, NoteDragState, NoteSection,
};
use super::sessions::{
    apply_group_highlight, close_target_row, compact_session_label,
    group_header_label_drag,
    group_drag_row_backdrop, group_section_highlight, group_sections,
    group_toggle_row_is_selected,
    scroll_session_count, session_display_label, session_row_backdrop_bg, session_row_base_style,
    session_row_is_selected,
    session_trailing_badge, sessions_block_title, render_sessions_title_hover_overlay,
    GroupDragState, GroupHighlight, GroupSection, RowKind,
};
use super::snapshot::{
    ChromeView, NotepadView, OverlayView, SessionsView, SidebarSnapshot,
};
use super::theme::*;
use super::widgets::{
    chrome_row_prefix, completion_badge_style, context_menu_action_at, context_menu_item_enabled,
    context_menu_items, context_menu_label, context_menu_rect, empty_trailing_slot,
    format_completion_square_slot, format_spinner_slot, format_trailing_slot, full_width_line,
    full_width_spans, group_add_badge_style, render_full_width_row_backdrop,
    delete_note_confirm_rect, render_delete_note_confirm, render_notepad_context_menu,
    render_notepad_scrollbar,
    rename_targets_note, rename_targets_session,
    rename_terminal_cursor_position, row_label_width, row_prefix, row_with_trailing_slot,
    run_spinner_glyph, spinner_badge_style, truncate, ContextMenuTarget, DeleteNoteConfirmState,
    RenameState, GROUP_ADD_ICON, CONTEXT_MENU_ITEM_HEIGHT,
};

fn notepad_trail_list_item(
    row_idx: usize,
    trail_row: NotepadTrailRow,
    line_width: usize,
    notes: &[Note],
    section_expanded: bool,
    active_note_index: Option<usize>,
    focused: bool,
    active_note_text: &str,
    body_scroll: usize,
    section_header_hover: bool,
    section_add_hover: bool,
    last_saved_at: Option<DateTime<Utc>>,
    note_hover: Option<usize>,
    selection: Option<(usize, usize)>,
    rename: Option<&RenameState>,
    close_modifier_held: bool,
    close_target: Option<usize>,
    trail_base: usize,
    note_state: &NotepadListState<'_>,
    note_drag: &NoteDragState,
    note_drag_sections: &[NoteSection],
    anim_frame: usize,
) -> ListItem<'static> {
    let trail_idx = row_idx.saturating_sub(trail_base);
    let note_highlight = note_section_highlight(note_drag_sections, trail_idx, note_drag);
    match trail_row {
        NotepadTrailRow::SectionPad => {
            ListItem::new(Line::from(Span::styled(" ", Style::default().bg(BG_BASE))))
        }
        NotepadTrailRow::SectionHeader => ListItem::new(notepad_section_header_line(
            line_width,
            section_expanded,
            section_header_hover,
            section_add_hover,
            last_saved_at,
        )),
        NotepadTrailRow::NoteTitle { note_index } => {
            let note = notes.get(note_index);
            let is_active = active_note_index == Some(note_index);
            let is_hovered = note_hover == Some(note_index);
            let editing = note.is_some_and(|note| {
                rename.is_some_and(|rename| rename_targets_note(rename, &note.id))
            });
            let is_close_target = note_close_target_row(
                row_idx,
                trail_base,
                close_modifier_held,
                close_target,
                note_state,
                line_width,
            );
            let drag_row = note_highlight.is_some();
            let row_style = if drag_row {
                notepad_note_title_row_style_drag(
                    editing,
                    is_active,
                    is_hovered,
                    is_close_target,
                    close_modifier_held,
                    note_highlight,
                )
            } else {
                notepad_note_title_row_style_in_list(
                    editing,
                    is_active,
                    is_hovered,
                    is_close_target,
                    close_modifier_held,
                )
            };
            let prefix = if drag_row {
                notepad_note_title_prefix_drag(editing, is_close_target, note_highlight)
            } else {
                notepad_note_title_prefix(editing, is_close_target)
            };
            let label_width = line_width.saturating_sub(prefix.chars().count());
            if let (Some(note), Some(rename)) = (
                note,
                note.and_then(|n| rename.filter(|r| rename_targets_note(r, &n.id))),
            ) {
                let _ = note;
                let title_text = truncate(&rename.buffer, label_width);
                let title_len = title_text.chars().count();
                let selected = rename.select_all && !rename.buffer.is_empty();
                let text_style = if selected {
                    Style::default()
                        .fg(NOTEPAD_SELECT_FG)
                        .bg(NOTEPAD_SELECT_BG)
                        .add_modifier(Modifier::BOLD)
                } else {
                    row_style
                };
                let pad_len = label_width.saturating_sub(title_len);
                let spans = vec![
                    Span::styled(prefix, row_style),
                    Span::styled(title_text, text_style),
                    Span::styled(" ".repeat(pad_len), row_style),
                ];
                return ListItem::new(full_width_spans(spans, line_width, row_style));
            }
            let title = note.map(|note| note.title.as_str()).unwrap_or("");
            ListItem::new(notepad_note_title_line(prefix, title, line_width, row_style))
        }
        NotepadTrailRow::NotesToggle {
            expanded,
            hidden_count,
        } => {
            let label = if expanded {
                "show less".to_string()
            } else {
                format!("show more (+{hidden_count})")
            };
            let style = Style::default().fg(GROUP_TOGGLE_FG).bg(BG_BASE);
            ListItem::new(notepad_note_title_line(
                notepad_note_title_prefix(false, false),
                &label,
                line_width,
                style,
            ))
        }
        NotepadTrailRow::NoteBodyPad { .. } => {
            let body_bg = NOTEPAD_EDIT_BG;
            ListItem::new(full_width_line(
                " ".to_string(),
                line_width,
                Style::default().bg(body_bg),
            ))
            .style(Style::default().bg(body_bg))
        }
        NotepadTrailRow::NoteBodySlot { note_index, slot } => {
            let body_bg = NOTEPAD_EDIT_BG;
            let is_active_body = active_note_index == Some(note_index);
            let text = if is_active_body {
                active_note_text
            } else {
                notes
                    .get(note_index)
                    .map(|note| note.text.as_str())
                    .unwrap_or("")
            };
            let scroll = if is_active_body { body_scroll } else { 0 };
            let body_focused = is_active_body && focused;
            let body_selection = if is_active_body {
                selection
            } else {
                None
            };
            let content_width = notepad_content_width(line_width, true);
            let wrapped = crate::bar::notepad::wrapped_display_lines(text, content_width);
            let line_idx = scroll.saturating_add(slot);
            let display_line = wrapped.get(line_idx);
            let line = display_line.map(|line| line.text.as_str()).unwrap_or("");
            let body_fg = NOTEPAD_EDIT_FG;
            let select_fg = if body_focused {
                NOTEPAD_SELECT_FG
            } else {
                BG_BASE
            };
            let select_bg = NOTEPAD_SELECT_BG;
            let line_start = display_line.map(|line| line.start).unwrap_or(0);
            let spans = notepad_body_line_spans(
                line,
                line_start,
                content_width,
                body_fg,
                body_bg,
                select_fg,
                select_bg,
                body_selection,
            );
            ListItem::new(full_width_spans(
                spans,
                line_width,
                Style::default().bg(body_bg),
            ))
            .style(Style::default().bg(body_bg).fg(body_fg))
        }
    }
}
pub(crate) fn list_row_backdrop_bg(
    row_idx: usize,
    trail_base: usize,
    rows: &[RowKind],
    selected: usize,
    close_modifier_held: bool,
    hover_row: Option<usize>,
    close_target: Option<usize>,
    group_hover_row: Option<usize>,
    section_header_hover: bool,
    note_state: &NotepadListState<'_>,
    line_width: usize,
    note_hover: Option<usize>,
    group_drag: &GroupDragState,
    sections: &[GroupSection],
    note_drag: &NoteDragState,
    note_drag_sections: &[NoteSection],
    rename: Option<&RenameState>,
) -> Option<Color> {
    if row_idx < trail_base {
        let row = rows.get(row_idx)?;
        return match row {
            RowKind::Session { session } => {
                if rename.is_some_and(|rename| rename_targets_session(rename, &session.id)) {
                    Some(RENAME_EDIT_BG)
                } else {
                    session_row_backdrop_bg(
                        session,
                        row_idx,
                        selected,
                        close_modifier_held,
                        hover_row,
                        close_target,
                        rows,
                        sections,
                        group_drag,
                    )
                }
            }
            RowKind::Group { .. } => group_drag_row_backdrop(&sections, rows, row_idx, group_drag)
                .or_else(|| {
                    (!group_drag.active() && group_hover_row == Some(row_idx)).then_some(BG_HIGHLIGHT)
                }),
            RowKind::GroupToggle { cwd_label, .. } => {
                group_drag_row_backdrop(&sections, rows, row_idx, group_drag).or_else(|| {
                    if group_drag.active() {
                        return None;
                    }
                    let is_selected =
                        group_toggle_row_is_selected(cwd_label, row_idx, selected, group_drag);
                    let is_hovered = hover_row == Some(row_idx);
                    if is_selected && is_hovered {
                        Some(BG_HOVER_SELECTED)
                    } else if is_selected {
                        Some(BG_SELECTED)
                    } else if is_hovered {
                        Some(BG_HIGHLIGHT)
                    } else {
                        None
                    }
                })
            }
            RowKind::Empty(_) => None,
        };
    }
    let trail_idx = row_idx.saturating_sub(trail_base);
    let trail_row = sidebar_trail_row_at(trail_idx, note_state)?;
    let note_highlight = note_section_highlight(note_drag_sections, trail_idx, note_drag);
    match trail_row {
        NotepadTrailRow::SectionHeader if section_header_hover && !note_drag.active() => {
            Some(BG_HIGHLIGHT)
        }
        NotepadTrailRow::NoteTitle { note_index } => {
            let is_close_target = note_close_target_row(
                row_idx,
                trail_base,
                close_modifier_held,
                close_target,
                note_state,
                line_width,
            );
            if note_highlight == Some(GroupHighlight::Source) {
                return notepad_note_title_backdrop_bg_in_list(
                    note_index,
                    note_state,
                    Some(note_index),
                    rename,
                    is_close_target,
                    close_modifier_held,
                );
            }
            notepad_note_title_backdrop_bg_in_list(
                note_index,
                note_state,
                note_hover,
                rename,
                is_close_target,
                close_modifier_held,
            )
        }
        NotepadTrailRow::NoteBodyPad { .. } | NotepadTrailRow::NoteBodySlot { .. } => {
            Some(NOTEPAD_EDIT_BG)
        }
        _ => None,
    }
}
pub fn draw(frame: &mut Frame, snap: &SidebarSnapshot<'_>) {
    let snap = *snap;
    let SidebarSnapshot {
        sessions,
        notepad,
        chrome,
        overlay,
    } = snap;
    let SessionsView {
        rows,
        selected,
        scroll,
        digit_buffer,
        close_modifier_held,
        hover_row,
        close_target,
        group_hover_row,
        sessions_expanded,
        folded_groups,
        group_order,
        group_drag,
        sessions_title_hover,
        sessions_title_add_hover,
        anim_frame,
    } = sessions;
    let NotepadView {
        notes,
        expanded: notepad_expanded,
        notes_list_expanded,
        active_note_index,
        text: notepad_text,
        cursor: notepad_cursor,
        scroll: notepad_scroll,
        focused: notepad_focused,
        section_header_hover,
        section_add_hover,
        note_hover,
        note_drag,
        selection: notepad_selection,
        last_saved_at: notepad_last_saved_at,
    } = notepad;
    let ChromeView {
        toolbar_hover,
        coming_soon_frames,
        settings_hover,
        leave_hover,
        workspace_settings_open,
        workspace_new_session_open,
    } = chrome;
    let OverlayView {
        context_menu,
        rename,
        delete_note_confirm,
        clipboard_notice,
        update_banner,
        update_upgrade_hover,
        update_dismiss_hover,
    } = overlay;
    let area = frame.area();
    let show_update_banner = update_banner.is_some();
    let plan = layout_plan_with_notepad(
        Size::new(area.width, area.height),
        rows,
        sessions_expanded,
        notes,
        notepad_expanded,
        show_update_banner,
    );
    frame.render_widget(Block::default().style(Style::default().bg(BG_BASE)), area);
    let (toolbar_area, body_area, settings_area) = layout_regions(area, &plan);
    let line_width = plan.metrics.list_line_width;
    let sessions_title = sessions_block_title(
        close_modifier_held,
        digit_buffer,
        rename,
        delete_note_confirm,
        sessions_expanded,
        sessions_title_hover,
        sessions_title_add_hover,
        line_width,
        clipboard_notice,
    );
    let terminal_block = Block::default()
        .title(sessions_title.clone())
        .borders(Borders::NONE)
        .padding(Padding::new(
            SESSION_BLOCK_PAD_LEFT,
            SESSION_BLOCK_PAD_RIGHT,
            0,
            0,
        ))
        .style(Style::default().bg(BG_BASE));
    let terminal_area = terminal_block.inner(body_area);
    render_toolbar(
        frame,
        area,
        toolbar_area,
        terminal_area,
        toolbar_hover,
        workspace_new_session_open,
        coming_soon_frames,
    );
    let body_height = terminal_area.height as usize;
    let trail_base = sidebar_trail_base_row(rows.len(), sessions_expanded);
    let notepad_state = notepad_list_state(
        notes,
        notepad_expanded,
        notes_list_expanded,
        active_note_index,
    );
    let note_drag_sections = note_sections(&notepad_state);
    let total_rows = total_list_rows(
        rows.len(),
        sessions_expanded,
        &notepad_state,
    );
    let mut session_ordinal = scroll_session_count(rows, scroll);
    let sections = group_sections(rows);
    let visible: Vec<ListItem> = (scroll..scroll.saturating_add(body_height))
        .take_while(|&row_idx| row_idx < total_rows)
        .map(|row_idx| {
            if row_idx >= trail_base {
                let trail_idx = row_idx.saturating_sub(trail_base);
                let trail_row =
                    sidebar_trail_row_at(trail_idx, &notepad_state)
                        .unwrap_or(NotepadTrailRow::SectionPad);
                return notepad_trail_list_item(
                    row_idx,
                    trail_row,
                    line_width,
                    notes,
                    notepad_expanded,
                    active_note_index,
                    notepad_focused,
                    notepad_text,
                    notepad_scroll,
                    section_header_hover,
                    section_add_hover,
                    notepad_last_saved_at,
                    note_hover,
                    notepad_selection,
                    rename,
                    close_modifier_held,
                    close_target,
                    trail_base,
                    &notepad_state,
                    note_drag,
                    &note_drag_sections,
                    anim_frame,
                );
            }
            let row = &rows[row_idx];
            match row {
                RowKind::Empty(label) => {
                    let row_style = Style::default().fg(PATH_FG).bg(BG_BASE);
                    let (trailing, trailing_style) = empty_trailing_slot(row_style);
                    ListItem::new(row_with_trailing_slot(
                        chrome_row_prefix(),
                        label,
                        trailing,
                        line_width,
                        row_style,
                        trailing_style,
                    ))
                }
                RowKind::Group { label, collapsed } => {
                    let group_highlight =
                        group_section_highlight(&sections, rows, row_idx, group_drag);
                    let hovered = !group_drag.active() && group_hover_row == Some(row_idx);
                    let base_style = if hovered {
                        Style::default().fg(TEXT_SECONDARY).bg(BG_HIGHLIGHT)
                    } else {
                        Style::default().fg(TEXT_SECONDARY).bg(BG_BASE)
                    };
                    let row_style = apply_group_highlight(base_style, group_highlight);
                    let (trailing, trailing_style) = if hovered {
                        (
                            format_trailing_slot(GROUP_ADD_ICON),
                            group_add_badge_style(row_style),
                        )
                    } else {
                        empty_trailing_slot(row_style)
                    };
                    ListItem::new(row_with_trailing_slot(
                        chrome_row_prefix(),
                        &group_header_label_drag(label, *collapsed, group_highlight),
                        trailing,
                        line_width,
                        row_style,
                        trailing_style,
                    ))
                }
                RowKind::GroupToggle {
                    cwd_label,
                    expanded,
                    hidden_count,
                    ..
                } => {
                    let is_selected =
                        group_toggle_row_is_selected(cwd_label, row_idx, selected, group_drag);
                    let is_hovered = !group_drag.active() && hover_row == Some(row_idx);
                    let group_highlight =
                        group_section_highlight(&sections, rows, row_idx, group_drag);
                    let lead = if is_selected { "▎" } else { " " };
                    let label = if *expanded {
                        "show less".to_string()
                    } else {
                        format!("show more (+{hidden_count})")
                    };
                    let base_style = if group_drag.active() {
                        Style::default().fg(GROUP_TOGGLE_FG).bg(BG_BASE)
                    } else if is_selected {
                        Style::default().fg(TEXT_SELECTED).bg(BG_SELECTED)
                    } else if is_hovered {
                        Style::default().fg(TEXT_SECONDARY).bg(BG_HIGHLIGHT)
                    } else {
                        Style::default().fg(GROUP_TOGGLE_FG).bg(BG_BASE)
                    };
                    let style = apply_group_highlight(base_style, group_highlight);
                    let (trailing, trailing_style) = empty_trailing_slot(style);
                    ListItem::new(row_with_trailing_slot(
                        row_prefix(lead, None),
                        &label,
                        trailing,
                        line_width,
                        style,
                        trailing_style,
                    ))
                }
                RowKind::Session { session } => {
                    session_ordinal += 1;
                    let is_selected =
                        session_row_is_selected(session, row_idx, selected, group_drag);
                    let is_close_target = close_target_row(
                        rows,
                        close_modifier_held,
                        close_target,
                        selected,
                        row_idx,
                    );
                    let group_highlight =
                        group_section_highlight(&sections, rows, row_idx, group_drag);
                    let base_row_style = session_row_base_style(
                        session,
                        row_idx,
                        selected,
                        close_modifier_held,
                        hover_row,
                        close_target,
                        rows,
                        group_drag,
                    );
                    let row_style = apply_group_highlight(base_row_style, group_highlight);
                    let badge = session_trailing_badge(session, row_style, anim_frame);
                    let row_bg = row_style.bg.unwrap_or(BG_BASE);
                    let badge_style = if is_close_target {
                        badge.1.bg(row_bg)
                    } else if close_modifier_held && !session.pins_to_group_top() {
                        row_style.fg(CLOSE_MODE_FG).bg(row_bg)
                    } else {
                        apply_group_highlight(badge.1, group_highlight).bg(row_bg)
                    };
                    let index_text = format!("{session_ordinal}");
                    let label_width = row_label_width(line_width);
                    let editing =
                        rename.is_some_and(|rename| rename_targets_session(rename, &session.id));
                    let lead = if editing {
                        "✎"
                    } else if is_selected {
                        "▎"
                    } else {
                        " "
                    };
                    let row_bg = if editing { RENAME_EDIT_BG } else { row_bg };
                    let title_fg = row_style.fg.unwrap_or(TEXT_PRIMARY);
                    let index_fg = if editing {
                        RENAME_EDIT_FG
                    } else if is_close_target {
                        CLOSE_HOVER_FG
                    } else if close_modifier_held {
                        CLOSE_MODE_FG
                    } else {
                        title_fg
                    };
                    let active_row_style = if editing {
                        Style::default()
                            .fg(RENAME_EDIT_FG)
                            .bg(RENAME_EDIT_BG)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        row_style.bg(row_bg)
                    };
                    let index_style = active_row_style.fg(index_fg);
                    let edit_style = active_row_style;
                    let title_spans =
                        if let Some(rename) =
                            rename.filter(|rename| rename_targets_session(rename, &session.id))
                        {
                            let field_width = label_width;
                            let title_text = truncate(&rename.buffer, field_width);
                            let title_len = title_text.chars().count();
                            let selected = rename.select_all && !rename.buffer.is_empty();
                            let text_style = if selected {
                                Style::default()
                                    .fg(RENAME_SELECT_FG)
                                    .bg(RENAME_SELECT_BG)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                edit_style
                            };
                            let pad_len = field_width.saturating_sub(title_len);
                            vec![
                                Span::styled(title_text, text_style),
                                Span::styled(" ".repeat(pad_len), edit_style),
                            ]
                        } else {
                            let label = if label_width < 12 {
                                compact_session_label(session)
                            } else {
                                session_display_label(session)
                            };
                            let title_text = truncate(&label, label_width);
                            vec![Span::styled(
                                format!("{:<width$}", title_text, width = label_width),
                                row_style,
                            )]
                        };
                    let close_mark_style = Style::default()
                        .fg(CLOSE_HOVER_FG)
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD);
                    let lead_prefix = if is_close_target {
                        " ✕".to_string()
                    } else {
                        format!("{lead} ")
                    };
                    let lead_style = if is_close_target {
                        close_mark_style
                    } else {
                        active_row_style
                    };
                    let mut spans = vec![
                        Span::styled(lead_prefix, lead_style),
                        Span::styled(format!("{index_text:>2}  "), index_style),
                    ];
                    spans.extend(title_spans);
                    spans.push(Span::styled("  ", active_row_style));
                    if editing {
                        spans.push(Span::styled(
                            "···",
                            Style::default().fg(TEXT_SECONDARY).bg(row_bg),
                        ));
                    } else {
                        spans.push(Span::styled(badge.0, badge_style));
                    }
                    let mut item =
                        ListItem::new(full_width_spans(spans, line_width, active_row_style));
                    if row_bg != BG_BASE {
                        item = item.style(Style::default().bg(row_bg));
                    }
                    item
                }
            }
        })
        .collect();

    let list = List::new(visible).style(Style::default().bg(BG_BASE));
    frame.render_widget(terminal_block, body_area);
    if sessions_title_hover {
        render_sessions_title_hover_overlay(
            frame,
            area,
            plan.metrics.sessions_title_y,
            sessions_title,
        );
    }
    for (visible_idx, row_idx) in (scroll..scroll.saturating_add(body_height))
        .take_while(|&row_idx| row_idx < total_rows)
        .enumerate()
    {
        let Some(bg) = list_row_backdrop_bg(
            row_idx,
            trail_base,
            rows,
            selected,
            close_modifier_held,
            hover_row,
            close_target,
            group_hover_row,
            section_header_hover,
            &notepad_state,
            line_width,
            note_hover,
            group_drag,
            &sections,
            note_drag,
            &note_drag_sections,
            rename,
        ) else {
            continue;
        };
        render_full_width_row_backdrop(
            frame,
            area,
            terminal_area.y.saturating_add(visible_idx as u16),
            bg,
        );
    }
    frame.render_widget(list, terminal_area);
    if let Some(banner) = update_banner {
        let box_rect = Rect {
            x: terminal_area.x,
            y: plan.metrics.update_banner_top_y,
            width: terminal_area.width,
            height: UPDATE_BOX_ROWS.min(settings_area.height),
        };
        render_update_box(
            frame,
            area,
            banner,
            box_rect,
            update_upgrade_hover,
            update_dismiss_hover,
        );
    }
    render_bottom_chrome(
        frame,
        area,
        settings_area,
        terminal_area,
        settings_hover,
        workspace_settings_open,
        leave_hover,
    );
    if notepad_expanded {
        if let Some(note_index) = active_note_index {
            if let Some(body) = notepad_note_body_visible_rect(
                terminal_area,
                scroll,
                body_height,
                trail_base,
                &notepad_state,
                note_index,
            ) {
                render_notepad_body_padding_backdrop(frame, body_area, terminal_area, body);
            }
            if let Some(scrollbar) = notepad_scrollbar_geometry(
                terminal_area,
                scroll,
                body_height,
                trail_base,
                &notepad_state,
                note_index,
                notepad_text,
                notepad_scroll,
                line_width,
            ) {
                render_notepad_scrollbar(frame, scrollbar, notepad_focused);
            }
        }
    }
    if notepad_focused && notepad_expanded {
        if let Some(note_index) = active_note_index {
            if let Some(pos) = notepad_terminal_cursor_position(
                terminal_area,
                scroll,
                body_height,
                trail_base,
                &notepad_state,
                note_index,
                notepad_focused,
                notepad_text,
                notepad_cursor,
                notepad_scroll,
                line_width,
            ) {
                frame.set_cursor_position(pos);
            }
        }
    } else if let Some(rename) = rename {
        if let Some(pos) =
            rename_terminal_cursor_position(terminal_area, scroll, body_height, rename)
        {
            frame.set_cursor_position(pos);
        }
    } else if let Some(confirm) = delete_note_confirm {
        let rect = delete_note_confirm_rect(area, &confirm.title);
        let inner = Block::default()
            .borders(Borders::ALL)
            .inner(rect);
        let cursor_x = inner
            .x
            .saturating_add(2)
            .saturating_add(confirm.buffer.chars().count() as u16);
        let cursor_y = inner.y.saturating_add(2);
        if cursor_y < inner.y.saturating_add(inner.height) {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }
    if let Some(confirm) = delete_note_confirm {
        render_delete_note_confirm(frame, confirm, area);
    }
    if let Some(menu) = context_menu {
        if matches!(menu.target, ContextMenuTarget::Notepad { .. }) {
            render_notepad_context_menu(frame, menu, area);
        } else {
            let rect = context_menu_rect(menu, area);
            frame.render_widget(Clear, rect);
            let style = Style::default().fg(TEXT_SELECTED).bg(BG_PANEL);
            for (idx, action) in context_menu_items(&menu.target).iter().enumerate() {
                let item_rect = Rect {
                    x: rect.x,
                    y: rect.y + idx as u16,
                    width: rect.width,
                    height: CONTEXT_MENU_ITEM_HEIGHT,
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        context_menu_label(&menu.target, *action),
                        style,
                    )))
                    .style(Style::default().bg(BG_PANEL)),
                    item_rect,
                );
            }
        }
    }
}

pub fn ensure_selection_visible(selected: usize, scroll: usize, body_height: usize) -> usize {
    if selected < scroll {
        selected
    } else if selected >= scroll + body_height {
        scroll.max(selected + 1 - body_height)
    } else {
        scroll
    }
}
