use crate::bar::notepad::Note;
use crate::bar::ui::*;
use crate::model::{AgentState, Session};
use chrono::Utc;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use std::collections::HashSet;

#[test]
fn list_selected_plain_text_spans_rows() {
    let rows = vec![
        RowKind::Group {
            label: "projects".into(),
            collapsed: false,
        },
        RowKind::Session {
            session: sample_session("auth refactor", 1, 5),
        },
    ];
    let anchor = ListTextPoint {
        row_idx: 0,
        char_idx: 2,
    };
    let head = ListTextPoint {
        row_idx: 1,
        char_idx: 4,
    };
    let text = list_selected_plain_text(&rows, 40, anchor, head);
    assert!(text.contains("projects") || text.contains('▾'));
    assert!(text.lines().count() >= 2);
}

#[test]
fn dim_style_preserves_background_and_unbolds() {
    let base = state_style(AgentState::Working, true, false);
    let dimmed = dim_style(base);
    assert_eq!(dimmed.bg, Some(BG_SELECTED));
    assert!(!dimmed.add_modifier.contains(Modifier::BOLD));
    assert_eq!(dimmed.fg, Some(dim_color(TEXT_SELECTED)));
}

#[test]
fn group_drag_source_highlight_dims_without_lifted_background() {
    let base = state_style(AgentState::Idle, true, false);
    let highlighted = apply_group_highlight(base, Some(GroupHighlight::Source));
    assert_eq!(highlighted.bg, Some(BG_SELECTED));
    assert_eq!(highlighted.fg, Some(dim_color(TEXT_SELECTED)));
}

#[test]
fn group_drag_target_highlight_covers_entire_pwd_section() {
    let sessions = vec![
        sample_session_in("~/a", "one", 1),
        sample_session_in("~/b", "two", 2),
    ];
    let rows = build_rows(
        &sessions,
        &HashSet::new(),
        &HashSet::new(),
        &["~/a".into(), "~/b".into()],
    );
    let sections = group_sections(&rows);
    let drag = GroupDragState {
        source: Some("~/a".into()),
        hover: Some("~/b".into()),
        dragged: true,
        ..Default::default()
    };
    let b = &sections[1];
    assert_eq!(
        group_section_highlight(&sections, &rows, b.start, &drag),
        Some(GroupHighlight::Target)
    );
    assert_eq!(
        group_section_highlight(&sections, &rows, b.start + 1, &drag),
        Some(GroupHighlight::Target)
    );
}

#[test]
fn group_drag_highlight_hidden_until_dragged() {
    // Active-but-not-dragged (engaged without leaving source) must not flash ⠿.
    let sessions = vec![
        sample_session_in("~/a", "one", 1),
        sample_session_in("~/b", "two", 2),
    ];
    let rows = build_rows(
        &sessions,
        &HashSet::new(),
        &HashSet::new(),
        &["~/a".into(), "~/b".into()],
    );
    let sections = group_sections(&rows);
    let active_only = GroupDragState {
        source: Some("~/a".into()),
        hover: Some("~/a".into()),
        dragged: false,
        ..Default::default()
    };
    assert_eq!(
        group_section_highlight(&sections, &rows, sections[0].start, &active_only),
        None
    );
    assert_eq!(
        group_section_highlight(&sections, &rows, sections[1].start, &active_only),
        None
    );
    let dragging = GroupDragState {
        source: Some("~/a".into()),
        hover: Some("~/b".into()),
        dragged: true,
        ..Default::default()
    };
    assert_eq!(
        group_section_highlight(&sections, &rows, sections[0].start, &dragging),
        Some(GroupHighlight::Source)
    );
}

#[test]
fn group_drag_backdrop_is_never_painted() {
    use super::sessions::group_drag_row_backdrop;

    let sessions = vec![
        sample_session_in("~/a", "one", 1),
        sample_session_in("~/b", "two", 2),
    ];
    let rows = build_rows(
        &sessions,
        &HashSet::new(),
        &HashSet::new(),
        &["~/a".into(), "~/b".into()],
    );
    let sections = group_sections(&rows);
    let drag = GroupDragState {
        source: Some("~/a".into()),
        hover: Some("~/b".into()),
        dragged: true,
        preserved_session_id: Some("tmux:win:1".into()),
        ..Default::default()
    };
    let a = &sections[0];
    let b = &sections[1];
    assert_eq!(
        group_drag_row_backdrop(&sections, &rows, a.start, &drag),
        None
    );
    assert_eq!(
        group_drag_row_backdrop(&sections, &rows, a.start + 1, &drag),
        None
    );
    assert_eq!(
        group_drag_row_backdrop(&sections, &rows, b.start, &drag),
        None
    );
    assert_eq!(
        group_drag_row_backdrop(&sections, &rows, b.start + 1, &drag),
        None
    );
    let session = &sessions[0];
    assert_eq!(
        session_row_backdrop_bg(
            session,
            a.start + 1,
            a.start + 1,
            false,
            None,
            None,
            &rows,
            &sections,
            &drag,
        ),
        None
    );
}

#[test]
fn session_row_is_selected_follows_preserved_id_during_drag() {
    let session = sample_session_in("~/a", "one", 1);
    let drag = GroupDragState {
        source: Some("~/a".into()),
        hover: Some("~/b".into()),
        dragged: true,
        preserved_session_id: Some("tmux:win:1".into()),
        ..Default::default()
    };
    assert!(session_row_is_selected(&session, 0, 0, &drag));
    assert!(session_row_is_selected(
        &session,
        3,
        3,
        &GroupDragState::default()
    ));
    let other = sample_session_in("~/b", "two", 2);
    assert!(!session_row_is_selected(&other, 3, 3, &drag));
}

#[test]
fn full_width_line_pads_to_terminal_width() {
    let style = Style::default();
    let line = full_width_line("hi".to_string(), 6, style);
    let text: String = line.spans.iter().map(|span| span.content.clone()).collect();
    assert_eq!(text.chars().count(), 6);
}

fn sample_session(description: &str, tab_index: u32, minutes_ago: i64) -> Session {
    Session {
        id: format!("tmux:win:{tab_index}"),
        kitty_window_id: tab_index as u64,
        kitty_tab_id: tab_index as u64,
        kitty_os_window_id: 1,
        tab_index,
        tmux_session: String::new(),
        tmux_pane_id: String::new(),
        pane_pid: 0,
        agent_session_id: None,
        title: format!("app · {description}"),
        description: description.into(),
        cwd: "/tmp".into(),
        cwd_label: "~/tmp".into(),
        project: "app".into(),
        state: AgentState::Idle,
        completed_thread: None,
        completed_at: None,
        messaged_at: Some(Utc::now() - chrono::Duration::minutes(minutes_ago)),
        prompt_submitted: true,
        title_manual: false,
        is_active: false,
        last_event_at: Utc::now() - chrono::Duration::minutes(minutes_ago),
        ..Default::default()
    }
}

fn completed_session(description: &str, tab_index: u32, minutes_ago: i64) -> Session {
    let mut session = sample_session(description, tab_index, minutes_ago);
    session.state = AgentState::Done;
    session.completed_thread = Some(description.into());
    session.completed_at = Some(Utc::now() - chrono::Duration::minutes(minutes_ago));
    session.messaged_at = Some(Utc::now() - chrono::Duration::minutes(minutes_ago));
    session
}

fn sample_expanded_notes(text: &str) -> Vec<Note> {
    vec![Note::new("Note 1", text, true)]
}

fn sample_notepad_state(notes: &[Note]) -> NotepadListState<'_> {
    notepad_list_state(notes, true, false, Some(0))
}

#[test]
fn notepad_trail_layout_caps_collapsed_note_titles_with_toggle() {
    let notes: Vec<Note> = (1..=5)
        .map(|n| Note::new(format!("Note {n}"), String::new(), false))
        .collect();
    let collapsed = notepad_list_state(&notes, true, false, Some(3));
    let layout = notepad_trail_layout(&collapsed);
    assert!(layout.iter().any(|row| {
        matches!(
            row,
            NotepadTrailRow::NotesToggle {
                expanded: false,
                hidden_count: 2
            }
        )
    }));
    let expanded = notepad_list_state(&notes, true, true, Some(3));
    let layout = notepad_trail_layout(&expanded);
    assert!(!layout
        .iter()
        .any(|row| matches!(row, NotepadTrailRow::NotesToggle { .. })));
    assert_eq!(
        layout
            .iter()
            .filter(|row| matches!(row, NotepadTrailRow::NoteTitle { .. }))
            .count(),
        5
    );
}

fn first_note_body_content_row(visible_sessions: usize, notes: &[Note]) -> usize {
    let state = sample_notepad_state(notes);
    let trail_idx = notepad_trail_layout(&state)
        .iter()
        .position(|row| {
            matches!(
                row,
                NotepadTrailRow::NoteBodySlot {
                    note_index: 0,
                    slot: 0
                }
            )
        })
        .expect("expanded note body");
    visible_sessions.saturating_add(trail_idx)
}

#[test]
fn row_from_mouse_respects_body_and_scroll() {
    assert_eq!(row_from_mouse(1, 1, 10, 0, 5), Some(0));
    assert_eq!(row_from_mouse(3, 1, 10, 1, 5), Some(3));
    assert_eq!(row_from_mouse(11, 1, 10, 0, 20), None);
    assert_eq!(row_from_mouse(0, 1, 10, 0, 5), None);
}

#[test]
fn scroll_list_by_clamps_to_bounds() {
    assert_eq!(scroll_list_by(5, -10, 20, 10), 0);
    assert_eq!(scroll_list_by(5, 3, 20, 10), 8);
    assert_eq!(scroll_list_by(10, 5, 20, 10), 10);
    assert_eq!(scroll_list_by(0, -1, 8, 10), 0);
}

#[test]
fn ensure_range_visible_scrolls_minimally() {
    assert_eq!(ensure_range_visible(3, 8, 0, 10), 0);
    assert_eq!(ensure_range_visible(3, 8, 5, 10), 3);
    assert_eq!(ensure_range_visible(3, 8, 6, 10), 3);
    assert_eq!(ensure_range_visible(3, 8, 2, 10), 2);
    assert_eq!(ensure_range_visible(3, 20, 0, 10), 3);
    assert_eq!(ensure_range_visible(3, 20, 5, 10), 3);
}

#[test]
fn ensure_active_note_visible_does_not_jump_to_list_bottom() {
    let notes = vec![Note::new("one", "", false), Note::new("two", "", true)];
    let state = notepad_list_state(&notes, true, false, Some(1));
    let trail_base = 5usize;
    let body_height = 8usize;
    let title_row = notepad_note_title_row_index(1, trail_base, &state).expect("note title");
    let (_, body_end) = notepad_note_body_row_range(1, trail_base, &state).expect("note body");
    assert!(body_end > body_height);

    let scroll = ensure_active_note_visible(0, body_height, trail_base, &state);
    assert_eq!(scroll, title_row);
    assert!(scroll < body_end.saturating_sub(body_height));
}

#[test]
fn pointer_in_list_viewport_y_excludes_toolbar_and_settings() {
    let metrics = LayoutMetrics {
        frame_width: 40,
        frame_height: 30,
        list_height: 15,
        list_top_y: 6,
        list_inner_x: 0,
        list_line_width: 38,
        toolbar_top_y: 1,
        toolbar_row_count: 4,
        update_banner_top_y: 0,
        update_banner_row_count: 0,
        settings_top_y: 27,
        settings_row_count: 1,
        leave_top_y: 28,
        leave_row_count: 1,
        notepad_top_y: 20,
        notepad_header_y: 21,
        notepad_body_top_y: 22,
        notepad_body_rows: 0,
        notepad_expanded: false,
        sessions_title_y: 5,
    };
    assert!(!pointer_in_list_viewport_y(5, &metrics));
    assert!(pointer_in_list_viewport_y(6, &metrics));
    assert!(pointer_in_list_viewport_y(26, &metrics));
    assert!(!pointer_in_list_viewport_y(27, &metrics));
}

#[test]
fn notepad_selection_cursor_clamps_to_document_edges() {
    let list_top_y = 4u16;
    let metrics = LayoutMetrics {
        frame_width: 24,
        frame_height: 24,
        list_height: 20,
        list_top_y,
        list_inner_x: 0,
        list_line_width: 20,
        toolbar_top_y: 0,
        toolbar_row_count: 2,
        update_banner_top_y: 0,
        update_banner_row_count: 0,
        settings_top_y: 18,
        settings_row_count: 1,
        leave_top_y: 19,
        leave_row_count: 1,
        notepad_top_y: 14,
        notepad_header_y: list_top_y + 1,
        notepad_body_top_y: list_top_y + 2,
        notepad_body_rows: NOTEPAD_BODY_ROWS,
        notepad_expanded: true,
        sessions_title_y: 3,
    };
    let notes = sample_expanded_notes("hello\nworld");
    let state = sample_notepad_state(&notes);
    let text = "hello\nworld";
    let (body_start, body_end) = notepad_note_body_row_range(0, 0, &state).expect("note body rows");
    let above_body_row_y = list_top_y + body_start.saturating_sub(1) as u16;
    assert_eq!(
        notepad_selection_cursor_from_mouse(
            5,
            above_body_row_y,
            &metrics,
            0,
            0,
            &state,
            0,
            text,
            0,
            None,
        ),
        Some(2)
    );
    let below_body_row_y = list_top_y + body_end as u16;
    assert_eq!(
        notepad_selection_cursor_from_mouse(
            5,
            below_body_row_y,
            &metrics,
            0,
            0,
            &state,
            0,
            text,
            0,
            None,
        ),
        Some(8)
    );
}

#[test]
fn notepad_terminal_cursor_hidden_when_scrolled_above_viewport() {
    let terminal_area = Rect::new(0, 4, 20, 10);
    let notes = sample_expanded_notes(&"one\n".repeat(20));
    let state = sample_notepad_state(&notes);
    let text = "one\n".repeat(20);
    assert!(notepad_terminal_cursor_position(
        terminal_area,
        0,
        10,
        0,
        &state,
        0,
        true,
        &text,
        0,
        5,
        terminal_area.width as usize,
        None,
        None,
        false,
    )
    .is_none());
}

#[test]
fn notepad_terminal_cursor_hidden_when_scrolled_below_viewport() {
    let terminal_area = Rect::new(0, 4, 20, 10);
    let notes = sample_expanded_notes(&"one\n".repeat(20));
    let state = sample_notepad_state(&notes);
    let text = "one\n".repeat(20);
    let cursor = text.chars().count();
    assert!(notepad_terminal_cursor_position(
        terminal_area,
        0,
        10,
        0,
        &state,
        0,
        true,
        &text,
        cursor,
        0,
        terminal_area.width as usize,
        None,
        None,
        false,
    )
    .is_none());
}

#[test]
fn notepad_terminal_cursor_round_trips_with_mouse_mapping() {
    let terminal_area = Rect::new(0, 4, 20, 10);
    let notes = sample_expanded_notes("hello\nworld");
    let state = sample_notepad_state(&notes);
    let text = "hello\nworld";
    let scroll = 0usize;
    let body_height = 10usize;
    let visible_sessions = 0usize;
    let body_y = terminal_area.y + first_note_body_content_row(visible_sessions, &notes) as u16;
    let pos = notepad_terminal_cursor_position(
        terminal_area,
        scroll,
        body_height,
        visible_sessions,
        &state,
        0,
        true,
        text,
        2,
        0,
        terminal_area.width as usize,
        None,
        None,
        false,
    )
    .expect("cursor should be visible");
    assert_eq!(pos.x, 5);
    assert_eq!(pos.y, body_y);
    let metrics = LayoutMetrics {
        frame_width: 20,
        frame_height: 20,
        list_height: body_height,
        list_top_y: terminal_area.y,
        list_inner_x: terminal_area.x,
        list_line_width: terminal_area.width as usize,
        toolbar_top_y: 0,
        toolbar_row_count: 2,
        update_banner_top_y: 0,
        update_banner_row_count: 0,
        settings_top_y: 18,
        settings_row_count: 1,
        leave_top_y: 19,
        leave_row_count: 1,
        notepad_top_y: 14,
        notepad_header_y: terminal_area.y + 1,
        notepad_body_top_y: body_y,
        notepad_body_rows: NOTEPAD_BODY_ROWS,
        notepad_expanded: true,
        sessions_title_y: 3,
    };
    let state = sample_notepad_state(&notes);
    assert_eq!(
        notepad_cursor_from_mouse(
            pos.x,
            pos.y,
            &metrics,
            scroll,
            visible_sessions,
            &state,
            0,
            text,
            0,
            None,
        ),
        Some(2)
    );
}

#[test]
fn notepad_cursor_from_mouse_maps_click_to_text_position() {
    let list_top_y = 4u16;
    let metrics = LayoutMetrics {
        frame_width: 24,
        frame_height: 20,
        list_height: 10,
        list_top_y,
        list_inner_x: 0,
        list_line_width: 20,
        toolbar_top_y: 0,
        toolbar_row_count: 2,
        update_banner_top_y: 0,
        update_banner_row_count: 0,
        settings_top_y: 18,
        settings_row_count: 1,
        leave_top_y: 19,
        leave_row_count: 1,
        notepad_top_y: 14,
        notepad_header_y: list_top_y + 1,
        notepad_body_top_y: list_top_y + 2,
        notepad_body_rows: NOTEPAD_BODY_ROWS,
        notepad_expanded: true,
        sessions_title_y: 3,
    };
    let notes = sample_expanded_notes("hello\nworld");
    let state = sample_notepad_state(&notes);
    let text = "hello\nworld";
    let body_y = list_top_y + first_note_body_content_row(0, &notes) as u16;
    assert_eq!(
        notepad_cursor_from_mouse(5, body_y, &metrics, 0, 0, &state, 0, text, 0, None),
        Some(2)
    );
    let second_line_y = body_y + 1;
    assert_eq!(
        notepad_cursor_from_mouse(5, second_line_y, &metrics, 0, 0, &state, 0, text, 0, None),
        Some(8)
    );
}

#[test]
fn pointer_in_list_body_requires_column_within_line_width() {
    let metrics = LayoutMetrics {
        frame_width: 24,
        frame_height: 20,
        list_height: 10,
        list_top_y: 4,
        list_inner_x: 0,
        list_line_width: 20,
        toolbar_top_y: 0,
        toolbar_row_count: 2,
        update_banner_top_y: 0,
        update_banner_row_count: 0,
        settings_top_y: 18,
        settings_row_count: 1,
        leave_top_y: 19,
        leave_row_count: 1,
        notepad_top_y: 14,
        notepad_header_y: 14,
        notepad_body_top_y: 15,
        notepad_body_rows: 0,
        notepad_expanded: false,
        sessions_title_y: 13,
    };
    assert!(pointer_in_list_body(0, &metrics));
    assert!(pointer_in_list_body(19, &metrics));
    assert!(!pointer_in_list_body(20, &metrics));
}

#[test]
fn state_style_done_matches_idle_background() {
    let done = state_style(AgentState::Done, false, false);
    let idle = state_style(AgentState::Idle, false, false);
    assert_eq!(done.bg, Some(BG_BASE));
    assert_eq!(done.bg, idle.bg);
}

#[test]
fn session_rows_use_neutral_backgrounds_only() {
    assert_eq!(
        state_style(AgentState::Working, false, false).bg,
        Some(BG_BASE)
    );
    assert_eq!(
        state_style(AgentState::Approval, false, false).bg,
        Some(BG_BASE)
    );
    assert_eq!(state_style(AgentState::Idle, false, true).bg, Some(BG_BASE));
    let selected = state_style(AgentState::Working, true, true);
    assert_eq!(selected.bg, Some(BG_SELECTED));
    assert_eq!(selected.fg, Some(TEXT_SELECTED));
    assert!(!selected.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn session_row_backdrop_covers_hovered_rows() {
    let sessions = vec![sample_session("one", 1, 0), sample_session("two", 2, 1)];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let first = rows
        .iter()
        .position(|row| matches!(row, RowKind::Session { .. }))
        .unwrap();
    let second = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, RowKind::Session { .. }))
        .nth(1)
        .map(|(idx, _)| idx)
        .unwrap();
    let sections = group_sections(&rows);
    let hovered = match &rows[second] {
        RowKind::Session { session, .. } => session,
        _ => unreachable!(),
    };
    assert_eq!(
        session_row_backdrop_bg(
            hovered,
            second,
            first,
            false,
            Some(second),
            None,
            &rows,
            &sections,
            &GroupDragState::default(),
        ),
        Some(BG_HIGHLIGHT)
    );
    assert_eq!(
        session_row_backdrop_bg(
            hovered,
            second,
            first,
            true,
            None,
            Some(second),
            &rows,
            &sections,
            &GroupDragState::default(),
        ),
        Some(CLOSE_HOVER_BG)
    );
}

#[test]
fn session_row_backdrop_distinguishes_selected_and_hovered() {
    let sessions = vec![sample_session("one", 1, 0), sample_session("two", 2, 1)];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let first = rows
        .iter()
        .position(|row| matches!(row, RowKind::Session { .. }))
        .unwrap();
    let session = match &rows[first] {
        RowKind::Session { session, .. } => session,
        _ => unreachable!(),
    };
    let sections = group_sections(&rows);
    assert_eq!(
        session_row_backdrop_bg(
            session,
            first,
            first,
            false,
            Some(first),
            None,
            &rows,
            &sections,
            &GroupDragState::default(),
        ),
        Some(BG_HOVER_SELECTED)
    );
}

#[test]
fn session_row_backdrop_covers_selected_rows() {
    let session = sample_session("ship api", 1, 0);
    let rows = build_rows(&[session], &HashSet::new(), &HashSet::new(), &[]);
    let session_row = rows
        .iter()
        .position(|row| matches!(row, RowKind::Session { .. }))
        .unwrap();
    let sections = group_sections(&rows);
    let session = match &rows[session_row] {
        RowKind::Session { session, .. } => session,
        _ => unreachable!(),
    };
    assert_eq!(
        session_row_backdrop_bg(
            session,
            session_row,
            session_row,
            false,
            None,
            None,
            &rows,
            &sections,
            &GroupDragState::default(),
        ),
        Some(BG_SELECTED)
    );
    assert_eq!(
        session_row_backdrop_bg(
            session,
            session_row,
            session_row + 1,
            false,
            None,
            None,
            &rows,
            &sections,
            &GroupDragState::default(),
        ),
        None
    );
}

#[test]
fn session_rows_use_same_white_backdrop_signals_selection() {
    let unselected = state_style(AgentState::Idle, false, false);
    let selected = state_style(AgentState::Idle, true, false);
    assert_eq!(unselected.fg, Some(TEXT_SELECTED));
    assert_eq!(selected.fg, Some(TEXT_SELECTED));
    assert_eq!(unselected.bg, Some(BG_BASE));
    assert_eq!(selected.bg, Some(BG_SELECTED));
    assert!(!selected.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn list_row_backdrop_covers_group_and_notepad_header_hovers() {
    let sessions = vec![sample_session("api", 1, 0)];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let notes = sample_expanded_notes("");
    let notepad_state = sample_notepad_state(&notes);
    let line_width = default_sidebar_line_width();
    let group_row = rows
        .iter()
        .position(|row| matches!(row, RowKind::Group { .. }))
        .unwrap();
    let sections = group_sections(&rows);
    assert_eq!(
        list_row_backdrop_bg(
            group_row,
            rows.len(),
            &rows,
            0,
            false,
            None,
            None,
            Some(group_row),
            false,
            &notepad_state,
            line_width,
            None,
            &GroupDragState::default(),
            &sections,
            &NoteDragState::default(),
            &[],
            None,
        ),
        Some(BG_HIGHLIGHT)
    );
    let header_row = notepad_header_row_index(rows.len(), true, &notepad_state);
    assert_eq!(
        list_row_backdrop_bg(
            header_row,
            rows.len(),
            &rows,
            0,
            false,
            None,
            None,
            None,
            true,
            &notepad_state,
            line_width,
            None,
            &GroupDragState::default(),
            &sections,
            &NoteDragState::default(),
            &[],
            None,
        ),
        Some(BG_HIGHLIGHT)
    );
    let (body_row, _) =
        notepad_note_body_row_range(0, rows.len(), &notepad_state).expect("note body rows");
    assert_eq!(
        list_row_backdrop_bg(
            body_row,
            rows.len(),
            &rows,
            0,
            false,
            None,
            None,
            None,
            false,
            &notepad_state,
            line_width,
            None,
            &GroupDragState::default(),
            &sections,
            &NoteDragState::default(),
            &[],
            None,
        ),
        Some(NOTEPAD_EDIT_BG)
    );
    let title_row = notepad_note_title_row_index(0, rows.len(), &notepad_state).unwrap();
    let active_state = notepad_list_state(&notes, true, true, Some(0));
    assert_eq!(
        list_row_backdrop_bg(
            title_row,
            rows.len(),
            &rows,
            0,
            false,
            None,
            None,
            None,
            false,
            &active_state,
            line_width,
            None,
            &GroupDragState::default(),
            &sections,
            &NoteDragState::default(),
            &[],
            None,
        ),
        Some(BG_SELECTED)
    );
    assert_eq!(
        list_row_backdrop_bg(
            title_row,
            rows.len(),
            &rows,
            0,
            false,
            None,
            None,
            None,
            false,
            &active_state,
            line_width,
            Some(0),
            &GroupDragState::default(),
            &sections,
            &NoteDragState::default(),
            &[],
            None,
        ),
        Some(BG_HOVER_SELECTED)
    );
}

fn chrome_row_backdrop_matches_session_hover_and_selection_signals() {
    assert_eq!(chrome_row_backdrop_bg(false, false), None);
    assert_eq!(chrome_row_backdrop_bg(true, false), Some(BG_HIGHLIGHT));
    assert_eq!(chrome_row_backdrop_bg(false, true), Some(BG_SELECTED));
    assert_eq!(chrome_row_backdrop_bg(true, true), Some(BG_HOVER_SELECTED));
    assert_eq!(chrome_button_style(true, false).bg, Some(BG_HIGHLIGHT));
    assert_eq!(chrome_button_style(false, true).bg, Some(BG_SELECTED));
}

#[test]
fn toolbar_active_follows_open_workspace_panel() {
    assert!(toolbar_action_is_active(
        ToolbarAction::Automations,
        false,
        true,
        false,
        false
    ));
    assert!(toolbar_action_is_active(
        ToolbarAction::Mcps,
        false,
        false,
        true,
        false
    ));
    assert!(toolbar_action_is_active(
        ToolbarAction::Skills,
        false,
        false,
        false,
        true
    ));
    assert!(toolbar_action_is_active(
        ToolbarAction::NewSession,
        true,
        false,
        false,
        false
    ));
    assert!(!toolbar_action_is_active(
        ToolbarAction::Search,
        true,
        true,
        true,
        true
    ));
    assert!(!toolbar_action_is_active(
        ToolbarAction::Automations,
        false,
        false,
        false,
        false
    ));
}

#[test]
fn run_spinner_cycles_through_dots13_frames() {
    assert_eq!(run_spinner_glyph(0), "⣶");
    assert_eq!(run_spinner_glyph(3), "⡟");
    assert_eq!(run_spinner_glyph(8), "⣶");
}

#[test]
fn session_trailing_badge_uses_spinner_for_running_sessions() {
    let mut running = sample_session("ship api", 1, 0);
    running.state = AgentState::Working;
    let row_style = state_style(AgentState::Idle, false, false);
    let (text, style) = session_trailing_badge(&running, row_style, 3);
    assert_eq!(text, format_spinner_slot("⡟"));
    assert_eq!(style.fg, Some(TEXT_SELECTED));
    assert_eq!(style.bg, Some(BG_BASE));
    assert!(style.add_modifier.contains(Modifier::BOLD));

    let selected_style = state_style(AgentState::Idle, true, false);
    let (text, style) = session_trailing_badge(&running, selected_style, 3);
    assert_eq!(text, format_spinner_slot("⡟"));
    assert_eq!(style.fg, Some(TEXT_SELECTED));

    let mut approval = sample_session("ship api", 3, 0);
    approval.state = AgentState::Approval;
    let (text, style) = session_trailing_badge(&approval, row_style, 3);
    assert_eq!(text, format_spinner_slot("⡟"));
    assert_eq!(style.fg, Some(TEXT_SELECTED));

    let idle = sample_session("idle", 2, 5);
    let (text, _) = session_trailing_badge(&idle, row_style, 0);
    assert_eq!(text, format_trailing_slot("5m"));

    let mut fresh = sample_session("fresh", 5, 0);
    fresh.prompt_submitted = false;
    fresh.messaged_at = None;
    let (text, _) = session_trailing_badge(&fresh, row_style, 0);
    assert_eq!(text, format_trailing_slot(""));

    let acknowledged = completed_session("ship api", 4, 5);
    let mut acknowledged = acknowledged;
    assert!(acknowledged.acknowledge_if_done());
    let (text, _) = session_trailing_badge(&acknowledged, row_style, 0);
    assert_eq!(text, format_trailing_slot("5m"));
}

#[test]
fn session_trailing_badge_uses_green_square_for_completed_threads() {
    let done = completed_session("ship api", 2, 0);
    let row_style = state_style(AgentState::Done, false, false);
    let (text, style) = session_trailing_badge(&done, row_style, 0);
    assert_eq!(text, format_completion_square_slot());
    assert_eq!(style.fg, Some(DONE_FG));
    assert_ne!(style.fg, Some(GROK_GREEN));
    assert!(style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn session_trailing_badge_shows_time_after_completion_acknowledged() {
    let mut done = completed_session("ship api", 2, 5);
    assert!(done.acknowledge_if_done());
    let row_style = state_style(AgentState::Idle, false, false);
    let (text, style) = session_trailing_badge(&done, row_style, 0);
    assert_eq!(text, format_trailing_slot("5m"));
    assert_eq!(style.fg, Some(PATH_FG));
    assert_eq!(done.sidebar_state(), AgentState::Idle);
}

#[test]
fn session_trailing_badge_keeps_time_grey_when_selected() {
    let mut done = completed_session("ship api", 2, 5);
    assert!(done.acknowledge_if_done());
    let selected_style = state_style(AgentState::Idle, true, true);
    let (_, style) = session_trailing_badge(&done, selected_style, 0);
    assert_eq!(style.fg, Some(PATH_FG));
    assert_eq!(style.bg, Some(BG_SELECTED));
}

#[test]
fn sessions_block_title_keeps_sessions_label_in_close_mode() {
    use super::widgets::DeleteNoteConfirmState;
    let normal = sessions_block_title(false, "", None, None, true, false, false, 0, None);
    assert_eq!(normal.spans.len(), 1);
    assert_eq!(normal.spans[0].content, " ▾ sessions ");

    let collapsed = sessions_block_title(false, "", None, None, false, false, false, 0, None);
    assert_eq!(collapsed.spans[0].content, " ▸ sessions ");

    let close = sessions_block_title(true, "", None, None, true, false, false, 0, None);
    assert_eq!(close.spans.len(), 2);
    assert_eq!(close.spans[0].content, " ▾ sessions ");
    assert_eq!(close.spans[1].content, " hold d · enter delete · esc exit ");

    let confirm = DeleteNoteConfirmState {
        note_id: "n1".into(),
        title: "Scratch".into(),
        buffer: String::new(),
    };
    let delete = sessions_block_title(false, "", None, Some(&confirm), true, false, false, 0, None);
    assert_eq!(delete.spans[0].content, " delete note ");
    assert_eq!(
        delete.spans[1].content,
        " type yes · enter confirm · esc cancel "
    );
}

#[test]
fn sessions_block_title_shows_add_button_on_hover() {
    let line = sessions_block_title(false, "", None, None, true, true, false, 48, None);
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert!(text.contains("▾ sessions"));
    assert!(text.contains(GROUP_ADD_ICON));
}

#[test]
fn collapse_control_is_labeled_and_top_right() {
    assert_eq!(collapse_control_label(), "[collapse]");
    assert_eq!(collapse_control_label(), RAIL_COLLAPSE_LABEL);

    let metrics = LayoutMetrics {
        frame_width: 53,
        frame_height: 40,
        list_height: 20,
        list_top_y: 8,
        list_inner_x: 0,
        list_line_width: 48,
        toolbar_top_y: 1,
        toolbar_row_count: 5,
        update_banner_top_y: 30,
        update_banner_row_count: 0,
        settings_top_y: 30,
        settings_row_count: 1,
        leave_top_y: 31,
        leave_row_count: 1,
        notepad_top_y: 20,
        notepad_header_y: 20,
        notepad_body_top_y: 21,
        notepad_body_rows: 0,
        notepad_expanded: false,
        sessions_title_y: 7,
    };
    // Pad row above toolbar buttons.
    assert_eq!(collapse_control_y(&metrics), 0);
    let start = collapse_control_start_x(&metrics).expect("wide enough");
    let w = collapse_control_width() as u16;
    assert_eq!(start + w, 48);
    assert!(collapse_control_hit(start, 0, &metrics));
    assert!(collapse_control_hit(start + w - 1, 0, &metrics));
    assert!(!collapse_control_hit(start.saturating_sub(1), 0, &metrics));
    assert!(!collapse_control_hit(start, 1, &metrics)); // toolbar row, not control
}

#[test]
fn collapse_control_is_text_only_with_session_white_hover() {
    // Default muted grey; hover uses session-row white (TEXT_SELECTED). No bg highlight.
    assert_eq!(PATH_FG, Color::Rgb(110, 110, 110));
    assert_eq!(TEXT_SELECTED, Color::Rgb(223, 223, 223));
    assert_eq!(collapse_control_label(), RAIL_COLLAPSE_LABEL);
    assert!(!collapse_control_label().contains('◂'));
    // Hit target is always the full control — click does not require hover state.
    let metrics = LayoutMetrics {
        frame_width: 53,
        frame_height: 40,
        list_height: 20,
        list_top_y: 8,
        list_inner_x: 0,
        list_line_width: 48,
        toolbar_top_y: 1,
        toolbar_row_count: 5,
        update_banner_top_y: 30,
        update_banner_row_count: 0,
        settings_top_y: 30,
        settings_row_count: 1,
        leave_top_y: 31,
        leave_row_count: 1,
        notepad_top_y: 20,
        notepad_header_y: 20,
        notepad_body_top_y: 21,
        notepad_body_rows: 0,
        notepad_expanded: false,
        sessions_title_y: 7,
    };
    let start = collapse_control_start_x(&metrics).expect("wide enough");
    assert!(collapse_control_hit(start, 0, &metrics));
    assert_eq!(
        collapse_control_hover_from_mouse(start, 0, &metrics),
        collapse_control_hit(start, 0, &metrics)
    );
}

#[test]
fn notepad_header_label_omits_content_when_collapsed() {
    assert_eq!(notepad_header_label(false), "▸ notes");
    assert_eq!(notepad_header_label(true), "▾ notes");
}

#[test]
fn notepad_save_status_text_keeps_last_saved_while_unsaved() {
    assert_eq!(notepad_save_status_text(None), None);
    let at = Utc::now() - chrono::Duration::minutes(5);
    assert_eq!(
        notepad_save_status_text(Some(at)),
        Some("saved 5m ago".into())
    );
}

#[test]
fn notepad_save_status_text_shows_saved_time_ago() {
    let at = Utc::now() - chrono::Duration::minutes(5);
    assert_eq!(
        notepad_save_status_text(Some(at)),
        Some("saved 5m ago".into())
    );
}

#[test]
fn format_save_time_ago_scales_units_progressively() {
    let fresh = Utc::now() - chrono::Duration::seconds(12);
    assert_eq!(format_save_time_ago(fresh), "1m ago");
    let mins = Utc::now() - chrono::Duration::minutes(3);
    assert_eq!(format_save_time_ago(mins), "3m ago");
    let hours = Utc::now() - chrono::Duration::hours(2);
    assert_eq!(format_save_time_ago(hours), "2hr ago");
    let days = Utc::now() - chrono::Duration::days(4);
    assert_eq!(format_save_time_ago(days), "4d ago");
    let weeks = Utc::now() - chrono::Duration::weeks(2);
    assert_eq!(format_save_time_ago(weeks), "2wk ago");
}

#[test]
fn format_save_time_ago_floors_sub_minute_to_one_minute() {
    let just_saved = Utc::now();
    assert_eq!(format_save_time_ago(just_saved), "1m ago");
}

#[test]
fn notepad_section_header_line_places_saved_status_beside_notes() {
    let at = Utc::now() - chrono::Duration::minutes(3);
    let line = notepad_section_header_line(48, true, false, false, Some(at));
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert!(text.contains("▾ notes — saved 3m ago"));
}

#[test]
fn notepad_section_header_line_keeps_saved_status_on_add_hover() {
    let at = Utc::now() - chrono::Duration::minutes(3);
    let line = notepad_section_header_line(48, true, true, false, Some(at));
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert!(text.contains("▾ notes — saved 3m ago"));
    assert!(text.contains(GROUP_ADD_ICON));
}

#[test]
fn close_mode_muted_style_greys_text_not_background() {
    let working = close_mode_muted_style(AgentState::Working, false, false);
    assert_eq!(working.fg, Some(CLOSE_MODE_FG));
    assert_eq!(working.bg, Some(BG_BASE));

    let selected = close_mode_muted_style(AgentState::Idle, true, true);
    assert_eq!(selected.fg, Some(CLOSE_MODE_FG));
    assert_eq!(selected.bg, Some(BG_SELECTED));
}

#[test]
fn delete_note_confirm_ready_only_when_buffer_is_yes() {
    use super::widgets::{delete_note_confirm_ready, DeleteNoteConfirmState};
    let mut confirm = DeleteNoteConfirmState {
        note_id: "n1".into(),
        title: "Note 1".into(),
        buffer: "ye".into(),
    };
    assert!(!delete_note_confirm_ready(&confirm));
    confirm.buffer.push('s');
    assert!(delete_note_confirm_ready(&confirm));
}

#[test]
fn note_close_target_row_marks_only_targeted_note_title() {
    use super::layout::notepad_list_state;
    use super::notepad::note_close_target_row;
    use crate::bar::notepad::Note;
    let notes = vec![Note::new("One", "", false), Note::new("Two", "", false)];
    let state = notepad_list_state(&notes, true, true, Some(0));
    let trail_base = 4usize;
    let title_one = notepad_note_title_row_index(0, trail_base, &state).unwrap();
    let title_two = notepad_note_title_row_index(1, trail_base, &state).unwrap();
    assert!(note_close_target_row(
        title_two,
        trail_base,
        true,
        Some(title_two),
        &state,
        40,
    ));
    assert!(!note_close_target_row(
        title_one,
        trail_base,
        true,
        Some(title_two),
        &state,
        40,
    ));
}

#[test]
fn close_target_lead_is_one_column_in_from_edge() {
    assert_eq!(" ✕".chars().count(), 2);
}

#[test]
fn note_close_target_x_aligns_with_session_close_lead() {
    let session_lead = " ✕";
    let note_prefix = notepad_note_title_prefix(false, true);
    assert_eq!(session_lead.chars().nth(1), note_prefix.chars().nth(1));
}

#[test]
fn close_target_row_only_marks_hovered_session() {
    let sessions = vec![sample_session("one", 1, 0), sample_session("two", 2, 1)];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let first = rows
        .iter()
        .position(|row| matches!(row, RowKind::Session { .. }))
        .unwrap();
    let second = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, RowKind::Session { .. }))
        .nth(1)
        .map(|(idx, _)| idx)
        .unwrap();

    assert!(!close_target_row(&rows, true, None, first, first));
    assert!(!close_target_row(&rows, true, None, first, second));
    assert!(close_target_row(&rows, true, Some(second), first, second));
}

#[test]
fn close_target_row_prefers_hover_over_selection() {
    let sessions = vec![sample_session("one", 1, 0), sample_session("two", 2, 1)];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let first = rows
        .iter()
        .position(|row| matches!(row, RowKind::Session { .. }))
        .unwrap();
    let second = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, RowKind::Session { .. }))
        .nth(1)
        .map(|(idx, _)| idx)
        .unwrap();

    assert!(close_target_row(&rows, true, Some(second), first, second));
    assert!(!close_target_row(&rows, true, Some(second), first, first));
    assert!(!close_target_row(&rows, true, None, first, first));
    assert!(!close_target_row(&rows, true, None, first, second));
}

#[test]
fn sidebar_row_uses_sidebar_state_not_live_working_color() {
    let mut working = sample_session("ship api", 1, 0);
    working.state = AgentState::Working;
    assert_eq!(working.sidebar_state(), AgentState::Idle);

    let done = completed_session("ship api", 2, 0);
    assert_eq!(done.sidebar_state(), AgentState::Done);
}

#[test]
fn session_label_omits_non_agent_project_prefix() {
    let session = sample_session("fix sidebar", 1, 0);
    assert_eq!(session_display_label(&session), "fix sidebar");
}

#[test]
fn session_label_shows_agent_application_and_thread() {
    let mut session = sample_session("fix sidebar", 1, 0);
    session.title = "codex · fix sidebar".into();
    session.project = "codex".into();
    assert_eq!(session_display_label(&session), "codex · fix sidebar");
}

#[test]
fn session_label_shows_agent_application_without_thread() {
    let mut session = sample_session("grok", 1, 0);
    session.title = "grok".into();
    session.project = "grok".into();
    assert_eq!(session_display_label(&session), "grok");
}

#[test]
fn session_label_uses_console_for_idle_shell() {
    let mut session = sample_session("console", 1, 0);
    session.title = "acme · console".into();
    session.project = "acme".into();
    assert_eq!(session_display_label(&session), "console");
}

#[test]
fn session_label_normalizes_legacy_raw_terminal_to_console() {
    let mut session = sample_session("acme", 1, 0);
    session.title = "acme · raw terminal".into();
    session.project = "acme".into();
    assert_eq!(session_display_label(&session), "console");
}

#[test]
fn build_rows_uses_saved_group_order() {
    let mut sessions = vec![
        sample_session("old-dir", 1, 60),
        sample_session("new-dir", 2, 5),
    ];
    sessions[0].cwd_label = "~/projects/old".into();
    sessions[1].cwd_label = "~/projects/new".into();
    let saved = vec!["~/projects/new".into(), "~/projects/old".into()];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &saved);
    let labels: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            RowKind::Group { label, .. } => Some(label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["~/projects/new", "~/projects/old"]);
}

#[test]
fn build_rows_defaults_unknown_groups_alphabetically() {
    let mut sessions = vec![
        sample_session("old-dir", 1, 60),
        sample_session("new-dir", 2, 5),
    ];
    sessions[0].cwd_label = "~/projects/old".into();
    sessions[1].cwd_label = "~/projects/new".into();
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let labels: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            RowKind::Group { label, .. } => Some(label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["~/projects/new", "~/projects/old"]);
}

#[test]
fn build_rows_orders_group_by_newest_activity() {
    let sessions = vec![
        completed_session("older", 1, 30),
        completed_session("newer", 2, 5),
    ];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let labels: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            RowKind::Session { session } => Some(session.description.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["newer", "older"]);
}

#[test]
fn build_rows_puts_newest_non_running_session_first() {
    let mut sessions = vec![
        sample_session("stale approval", 8, 24),
        sample_session("fresh approval", 16, 0),
    ];
    sessions[0].state = AgentState::Approval;
    sessions[1].state = AgentState::Approval;
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let labels: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            RowKind::Session { session } => Some(session.description.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["fresh approval", "stale approval"]);
}

#[test]
fn build_rows_orders_by_last_message_not_running_state() {
    let mut sessions = vec![
        sample_session("running", 3, 2),
        sample_session("fresh", 16, 0),
        sample_session("stale-running", 8, 27),
    ];
    sessions[0].state = AgentState::Working;
    sessions[2].state = AgentState::Working;
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let labels: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            RowKind::Session { session } => Some(session.description.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["fresh", "running", "stale-running"]);
}

#[test]
fn build_rows_keeps_message_order_when_running_sessions_use_tools() {
    let mut sessions = vec![
        sample_session("older prompt", 3, 30),
        sample_session("newer prompt", 16, 1),
    ];
    sessions[0].state = AgentState::Working;
    sessions[1].state = AgentState::Working;
    // Tool hooks on the older session keep bumping last_event_at.
    sessions[0].last_event_at = Utc::now();
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let labels: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            RowKind::Session { session } => Some(session.description.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["newer prompt", "older prompt"]);
}

#[test]
fn build_rows_collapses_groups_over_limit() {
    let sessions: Vec<_> = (1..=14)
        .map(|i| sample_session(&format!("thread-{i}"), i, i as i64))
        .collect();
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let labels: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            RowKind::Session { session } => Some(session.description.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels.len(), MAX_THREADS_PER_GROUP);
    assert!(rows.iter().any(|row| {
        matches!(
            row,
            RowKind::GroupToggle {
                hidden_count: 8,
                expanded: false,
                ..
            }
        )
    }));
}

#[test]
fn build_rows_collapsed_group_respects_message_order_not_running_state() {
    let mut sessions: Vec<_> = (1..=14)
        .map(|i| sample_session(&format!("thread-{i}"), i, i as i64))
        .collect();
    sessions[13].state = AgentState::Working;
    sessions[13].last_event_at = Utc::now();
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let labels: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            RowKind::Session { session } => Some(session.description.as_str()),
            _ => None,
        })
        .collect();
    assert!(!labels.iter().any(|label| *label == "thread-14"));
    assert_eq!(
        labels,
        vec!["thread-1", "thread-2", "thread-3", "thread-4", "thread-5", "thread-6"]
    );
}

#[test]
fn build_rows_collapsed_group_respects_message_order_not_active_focus() {
    let mut sessions: Vec<_> = (1..=14)
        .map(|i| sample_session(&format!("thread-{i}"), i, i as i64))
        .collect();
    sessions[13].is_active = true;
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let labels: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            RowKind::Session { session } => Some(session.description.as_str()),
            _ => None,
        })
        .collect();
    assert!(!labels.iter().any(|label| *label == "thread-14"));
    assert_eq!(labels.len(), MAX_THREADS_PER_GROUP);
}

#[test]
fn build_rows_skips_toggle_when_group_is_under_limit() {
    let sessions: Vec<_> = (1..=4)
        .map(|i| sample_session(&format!("thread-{i}"), i, i as i64))
        .collect();
    let mut expanded = HashSet::new();
    expanded.insert("~/tmp".into());
    let rows = build_rows(&sessions, &expanded, &HashSet::new(), &[]);
    assert!(!rows
        .iter()
        .any(|row| matches!(row, RowKind::GroupToggle { .. })));
}

#[test]
fn build_rows_expanded_group_always_shows_show_less() {
    let sessions: Vec<_> = (1..=14)
        .map(|i| sample_session(&format!("thread-{i}"), i, i as i64))
        .collect();
    let mut expanded = HashSet::new();
    expanded.insert("~/tmp".into());
    let rows = build_rows(&sessions, &expanded, &HashSet::new(), &[]);
    assert!(rows.iter().any(|row| {
        matches!(
            row,
            RowKind::GroupToggle {
                expanded: true,
                hidden_count: 0,
                ..
            }
        )
    }));
}

#[test]
fn build_rows_expands_collapsed_group() {
    let sessions: Vec<_> = (1..=14)
        .map(|i| sample_session(&format!("thread-{i}"), i, i as i64))
        .collect();
    let mut expanded = HashSet::new();
    expanded.insert("~/tmp".into());
    let rows = build_rows(&sessions, &expanded, &HashSet::new(), &[]);
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row, RowKind::Session { .. }))
            .count(),
        14
    );
    assert!(rows
        .iter()
        .any(|row| { matches!(row, RowKind::GroupToggle { expanded: true, .. }) }));
}

#[test]
fn folded_group_hides_sessions_under_pwd_header() {
    let sessions = vec![sample_session("one", 1, 0), sample_session("two", 2, 0)];
    let mut folded = HashSet::new();
    folded.insert("~/tmp".to_string());
    let rows = build_rows(&sessions, &HashSet::new(), &folded, &[]);
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row, RowKind::Session { .. }))
            .count(),
        0
    );
    assert!(rows.iter().any(|row| {
        matches!(
            row,
            RowKind::Group {
                label,
                collapsed: true,
            } if label == "~/tmp"
        )
    }));
}

#[test]
fn desired_pane_width_is_fixed_default() {
    let short = build_rows(
        &[sample_session("api", 1, 0)],
        &HashSet::new(),
        &HashSet::new(),
        &[],
    );
    let long = build_rows(
        &[sample_session(
            "implement dynamic sidebar spacing for long session titles",
            1,
            0,
        )],
        &HashSet::new(),
        &HashSet::new(),
        &[],
    );
    assert_eq!(desired_pane_width(&short, 1, ""), DEFAULT_PANE_WIDTH);
    assert_eq!(desired_pane_width(&long, 1, ""), DEFAULT_PANE_WIDTH);
}

#[test]
fn sidebar_auto_collapse_uses_hysteresis() {
    let preferred = DEFAULT_PANE_WIDTH;
    let collapse_below = sidebar_auto_collapse_below(preferred);
    let expand_above = sidebar_auto_expand_above(preferred);
    // Hold preferred fixed until preferred + workspace min no longer fits (54+48=102).
    assert_eq!(collapse_below, preferred + WORKSPACE_MIN_WIDTH); // 102
    assert!(expand_above > collapse_below);

    // Vertical split ~80 cols: auto-collapse so the agent pane stays usable.
    assert!(sidebar_should_auto_collapse(80, preferred, false));
    assert!(sidebar_should_auto_collapse(80, preferred, true));

    // Just under preferred+workspace: collapse rather than soft-clamp the list.
    assert!(sidebar_should_auto_collapse(100, preferred, false));

    // Comfortable full width stays expanded.
    assert!(!sidebar_should_auto_collapse(160, preferred, false));
    assert!(!sidebar_should_auto_collapse(160, preferred, true));
    // At/above collapse threshold while expanded: stay expanded.
    assert!(!sidebar_should_auto_collapse(
        collapse_below,
        preferred,
        false
    ));

    // Just under expand threshold while collapsed stays collapsed (hysteresis).
    assert!(sidebar_should_auto_collapse(
        collapse_below,
        preferred,
        true
    ));
    assert!(!sidebar_should_auto_collapse(expand_above, preferred, true));
}

#[test]
fn responsive_sidebar_width_collapses_to_rail() {
    let preferred = DEFAULT_PANE_WIDTH;
    assert_eq!(
        responsive_sidebar_width(preferred, 80, true, false),
        COLLAPSED_PANE_WIDTH
    );
    // Peek expand uses a lower workspace floor so the list opens at full default width.
    let peeked = responsive_sidebar_width(preferred, 80, true, true);
    assert_eq!(
        peeked, DEFAULT_PANE_WIDTH,
        "peek on ~80 cols should open at default list width, not the crushed clamp"
    );
    assert!(peeked > 80 - WORKSPACE_MIN_WIDTH); // wider than the old 32-col clamp
                                                // Very tight client: take as much as peek floor allows.
    let tight = responsive_sidebar_width(preferred, 60, true, true);
    assert_eq!(tight, 60 - PEEK_WORKSPACE_MIN); // 36
                                                // Wide client + not collapsed → preferred (fixed, no soft clamp).
    assert_eq!(
        responsive_sidebar_width(preferred, 160, false, false),
        preferred
    );
    // Mid-band client: still report preferred; auto-collapse owns the rail transition.
    assert_eq!(
        responsive_sidebar_width(preferred, 100, false, false),
        preferred
    );
}

#[test]
fn rail_status_items_keep_list_row_positions() {
    let mut working = sample_session("w", 1, 0);
    working.state = AgentState::Working;
    working.last_event_at = Utc::now();

    let done = completed_session("d", 2, 0);

    let mut idle_active = sample_session("a", 3, 0);
    idle_active.state = AgentState::Idle;
    idle_active.is_active = true;

    let mut quiet = sample_session("q", 4, 0);
    quiet.state = AgentState::Idle;

    // Mirror expanded sidebar: group header then sessions (indices 1,2,3,4).
    let rows = vec![
        RowKind::Group {
            label: "~/tmp".into(),
            collapsed: false,
        },
        RowKind::Session { session: working },
        RowKind::Session { session: done },
        RowKind::Session {
            session: idle_active,
        },
        RowKind::Session { session: quiet },
    ];
    let items = rail_status_items(&rows);
    assert_eq!(items.len(), 3, "quiet idle stays off the rail: {items:?}");
    assert_eq!(items[0].list_row, 1);
    assert_eq!(items[0].kind, RailStatusKind::Working);
    assert_eq!(items[1].list_row, 2);
    assert_eq!(items[1].kind, RailStatusKind::Done);
    assert_eq!(items[2].list_row, 3);
    assert_eq!(items[2].kind, RailStatusKind::Active);

    // Same Y as expanded: list_top=8, scroll=0 → working at y=9 (row 1).
    assert_eq!(rail_item_screen_y(1, 0, 8, 20), Some(9));
    assert_eq!(rail_item_screen_y(1, 1, 8, 20), Some(8)); // scrolled
    assert_eq!(rail_item_screen_y(0, 0, 8, 20), Some(8)); // group header slot empty
}

#[test]
fn rail_centered_cell_uses_optical_center() {
    // Width 4 used to be " X  " (left-heavy); prefer "  X " so chips clear the edge.
    assert_eq!(rail_centered_cell('■', 4), "  ■ ");
    assert_eq!(rail_centered_cell('▸', 4), "  ▸ ");
    assert_eq!(rail_centered_cell('·', 3), " · ");
    assert_eq!(rail_centered_cell('x', 1), "x");
    assert_eq!(rail_centered_cell('x', 2), " x");
}

#[test]
fn rail_status_glyphs_are_icon_only_without_highlight_fills() {
    // Collapsed rail must not paint WORKING_BG / selection-style backdrops — only
    // colored status glyphs on the base background.
    for kind in [
        RailStatusKind::Working,
        RailStatusKind::Approval,
        RailStatusKind::Error,
        RailStatusKind::Done,
        RailStatusKind::Active,
    ] {
        let (_ch, style) = rail_status_glyph(kind, 0);
        assert_eq!(
            style.bg,
            Some(BG_BASE),
            "{kind:?} should sit on base, not a status/section fill"
        );
        assert!(style.fg.is_some(), "{kind:?} needs a readable status color");
    }
}

#[test]
fn layout_plan_matches_top_margin_to_horizontal_margin() {
    let rows = build_rows(
        &[sample_session("api", 1, 0)],
        &HashSet::new(),
        &HashSet::new(),
        &[],
    );
    let plan = layout_plan(Size::new(34, 24), &rows);
    assert_eq!(plan.frame_margin_top, plan.frame_margin_h);
}

#[test]
fn layout_margins_stable_across_sidebar_widths() {
    let rows = build_rows(
        &[sample_session("api", 1, 0)],
        &HashSet::new(),
        &HashSet::new(),
        &[],
    );
    let at_default = layout_plan(Size::new(DEFAULT_PANE_WIDTH, 24), &rows);
    let wider = layout_plan(Size::new(80, 24), &rows);
    assert_eq!(at_default.frame_margin_h, wider.frame_margin_h);
    assert_eq!(at_default.frame_margin_top, wider.frame_margin_top);
    assert_eq!(
        at_default.metrics.toolbar_top_y,
        wider.metrics.toolbar_top_y
    );
    assert_eq!(at_default.metrics.list_top_y, wider.metrics.list_top_y);
    assert_eq!(at_default.metrics.list_inner_x, wider.metrics.list_inner_x);
}

#[test]
fn layout_plan_reserves_sessions_title_and_settings() {
    let rows = build_rows(
        &[sample_session("api", 1, 0)],
        &HashSet::new(),
        &HashSet::new(),
        &[],
    );
    let plan = layout_plan(Size::new(34, 24), &rows);
    assert_eq!(
        plan.metrics.toolbar_top_y,
        FRAME_MARGIN_TOP + TOOLBAR_SECTION_PAD
    );
    assert_eq!(plan.metrics.toolbar_row_count, TOOLBAR_BUTTON_ROWS);
    assert_eq!(
        plan.metrics.list_top_y,
        FRAME_MARGIN_TOP + TOOLBAR_SECTION_PAD + TOOLBAR_BUTTON_ROWS + TOOLBAR_SECTION_PAD + 1
    );
    assert!(plan.metrics.list_height >= 1);
    assert!(plan.metrics.list_height < 24);
}

#[test]
fn toolbar_shortcuts_align_with_session_time_column() {
    assert_eq!(format_trailing_slot("⌘+C"), "⌘+C");
    assert_eq!(format_trailing_slot("⌘+S"), "⌘+S");
    assert_eq!(format_trailing_slot("⌘+A"), "⌘+A");
    assert_eq!(format_trailing_slot("⌘+M"), "⌘+M");
    assert_eq!(format_trailing_slot("⌘+K"), "⌘+K");
}

#[test]
fn toolbar_coming_soon_actions_are_search_only() {
    assert!(toolbar_action_coming_soon(ToolbarAction::Search));
    assert!(!toolbar_action_coming_soon(ToolbarAction::Automations));
    assert!(!toolbar_action_coming_soon(ToolbarAction::Mcps));
    assert!(!toolbar_action_coming_soon(ToolbarAction::Skills));
    assert!(!toolbar_action_coming_soon(ToolbarAction::NewSession));
    assert!(!toolbar_action_coming_soon(ToolbarAction::Settings));
    assert!(!toolbar_action_coming_soon(ToolbarAction::Leave));
}

#[test]
fn coming_soon_label_reserves_fixed_width_for_shortcut_alignment() {
    let width = 40;
    let prefix_width = row_prefix(" ", Some("↻")).chars().count();
    let label_width = row_label_width_after_prefix(width, prefix_width);
    for frame in 0..COMING_SOON_CYCLE_FRAMES {
        for base in ["MCPs", "Automations", "Skills", "Search"] {
            let spans = coming_soon_label_spans(base, label_width, frame, Style::default());
            let label_chars: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(
                label_chars, label_width,
                "frame {frame} base {base}: got {label_chars}, want {label_width}"
            );
        }
    }
}

#[test]
fn coming_soon_label_decodes_holds_then_restores_title() {
    let width = 15;
    let glitch = coming_soon_label_text("Search", width, 0);
    let decode_end = COMING_SOON_GLITCH_FRAMES + COMING_SOON_DECODE_FRAMES - 1;
    let hold_start = COMING_SOON_GLITCH_FRAMES + COMING_SOON_DECODE_FRAMES;
    let hold_one_dot = hold_start + COMING_SOON_HOLD_PLAIN_FRAMES;
    let hold_ellipsis =
        hold_start + COMING_SOON_HOLD_PLAIN_FRAMES + COMING_SOON_DOT_STEP_FRAMES * 2;
    let restore_end = COMING_SOON_CYCLE_FRAMES - 1;
    assert_ne!(glitch, coming_soon_label_text("Search", width, decode_end));
    assert_eq!(
        coming_soon_label_text("Search", width, decode_end),
        "Coming soon"
    );
    assert_eq!(
        coming_soon_label_text("Search", width, hold_start),
        "Coming soon"
    );
    assert_eq!(
        coming_soon_label_text("Search", width, hold_one_dot),
        "Coming soon."
    );
    assert_eq!(
        coming_soon_label_text("Search", width, hold_ellipsis),
        "Coming soon..."
    );
    assert_eq!(
        coming_soon_label_text("Search", width, restore_end),
        "Search"
    );
    assert_eq!(
        coming_soon_label_text("Automations", width, restore_end),
        "Automations"
    );
}

#[test]
fn decode_biases_toward_first_letters() {
    let width = 15;
    let decode_start = COMING_SOON_GLITCH_FRAMES;
    let early = coming_soon_label_text("Search", width, decode_start + 1);
    assert!(early.starts_with('C'));
    let mid = coming_soon_label_text("Search", width, decode_start + 3);
    assert!(mid.starts_with("Com") || mid.starts_with("Co"));
}

#[test]
fn coming_soon_cycle_runs_glitch_decode_hold_and_restore() {
    assert_eq!(COMING_SOON_CYCLE_FRAMES, 45);
    assert_eq!(COMING_SOON_CYCLE_MS, 45 * COMING_SOON_INTERVAL_MS);
    assert!(COMING_SOON_HOLD_FRAMES as u64 * COMING_SOON_INTERVAL_MS >= 2000);
    let decode_end = COMING_SOON_GLITCH_FRAMES + COMING_SOON_DECODE_FRAMES - 1;
    assert_eq!(
        coming_soon_label_text("MCPs", 15, decode_end),
        "Coming soon"
    );
}

#[test]
fn row_prefix_and_trailing_reserve_shared_width() {
    assert_eq!(row_prefix(" ", None).chars().count(), ROW_LABEL_OFFSET);
    assert_eq!(row_prefix(" ", Some("+")).chars().count(), ROW_LABEL_OFFSET);
    assert_eq!(row_prefix("▎", None).chars().count(), ROW_LABEL_OFFSET);
    assert_eq!(
        row_label_width(40),
        40 - ROW_LABEL_OFFSET - ROW_PRE_TRAILING_GAP - TRAILING_SLOT_WIDTH
    );
}

#[test]
fn chrome_row_prefix_matches_sessions_title_inset() {
    assert_eq!(chrome_row_prefix(), " ");
    assert_eq!(
        row_label_width_after_prefix(40, chrome_row_prefix().chars().count()),
        40 - 1 - ROW_PRE_TRAILING_GAP - TRAILING_SLOT_WIDTH
    );
}

#[test]
fn notepad_note_title_prefix_aligns_with_notes_header_label() {
    let header = notepad_header_label(true);
    let n_in_header = chrome_row_prefix().chars().count()
        + header
            .chars()
            .position(|c| c == 'n')
            .expect("notes header contains n");
    assert_eq!(
        notepad_note_title_prefix(false, false).chars().count(),
        n_in_header
    );
    assert_eq!(
        notepad_note_title_prefix(true, false).chars().count(),
        n_in_header
    );
    assert_eq!(
        notepad_note_title_prefix(false, true).chars().count(),
        n_in_header
    );
    assert_eq!(
        notepad_note_title_prefix(false, true)
            .chars()
            .nth(1)
            .unwrap(),
        '✕'
    );
}

#[test]
fn notepad_note_title_drag_prefix_preserves_title_offset() {
    use super::notepad::{notepad_note_title_prefix_drag, NOTEPAD_NOTE_TITLE_OFFSET};
    use super::sessions::GroupHighlight;
    let base = notepad_note_title_prefix(false, false);
    assert_eq!(base.chars().count(), NOTEPAD_NOTE_TITLE_OFFSET);
    assert_eq!(
        notepad_note_title_prefix_drag(false, false, Some(GroupHighlight::Source))
            .chars()
            .count(),
        NOTEPAD_NOTE_TITLE_OFFSET
    );
    assert_eq!(
        notepad_note_title_prefix_drag(false, false, Some(GroupHighlight::Target))
            .chars()
            .count(),
        NOTEPAD_NOTE_TITLE_OFFSET
    );
    assert_eq!(
        notepad_note_title_prefix_drag(false, false, None)
            .chars()
            .count(),
        NOTEPAD_NOTE_TITLE_OFFSET
    );
}

#[test]
fn notepad_note_title_close_mode_mutes_non_target_rows() {
    use super::notepad::notepad_note_title_row_style_in_list;
    use super::theme::{BG_BASE, CLOSE_HOVER_BG, CLOSE_HOVER_FG, CLOSE_MODE_FG};
    let muted = notepad_note_title_row_style_in_list(false, true, true, false, true);
    assert_eq!(muted.fg, Some(CLOSE_MODE_FG));
    assert_eq!(muted.bg, Some(BG_BASE));
    assert!(!muted.add_modifier.contains(Modifier::BOLD));

    let target = notepad_note_title_row_style_in_list(false, false, false, true, true);
    assert_eq!(target.fg, Some(CLOSE_HOVER_FG));
    assert_eq!(target.bg, Some(CLOSE_HOVER_BG));
    assert!(target.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn notepad_note_title_row_style_matches_backdrop_when_active_and_hovered() {
    assert_eq!(
        notepad_note_title_row_bg(false, true, true),
        Some(BG_HOVER_SELECTED)
    );
    assert_eq!(
        notepad_note_title_row_style(false, true, true).bg,
        Some(BG_HOVER_SELECTED)
    );
}

#[test]
fn toolbar_action_from_mouse_maps_rows() {
    let rows = build_rows(
        &[sample_session("api", 1, 0)],
        &HashSet::new(),
        &HashSet::new(),
        &[],
    );
    let metrics = layout_plan(Size::new(34, 24), &rows).metrics;
    assert_eq!(
        toolbar_action_from_mouse(metrics.toolbar_top_y, &metrics),
        Some(ToolbarAction::NewSession)
    );
    assert_eq!(
        toolbar_action_from_mouse(metrics.toolbar_top_y + 1, &metrics),
        Some(ToolbarAction::Search)
    );
    assert_eq!(
        toolbar_action_from_mouse(metrics.toolbar_top_y + 2, &metrics),
        Some(ToolbarAction::Automations)
    );
    assert_eq!(
        toolbar_action_from_mouse(metrics.toolbar_top_y + 3, &metrics),
        Some(ToolbarAction::Mcps)
    );
    assert_eq!(
        toolbar_action_from_mouse(metrics.toolbar_top_y + 4, &metrics),
        Some(ToolbarAction::Skills)
    );
    assert_eq!(
        toolbar_action_from_mouse(metrics.toolbar_top_y + 5, &metrics),
        None
    );
    assert_eq!(
        toolbar_action_from_mouse(metrics.list_top_y, &metrics),
        None
    );
}

#[test]
fn settings_action_from_mouse_maps_settings_row_only() {
    let rows = build_rows(
        &[sample_session("api", 1, 0)],
        &HashSet::new(),
        &HashSet::new(),
        &[],
    );
    let metrics = layout_plan(Size::new(34, 24), &rows).metrics;
    assert_eq!(metrics.settings_row_count, SETTINGS_BUTTON_ROWS);
    assert_eq!(metrics.leave_row_count, LEAVE_BUTTON_ROWS);
    assert!(settings_action_from_mouse(metrics.settings_top_y, &metrics));
    assert!(!settings_action_from_mouse(metrics.leave_top_y, &metrics));
    assert!(!settings_action_from_mouse(
        metrics.settings_top_y - 1,
        &metrics
    ));
}

#[test]
fn leave_action_from_mouse_maps_row_below_settings() {
    let rows = build_rows(
        &[sample_session("api", 1, 0)],
        &HashSet::new(),
        &HashSet::new(),
        &[],
    );
    let metrics = layout_plan(Size::new(34, 24), &rows).metrics;
    assert!(leave_action_from_mouse(metrics.leave_top_y, &metrics));
    assert!(!leave_action_from_mouse(metrics.settings_top_y, &metrics));
    assert!(!leave_action_from_mouse(metrics.leave_top_y - 1, &metrics));
}

#[test]
fn layout_plan_shrinks_on_short_panes() {
    let rows = build_rows(
        &[sample_session("api", 1, 0)],
        &HashSet::new(),
        &HashSet::new(),
        &[],
    );
    let plan = layout_plan(Size::new(20, 10), &rows);
    assert!(plan.metrics.list_height >= 1);
    assert!(plan.metrics.list_height < 5);
}

fn sample_session_in(label: &str, description: &str, tab_index: u32) -> Session {
    let mut session = sample_session(description, tab_index, 0);
    session.cwd_label = label.into();
    session
}

#[test]
fn group_sections_include_sessions_and_toggle_rows() {
    let sessions = vec![
        sample_session_in("~/a", "one", 1),
        sample_session_in("~/a", "two", 2),
        sample_session_in("~/b", "three", 3),
    ];
    let rows = build_rows(
        &sessions,
        &HashSet::new(),
        &HashSet::new(),
        &["~/a".into(), "~/b".into()],
    );
    let sections = group_sections(&rows);
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].label, "~/a");
    assert_eq!(sections[0].end - sections[0].start + 1, 3);
    assert_eq!(sections[1].label, "~/b");
    assert_eq!(sections[1].end - sections[1].start + 1, 2);
}

#[test]
fn group_drag_target_uses_full_section_height_before_swapping_down() {
    let sessions = vec![
        sample_session_in("~/a", "one", 1),
        sample_session_in("~/a", "two", 2),
        sample_session_in("~/a", "three", 3),
        sample_session_in("~/b", "four", 4),
    ];
    let rows = build_rows(
        &sessions,
        &HashSet::new(),
        &HashSet::new(),
        &["~/a".into(), "~/b".into()],
    );
    let sections = group_sections(&rows);
    let b = &sections[1];

    assert_eq!(
        group_drag_target(&rows, b.start, "~/a"),
        Some("~/b".to_string())
    );
    assert_eq!(
        group_drag_target(&rows, b.end, "~/a"),
        Some("~/b".to_string())
    );
}

#[test]
fn group_drag_target_uses_full_section_height_before_swapping_up() {
    let sessions = vec![
        sample_session_in("~/a", "one", 1),
        sample_session_in("~/b", "two", 2),
        sample_session_in("~/b", "three", 3),
        sample_session_in("~/b", "four", 4),
    ];
    let rows = build_rows(
        &sessions,
        &HashSet::new(),
        &HashSet::new(),
        &["~/a".into(), "~/b".into()],
    );
    let sections = group_sections(&rows);
    let a = &sections[0];
    let b = &sections[1];

    assert_eq!(
        group_drag_target(&rows, b.end, "~/b"),
        Some("~/b".to_string())
    );
    assert_eq!(
        group_drag_target(&rows, a.end, "~/b"),
        Some("~/b".to_string())
    );
    assert_eq!(
        group_drag_target(&rows, a.start, "~/b"),
        Some("~/a".to_string())
    );
}

#[test]
fn note_drag_target_maps_body_rows_to_owning_note() {
    use super::notepad::{note_drag_target, note_sections};

    let notes = vec![
        Note::new("A", "line one\nline two", true),
        Note::new("B", "", false),
    ];
    let state = notepad_list_state(&notes, true, true, Some(0));
    let sections = note_sections(&state);
    let id_a = notes[0].id.clone();
    let body_row = sections[0].start + 2;
    assert_eq!(note_drag_target(&state, body_row, &id_a), Some(id_a));
}

#[test]
fn note_drag_source_keeps_title_backdrop_while_pending_and_dragging() {
    let sessions = vec![sample_session("api", 1, 0)];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &[]);
    let notes = sample_expanded_notes("draft");
    let active_state = notepad_list_state(&notes, true, true, Some(0));
    let line_width = default_sidebar_line_width();
    let sections = group_sections(&rows);
    let title_row = notepad_note_title_row_index(0, rows.len(), &active_state).unwrap();
    let pending = NoteDragState {
        pending_click_note_id: Some(notes[0].id.clone()),
        ..Default::default()
    };
    assert_eq!(
        list_row_backdrop_bg(
            title_row,
            rows.len(),
            &rows,
            0,
            false,
            None,
            None,
            None,
            false,
            &active_state,
            line_width,
            Some(0),
            &GroupDragState::default(),
            &sections,
            &pending,
            &note_sections(&active_state),
            None,
        ),
        Some(BG_HOVER_SELECTED)
    );
    let dragging = NoteDragState {
        source: Some(notes[0].id.clone()),
        hover: Some(notes[0].id.clone()),
        dragged: false,
        ..Default::default()
    };
    assert_eq!(
        list_row_backdrop_bg(
            title_row,
            rows.len(),
            &rows,
            0,
            false,
            None,
            None,
            None,
            false,
            &active_state,
            line_width,
            Some(0),
            &GroupDragState::default(),
            &sections,
            &dragging,
            &note_sections(&active_state),
            None,
        ),
        Some(BG_HOVER_SELECTED)
    );
}

#[test]
fn note_drag_only_highlights_source_not_neighbors() {
    use super::notepad::{note_section_highlight, note_sections};
    use super::sessions::GroupHighlight;

    let notes: Vec<Note> = (1..=3)
        .map(|n| Note::new(format!("Note {n}"), String::new(), false))
        .collect();
    let state = notepad_list_state(&notes, true, true, Some(0));
    let sections = note_sections(&state);
    let drag = NoteDragState {
        source: Some(notes[0].id.clone()),
        hover: Some(notes[2].id.clone()),
        dragged: true,
        ..Default::default()
    };
    assert_eq!(
        note_section_highlight(&sections, sections[0].start, &drag),
        Some(GroupHighlight::Source)
    );
    assert_eq!(
        note_section_highlight(&sections, sections[1].start, &drag),
        None
    );
    assert_eq!(
        note_section_highlight(&sections, sections[2].start, &drag),
        None
    );
}

#[test]
fn note_drag_target_swaps_when_dragging_down_past_lower_half() {
    use super::notepad::{note_drag_target, note_sections};

    let notes: Vec<Note> = (1..=3)
        .map(|n| Note::new(format!("Note {n}"), String::new(), false))
        .collect();
    let state = notepad_list_state(&notes, true, true, Some(0));
    let sections = note_sections(&state);
    let source_id = notes[0].id.clone();
    let target = &sections[2];
    assert_eq!(
        note_drag_target(&state, target.start, &source_id),
        Some(notes[2].id.clone())
    );
}

#[test]
fn group_add_icon_fills_trailing_slot() {
    assert_eq!(format_trailing_slot(GROUP_ADD_ICON), "[+]");
    assert_eq!(GROUP_GROK_ICON.chars().count(), TRAILING_SLOT_WIDTH);
    assert_eq!(GROUP_OPENCODE_ICON.chars().count(), TRAILING_SLOT_WIDTH);
    assert_eq!(GROUP_ADD_ICON.chars().count(), TRAILING_SLOT_WIDTH);
    assert_eq!(group_launch_trailing_width(3), TRAILING_SLOT_WIDTH * 3);
    assert_eq!(group_launch_trailing_width(4), TRAILING_SLOT_WIDTH * 4);
    assert_eq!(group_launch_trailing_width(0), 0);
}

#[test]
fn notepad_body_visible_rect_tracks_scroll_window() {
    let terminal = Rect::new(1, 4, 40, 18);
    let notes = sample_expanded_notes("");
    let state = sample_notepad_state(&notes);
    let (body_start, body_end) = notepad_note_body_row_range(0, 3, &state).unwrap();
    let rect = notepad_note_body_visible_rect(terminal, 0, 18, 3, &state, 0)
        .expect("notepad body should be visible");
    assert_eq!(rect.x, 1);
    assert_eq!(rect.y, terminal.y + (body_start as u16));
    assert_eq!(rect.width, 40);
    assert!(rect.height > 0);
    assert!(rect.height <= (body_end - body_start) as u16);
}

#[test]
fn notepad_scrollbar_geometry_sizes_thumb_to_viewport_ratio() {
    let terminal = Rect::new(1, 4, 40, 18);
    let notes = sample_expanded_notes(&"one\n".repeat(30));
    let state = sample_notepad_state(&notes);
    let text = "one\n".repeat(30);
    let (_, body_end) = notepad_note_body_row_range(0, 3, &state).unwrap();
    let (body_start, _) = notepad_note_body_row_range(0, 3, &state).unwrap();
    let scrollbar = notepad_scrollbar_geometry(terminal, 0, 18, 3, &state, 0, &text, 0, 40)
        .expect("scrollbar should appear when content overflows");
    assert_eq!(scrollbar.track.x, 42);
    assert!(scrollbar.track.height > 0);
    assert!(scrollbar.track.height <= (body_end - body_start) as u16);
    assert!(scrollbar.thumb.height >= 1);
    assert!(scrollbar.thumb.height < scrollbar.track.height);
}

#[test]
fn notepad_scroll_from_track_click_maps_click_to_scroll_offset() {
    let scrollbar = NotepadScrollbar {
        track: Rect::new(10, 5, 1, 10),
        thumb: Rect::new(10, 5, 1, 2),
    };
    assert_eq!(
        notepad_scroll_from_track_click(5, &scrollbar, 8),
        usize::MAX
    );
    assert_eq!(notepad_scroll_from_track_click(14, &scrollbar, 8), 8);
    assert_eq!(notepad_scroll_from_track_click(7, &scrollbar, 8), 1);
}

#[test]
fn notepad_scroll_from_thumb_drag_tracks_pointer_with_grab_offset() {
    let scrollbar = NotepadScrollbar {
        track: Rect::new(10, 5, 1, 10),
        thumb: Rect::new(10, 5, 1, 2),
    };
    assert_eq!(notepad_scroll_from_thumb_drag(5, &scrollbar, 8, 0), 0);
    assert_eq!(notepad_scroll_from_thumb_drag(14, &scrollbar, 8, 0), 8);
    assert_eq!(notepad_scroll_from_thumb_drag(13, &scrollbar, 8, 1), 7);
}

#[test]
fn group_add_click_detects_trailing_icon_on_group_row() {
    let sessions = vec![sample_session_in("~/a", "one", 1)];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &["~/a".into()]);
    let metrics = layout_plan(Size::new(34, 24), &rows).metrics;
    let group_row = rows
        .iter()
        .position(|row| matches!(row, RowKind::Group { .. }))
        .unwrap();
    let rel = group_row.saturating_sub(0);
    let row_y = metrics.list_top_y + rel as u16;
    let agents = default_group_launch_agents();
    let trailing_col = metrics.list_inner_x
        + metrics
            .list_line_width
            .saturating_sub(group_launch_click_width(agents.len())) as u16;
    let inside_slot = trailing_col + ROW_PRE_TRAILING_GAP as u16;
    assert_eq!(
        group_launch_click(inside_slot, row_y, &metrics, 0, rows.len(), &rows, &agents)
            .map(|(l, _)| l),
        Some("~/a")
    );
    assert_eq!(
        group_launch_click(trailing_col, row_y, &metrics, 0, rows.len(), &rows, &agents)
            .map(|(l, _)| l),
        Some("~/a")
    );
    assert!(group_launch_click(
        trailing_col.saturating_sub(1),
        row_y,
        &metrics,
        0,
        rows.len(),
        &rows,
        &agents
    )
    .is_none());
}

#[test]
fn group_launch_click_maps_g_o_plus_badges() {
    let sessions = vec![sample_session_in("~/a", "one", 1)];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &["~/a".into()]);
    let metrics = layout_plan(Size::new(48, 24), &rows).metrics;
    let group_row = rows
        .iter()
        .position(|row| matches!(row, RowKind::Group { .. }))
        .unwrap();
    let row_y = metrics.list_top_y + group_row as u16;
    let agents = default_group_launch_agents();
    let trail_start = metrics.list_inner_x
        + metrics
            .list_line_width
            .saturating_sub(group_launch_trailing_width(agents.len())) as u16;
    let grok_col = trail_start;
    let opencode_col = trail_start + GROUP_LAUNCH_BUTTON_WIDTH as u16;
    let console_col = trail_start + (GROUP_LAUNCH_BUTTON_WIDTH * 2) as u16;
    assert_eq!(
        group_launch_click(grok_col, row_y, &metrics, 0, rows.len(), &rows, &agents),
        Some(("~/a", "grok".into()))
    );
    assert_eq!(
        group_launch_click(opencode_col, row_y, &metrics, 0, rows.len(), &rows, &agents),
        Some(("~/a", "opencode".into()))
    );
    assert_eq!(
        group_launch_click(console_col, row_y, &metrics, 0, rows.len(), &rows, &agents),
        Some(("~/a", "console".into()))
    );
}

#[test]
fn group_launch_click_supports_four_badges() {
    let sessions = vec![sample_session_in("~/a", "one", 1)];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &["~/a".into()]);
    let metrics = layout_plan(Size::new(56, 24), &rows).metrics;
    let group_row = rows
        .iter()
        .position(|row| matches!(row, RowKind::Group { .. }))
        .unwrap();
    let row_y = metrics.list_top_y + group_row as u16;
    let agents = vec![
        "grok".into(),
        "codex".into(),
        "claude".into(),
        "opencode".into(),
    ];
    let trail_start = metrics.list_inner_x
        + metrics
            .list_line_width
            .saturating_sub(group_launch_trailing_width(agents.len())) as u16;
    for (i, agent) in agents.iter().enumerate() {
        let col = trail_start + (GROUP_LAUNCH_BUTTON_WIDTH * i) as u16;
        assert_eq!(
            group_launch_click(col, row_y, &metrics, 0, rows.len(), &rows, &agents),
            Some(("~/a", agent.clone()))
        );
    }
}

#[test]
fn group_header_line_shows_agent_badges_on_hover() {
    let style = Style::default().fg(TEXT_SECONDARY).bg(BG_BASE);
    let agents = default_group_launch_agents();
    let idle = group_header_line("~/proj", false, None, false, 40, style, &agents);
    let idle_text: String = idle.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(!idle_text.contains(GROUP_GROK_ICON));
    assert!(!idle_text.contains(GROUP_OPENCODE_ICON));
    assert!(!idle_text.contains(GROUP_ADD_ICON));

    let hovered = group_header_line("~/proj", false, None, true, 40, style, &agents);
    let text: String = hovered.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains(GROUP_GROK_ICON));
    assert!(text.contains(GROUP_OPENCODE_ICON));
    assert!(text.contains(GROUP_ADD_ICON));
    let g = text.find(GROUP_GROK_ICON).unwrap();
    let o = text.find(GROUP_OPENCODE_ICON).unwrap();
    let plus = text.find(GROUP_ADD_ICON).unwrap();
    assert!(g < o && o < plus);
}

#[test]
fn group_header_line_respects_custom_agents() {
    let style = Style::default().fg(TEXT_SECONDARY).bg(BG_BASE);
    let agents = vec!["claude".into(), "codex".into()];
    let hovered = group_header_line("~/proj", false, None, true, 40, style, &agents);
    let text: String = hovered.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains(GROUP_CLAUDE_ICON));
    assert!(text.contains(GROUP_CODEX_ICON));
    assert!(!text.contains(GROUP_GROK_ICON));
    assert!(!text.contains(GROUP_ADD_ICON));
}

#[test]
fn group_row_from_mouse_maps_only_group_headers() {
    let sessions = vec![
        sample_session_in("~/a", "one", 1),
        sample_session_in("~/a", "two", 2),
    ];
    let rows = build_rows(&sessions, &HashSet::new(), &HashSet::new(), &["~/a".into()]);
    let metrics = layout_plan(Size::new(34, 24), &rows).metrics;
    let group_row = rows
        .iter()
        .position(|row| matches!(row, RowKind::Group { .. }))
        .unwrap();
    assert_eq!(
        group_row_from_mouse(
            metrics.list_top_y + group_row as u16,
            &metrics,
            0,
            rows.len(),
            &rows
        ),
        Some(group_row)
    );
    let session_row = rows
        .iter()
        .position(|row| matches!(row, RowKind::Session { .. }))
        .unwrap();
    assert_eq!(
        group_row_from_mouse(
            metrics.list_top_y + session_row as u16,
            &metrics,
            0,
            rows.len(),
            &rows
        ),
        None
    );
}

#[test]
fn group_drag_target_maps_session_rows_to_owning_group() {
    let sessions = vec![
        sample_session_in("~/a", "one", 1),
        sample_session_in("~/a", "two", 2),
        sample_session_in("~/b", "three", 3),
    ];
    let rows = build_rows(
        &sessions,
        &HashSet::new(),
        &HashSet::new(),
        &["~/a".into(), "~/b".into()],
    );
    let sections = group_sections(&rows);
    let session_row = sections[0].start + 1;

    assert_eq!(
        group_drag_target(&rows, session_row, "~/a"),
        Some("~/a".to_string())
    );
}

#[test]
fn group_drag_target_stays_stable_when_preview_reorders_rows() {
    let sessions = vec![
        sample_session_in("~/a", "one", 1),
        sample_session_in("~/a", "two", 2),
        sample_session_in("~/a", "three", 3),
        sample_session_in("~/b", "four", 4),
    ];
    let base_rows = build_rows(
        &sessions,
        &HashSet::new(),
        &HashSet::new(),
        &["~/a".into(), "~/b".into()],
    );
    let preview_rows = build_rows(
        &sessions,
        &HashSet::new(),
        &HashSet::new(),
        &["~/b".into(), "~/a".into()],
    );
    let b_end = group_sections(&base_rows)[1].end;

    assert_eq!(
        group_drag_target(&base_rows, b_end, "~/a"),
        Some("~/b".to_string())
    );
    // Preview layout moves ~/a under the same screen row — hit-testing it flips back to source.
    assert_eq!(
        group_drag_target(&preview_rows, b_end, "~/a"),
        Some("~/a".to_string())
    );
}

#[test]
fn rename_starts_with_select_all_and_first_char_replaces_title() {
    let mut rename = RenameState {
        target: RenameTarget::Session {
            session_id: "tmux:win:1".into(),
        },
        row_idx: 0,
        buffer: "old title".into(),
        select_all: true,
    };
    rename_apply_char(&mut rename, 'n');
    assert_eq!(rename.buffer, "n");
    assert!(!rename.select_all);
}

#[test]
fn rename_backspace_on_select_all_clears_title() {
    let mut rename = RenameState {
        target: RenameTarget::Session {
            session_id: "tmux:win:1".into(),
        },
        row_idx: 0,
        buffer: "old title".into(),
        select_all: true,
    };
    rename_apply_backspace(&mut rename);
    assert!(rename.buffer.is_empty());
    assert!(!rename.select_all);
}

#[test]
fn context_menu_items_include_rename_for_sessions_only() {
    let session = ContextMenuTarget::Session {
        session_id: "tmux:win:1".into(),
    };
    let group = ContextMenuTarget::Group {
        cwd_label: "~/projects".into(),
    };
    let note = ContextMenuTarget::Note {
        note_id: "note-1".into(),
    };
    assert_eq!(
        context_menu_items(&session),
        &[ContextMenuAction::Rename, ContextMenuAction::Delete]
    );
    assert_eq!(context_menu_items(&group), &[ContextMenuAction::Delete]);
    assert_eq!(
        context_menu_items(&note),
        &[ContextMenuAction::Rename, ContextMenuAction::Delete]
    );
}

#[test]
fn context_menu_action_at_maps_rows_to_actions() {
    let menu = ContextMenu {
        target: ContextMenuTarget::Session {
            session_id: "tmux:win:1".into(),
        },
        x: 2,
        y: 3,
        hover: None,
    };
    let area = Rect::new(0, 0, 40, 20);
    assert_eq!(
        context_menu_action_at(&menu, 2, 3, area),
        Some(ContextMenuAction::Rename)
    );
    assert_eq!(
        context_menu_action_at(&menu, 2, 4, area),
        Some(ContextMenuAction::Delete)
    );
    assert_eq!(context_menu_action_at(&menu, 2, 5, area), None);
}

#[test]
fn context_menu_labels_differ_for_session_and_group() {
    let session = ContextMenuTarget::Session {
        session_id: "tmux:win:1".into(),
    };
    let group = ContextMenuTarget::Group {
        cwd_label: "~/projects".into(),
    };
    assert_eq!(
        context_menu_label(&session, ContextMenuAction::Delete),
        " End session "
    );
    assert_eq!(
        context_menu_label(&group, ContextMenuAction::Delete),
        " End all sessions "
    );
}

#[test]
fn notepad_context_menu_disables_cut_and_copy_without_selection() {
    let target = ContextMenuTarget::Notepad {
        has_selection: false,
    };
    assert!(!context_menu_item_enabled(&target, ContextMenuAction::Cut));
    assert!(!context_menu_item_enabled(&target, ContextMenuAction::Copy));
    assert!(context_menu_item_enabled(&target, ContextMenuAction::Paste));
}

#[test]
fn notepad_context_menu_action_at_respects_border_and_disabled_rows() {
    let menu = ContextMenu {
        target: ContextMenuTarget::Notepad {
            has_selection: false,
        },
        x: 4,
        y: 6,
        hover: None,
    };
    let area = Rect::new(0, 0, 40, 20);
    assert_eq!(context_menu_action_at(&menu, 4, 6, area), None);
    assert_eq!(context_menu_action_at(&menu, 4, 7, area), None);
    assert_eq!(context_menu_action_at(&menu, 4, 8, area), None);
    assert_eq!(
        context_menu_action_at(&menu, 4, 9, area),
        Some(ContextMenuAction::Paste)
    );
    assert_eq!(
        context_menu_action_at(&menu, 4, 10, area),
        Some(ContextMenuAction::SelectAll)
    );
}

#[test]
fn notepad_task_list_renders_solid_green_block_not_emoji() {
    let spans = notepad_body_line_spans(
        "- [x] Listing Manager",
        0,
        40,
        NOTEPAD_EDIT_FG,
        NOTEPAD_EDIT_BG,
        NOTEPAD_SELECT_FG,
        NOTEPAD_SELECT_BG,
        None,
        None,
    );
    let rendered: String = spans.iter().map(|span| span.content.as_ref()).collect();
    for ch in ['⬜', '✅', '☐', '☑'] {
        assert!(
            !rendered.contains(ch),
            "unexpected glyph {ch:?} in {rendered:?}"
        );
    }
    assert!(
        !rendered.contains("- ["),
        "markdown brackets should be hidden: {rendered:?}"
    );
    assert!(
        rendered.contains('█'),
        "checked task should render solid block: {rendered:?}"
    );

    let unchecked = notepad_body_line_spans(
        "- [ ] Saved Items",
        0,
        40,
        NOTEPAD_EDIT_FG,
        NOTEPAD_EDIT_BG,
        NOTEPAD_SELECT_FG,
        NOTEPAD_SELECT_BG,
        None,
        None,
    );
    let unchecked_text: String = unchecked.iter().map(|span| span.content.as_ref()).collect();
    assert!(!unchecked_text.contains("- ["));
    assert!(!unchecked_text.contains('⬜'));
}
