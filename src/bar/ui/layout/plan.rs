use crate::bar::notepad::Note;
use ratatui::layout::{Constraint, Direction, Layout, Rect, Size};
use ratatui::widgets::block::Padding;
use ratatui::widgets::Block;
use std::collections::HashSet;

use super::metrics::{LayoutMetrics, LayoutPlan};
use super::trail::{
    default_sidebar_line_width, notepad_header_row_index, notepad_list_state,
    visible_session_rows, NOTEPAD_BODY_PAD_TOP, NOTEPAD_BODY_ROWS,
};
use super::super::sessions::RowKind;

pub(crate) const SESSION_TITLE_ROWS: u16 = 1;
pub(crate) const SESSION_BLOCK_ROWS: u16 = SESSION_TITLE_ROWS;
pub(crate) const TOOLBAR_BUTTON_ROWS: u16 = 5;
pub(crate) const TOOLBAR_SECTION_PAD: u16 = 1;
pub(crate) const TOOLBAR_SECTION_ROWS: u16 = TOOLBAR_BUTTON_ROWS + TOOLBAR_SECTION_PAD * 2;
pub(crate) const SETTINGS_BUTTON_ROWS: u16 = 1;
pub(crate) const LEAVE_BUTTON_ROWS: u16 = 1;
pub(crate) const BOTTOM_CHROME_ROWS: u16 = SETTINGS_BUTTON_ROWS + LEAVE_BUTTON_ROWS;
const UPDATE_MESSAGE_ROWS: u16 = 1;
const UPDATE_ACTION_ROWS: u16 = 2;
pub(crate) const UPDATE_BOX_ROWS: u16 = UPDATE_MESSAGE_ROWS + UPDATE_ACTION_ROWS;

pub(crate) fn settings_section_rows(show_update_banner: bool) -> u16 {
    let update_rows = if show_update_banner { UPDATE_BOX_ROWS } else { 0 };
    update_rows + BOTTOM_CHROME_ROWS + TOOLBAR_SECTION_PAD * 2
}

const NOTEPAD_HEADER_ROWS: u16 = 1;

pub const MIN_PANE_WIDTH: u16 = 22;
pub const DEFAULT_PANE_WIDTH: u16 = 55;
/// Fixed inset — must not vary with pane width or sidebar resize shifts content.
pub(crate) const FRAME_MARGIN_H: u16 = 0;
pub(crate) const FRAME_MARGIN_TOP: u16 = 0;
/// Inset inside the frame margin — right is thicker so shortcuts align with left icon inset.
pub const SESSION_BLOCK_PAD_LEFT: u16 = 0;
pub const SESSION_BLOCK_PAD_RIGHT: u16 = 2;

pub fn layout_plan(size: Size, rows: &[RowKind]) -> LayoutPlan {
    layout_plan_for_rect(
        Rect::new(0, 0, size.width.max(1), size.height),
        rows,
        true,
        &[],
        false,
        false,
    )
}

pub fn layout_plan_with_notepad(
    size: Size,
    rows: &[RowKind],
    sessions_expanded: bool,
    notes: &[Note],
    notepad_expanded: bool,
    show_update_banner: bool,
) -> LayoutPlan {
    layout_plan_for_rect(
        Rect::new(0, 0, size.width.max(1), size.height),
        rows,
        sessions_expanded,
        notes,
        notepad_expanded,
        show_update_banner,
    )
}

pub fn layout_metrics(size: Size, rows: &[RowKind]) -> LayoutMetrics {
    layout_plan(size, rows).metrics
}

pub fn layout_metrics_with_notepad(
    size: Size,
    rows: &[RowKind],
    sessions_expanded: bool,
    notes: &[Note],
    notepad_expanded: bool,
    show_update_banner: bool,
) -> LayoutMetrics {
    layout_plan_with_notepad(
        size,
        rows,
        sessions_expanded,
        notes,
        notepad_expanded,
        show_update_banner,
    )
    .metrics
}

pub fn desired_pane_width(_rows: &[RowKind], _session_count: usize, _digit_buffer: &str) -> u16 {
    DEFAULT_PANE_WIDTH
}

fn layout_plan_for_rect(
    area: Rect,
    rows: &[RowKind],
    sessions_expanded: bool,
    notes: &[Note],
    notepad_expanded: bool,
    show_update_banner: bool,
) -> LayoutPlan {
    let frame_margin_h = FRAME_MARGIN_H;
    let frame_margin_top = FRAME_MARGIN_TOP;
    let bottom_section_rows = settings_section_rows(show_update_banner);
    let update_rows = if show_update_banner { UPDATE_BOX_ROWS } else { 0 };
    let content_h = area.height.saturating_sub(frame_margin_top);
    let sessions_outer_h = content_h
        .saturating_sub(TOOLBAR_SECTION_ROWS)
        .saturating_sub(bottom_section_rows);
    let list_height = sessions_outer_h.saturating_sub(SESSION_BLOCK_ROWS).max(1) as usize;
    let content_top = area.y.saturating_add(frame_margin_top);
    let toolbar_top_y = content_top.saturating_add(TOOLBAR_SECTION_PAD);
    let list_top_y = toolbar_top_y
        .saturating_add(TOOLBAR_BUTTON_ROWS)
        .saturating_add(TOOLBAR_SECTION_PAD)
        .saturating_add(1);
    let visible_sessions = visible_session_rows(rows.len(), sessions_expanded);
    let notepad_state = notepad_list_state(notes, notepad_expanded, false, None);
    let line_width = default_sidebar_line_width();
    let header_row = notepad_header_row_index(
        visible_sessions,
        sessions_expanded,
        &notepad_state,
    );
    let sessions_title_y = list_top_y.saturating_sub(1);
    let notepad_header_y =
        list_top_y.saturating_add(header_row.min(list_height.saturating_sub(1)) as u16);
    let notepad_body_top_y = notepad_header_y.saturating_add(NOTEPAD_HEADER_ROWS);
    let notepad_top_y = notepad_header_y.saturating_sub(TOOLBAR_SECTION_PAD);
    let notepad_body_rows = if notepad_expanded {
        notes
            .iter()
            .filter(|note| note.expanded)
            .map(|_| NOTEPAD_BODY_ROWS.saturating_add(NOTEPAD_BODY_PAD_TOP))
            .sum()
    } else {
        0
    };
    let settings_section_top = content_top
        .saturating_add(TOOLBAR_SECTION_ROWS)
        .saturating_add(sessions_outer_h);
    let update_banner_top_y = settings_section_top.saturating_add(TOOLBAR_SECTION_PAD);
    let settings_top_y = update_banner_top_y.saturating_add(update_rows);
    let leave_top_y = settings_top_y.saturating_add(SETTINGS_BUTTON_ROWS);

    let plan = LayoutPlan {
        settings_section_rows: bottom_section_rows,
        metrics: LayoutMetrics {
            frame_width: area.width,
            frame_height: area.height,
            list_height,
            list_top_y,
            list_inner_x: 0,
            list_line_width: 0,
            toolbar_top_y,
            toolbar_row_count: TOOLBAR_BUTTON_ROWS,
            update_banner_top_y,
            update_banner_row_count: update_rows,
            settings_top_y,
            settings_row_count: SETTINGS_BUTTON_ROWS,
            leave_top_y,
            leave_row_count: LEAVE_BUTTON_ROWS,
            notepad_top_y,
            notepad_header_y,
            notepad_body_top_y,
            notepad_body_rows,
            notepad_expanded,
            sessions_title_y,
        },
        frame_margin_top,
        frame_margin_h,
    };
    let list_inner = sessions_list_inner(area, &plan);

    LayoutPlan {
        metrics: LayoutMetrics {
            list_inner_x: list_inner.x,
            list_line_width: list_inner.width as usize,
            ..plan.metrics
        },
        settings_section_rows: plan.settings_section_rows,
        frame_margin_top: plan.frame_margin_top,
        frame_margin_h: plan.frame_margin_h,
    }
}

fn sessions_list_inner(area: Rect, plan: &LayoutPlan) -> Rect {
    let (_, body_area, _) = layout_regions(area, plan);
    Block::default()
        .padding(Padding::new(
            SESSION_BLOCK_PAD_LEFT,
            SESSION_BLOCK_PAD_RIGHT,
            0,
            0,
        ))
        .inner(body_area)
}

fn content_rect(area: Rect, plan: &LayoutPlan) -> Rect {
    Rect::new(
        area.x.saturating_add(plan.frame_margin_h),
        area.y.saturating_add(plan.frame_margin_top),
        area
            .width
            .saturating_sub(plan.frame_margin_h.saturating_mul(2)),
        area.height.saturating_sub(plan.frame_margin_top),
    )
}

pub fn terminal_list_area(metrics: &LayoutMetrics) -> Rect {
    Rect {
        x: metrics.list_inner_x,
        y: metrics.list_top_y,
        width: metrics.list_line_width as u16,
        height: metrics.list_height as u16,
    }
}

pub(crate) fn layout_regions(area: Rect, plan: &LayoutPlan) -> (Rect, Rect, Rect) {
    let content = content_rect(area, plan);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TOOLBAR_SECTION_ROWS),
            Constraint::Min(SESSION_BLOCK_ROWS + 1),
            Constraint::Length(plan.settings_section_rows),
        ])
        .split(content);
    (
        chunks.first().copied().unwrap_or_default(),
        chunks.get(1).copied().unwrap_or(content),
        chunks.get(2).copied().unwrap_or_default(),
    )
}