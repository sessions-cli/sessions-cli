use crate::bar::notepad::Note;
use ratatui::layout::{Constraint, Direction, Layout, Rect, Size};
use ratatui::widgets::block::Padding;
use ratatui::widgets::Block;

use super::super::sessions::RowKind;
use super::metrics::{LayoutMetrics, LayoutPlan};
use super::trail::{
    default_sidebar_line_width, notepad_header_row_index, notepad_list_state, visible_session_rows,
    NOTEPAD_BODY_PAD_TOP, NOTEPAD_BODY_ROWS,
};

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
    let update_rows = if show_update_banner {
        UPDATE_BOX_ROWS
    } else {
        0
    };
    update_rows + BOTTOM_CHROME_ROWS + TOOLBAR_SECTION_PAD * 2
}

const NOTEPAD_HEADER_ROWS: u16 = 1;

pub const MIN_PANE_WIDTH: u16 = 22;
pub const DEFAULT_PANE_WIDTH: u16 = 54;
/// Keyboard resize step for `[` / `]` (columns).
pub const KEYBOARD_RESIZE_STEP: u16 = 4;
/// Larger keyboard resize step for `{` / `}` / Shift+`[` / Shift+`]`.
pub const KEYBOARD_RESIZE_STEP_LARGE: u16 = 10;
/// Micro status rail when the outer client is too narrow for a full sidebar.
/// Wide enough for a centered status chip with side padding + top expand control.
pub const COLLAPSED_PANE_WIDTH: u16 = 4;
/// Dragging the divider to this width or below snaps into the micro rail.
/// Must be below [`MIN_PANE_WIDTH`] so a normal narrow list is still possible above it.
pub const DRAG_COLLAPSE_AT_OR_BELOW: u16 = 16;
/// While collapsed, any drag open past the rail width counts as expand intent.
/// Kept low so a short drag isn't fought back to the rail every sync tick.
pub const DRAG_EXPAND_AT_OR_ABOVE: u16 = COLLAPSED_PANE_WIDTH + 2; // 6
/// Rows reserved at the top of the collapsed rail for the expand control (+ gap).
pub const RAIL_EXPAND_HEADER_ROWS: u16 = 2;
/// Expand affordance — points into the workspace (open full sidebar).
/// Single cell; rail layout centers it with left-heavy padding for optical balance.
pub const RAIL_EXPAND_ICON: char = '▸';
/// Text-only collapse control at the top-right of the expanded sidebar (no chevron).
pub const RAIL_COLLAPSE_LABEL: &str = "[collapse]";
/// Minimum expanded sidebar before we prefer collapsing over a crushed list.
pub const MIN_EXPANDED_PANE_WIDTH: u16 = 36;
/// Workspace columns we try to protect when deciding whether to auto-collapse.
/// Matches daemon `WORKSPACE_MIN_WIDTH` so clamp + collapse policy stay aligned.
pub const WORKSPACE_MIN_WIDTH: u16 = 48;
/// Temporary workspace floor while the user peeks the full sidebar on a narrow client.
/// Lower than [`WORKSPACE_MIN_WIDTH`] so the list opens near default width and stays usable.
pub const PEEK_WORKSPACE_MIN: u16 = 24;
/// Hysteresis so tiny client-width jitter does not thrash collapse/expand.
pub const SIDEBAR_AUTO_COLLAPSE_HYSTERESIS: u16 = 16;
/// Fixed inset — must not vary with pane width or sidebar resize shifts content.
pub(crate) const FRAME_MARGIN_H: u16 = 0;
pub(crate) const FRAME_MARGIN_TOP: u16 = 0;
/// Inset inside the frame margin — right is thicker so shortcuts align with left icon inset.
pub const SESSION_BLOCK_PAD_LEFT: u16 = 0;
pub const SESSION_BLOCK_PAD_RIGHT: u16 = 2;

/// Client width below which the expanded sidebar should snap to the rail.
///
/// Formula: `preferred + WORKSPACE_MIN_WIDTH` (default 54 + 48 = 102).
/// Host-terminal resize must **hold preferred fixed** until this point, then
/// collapse — never soft-clamp the list through intermediate widths (that reflows
/// every column and feels like the sidebar is jumping).
pub fn sidebar_auto_collapse_below(preferred_sidebar: u16) -> u16 {
    let preferred = preferred_sidebar.max(MIN_PANE_WIDTH);
    preferred.saturating_add(WORKSPACE_MIN_WIDTH)
}

/// Client width at/above which an auto-collapsed sidebar expands again.
pub fn sidebar_auto_expand_above(preferred_sidebar: u16) -> u16 {
    sidebar_auto_collapse_below(preferred_sidebar).saturating_add(SIDEBAR_AUTO_COLLAPSE_HYSTERESIS)
}

/// Whether the sidebar should be in the auto-collapsed rail state for this client width.
///
/// Uses hysteresis: once collapsed, stays collapsed until the client grows past
/// [`sidebar_auto_expand_above`]; once expanded, stays expanded until it falls
/// below [`sidebar_auto_collapse_below`].
pub fn sidebar_should_auto_collapse(
    client_width: u16,
    preferred_sidebar: u16,
    currently_collapsed: bool,
) -> bool {
    if currently_collapsed {
        client_width < sidebar_auto_expand_above(preferred_sidebar)
    } else {
        client_width < sidebar_auto_collapse_below(preferred_sidebar)
    }
}

/// Target sidebar pane width for the current responsive state.
///
/// - Collapsed rail → [`COLLAPSED_PANE_WIDTH`] (micro status strip).
/// - Peek expand (`force_expanded`) → prefer full preferred/default width, only
///   leaving [`PEEK_WORKSPACE_MIN`] for the agent pane (temporary, intentional squeeze).
/// - Normal expanded → **fixed preferred** (no soft clamp). Auto-collapse is the
///   only transition out of this width during host resize.
pub fn responsive_sidebar_width(
    preferred_sidebar: u16,
    client_width: u16,
    auto_collapsed: bool,
    force_expanded: bool,
) -> u16 {
    if auto_collapsed && !force_expanded {
        return COLLAPSED_PANE_WIDTH;
    }
    let preferred = preferred_sidebar.max(MIN_PANE_WIDTH);
    // Peek: open to at least the default list width so titles/status stay readable.
    if force_expanded {
        let target = preferred.max(DEFAULT_PANE_WIDTH);
        let max_allowed = client_width.saturating_sub(PEEK_WORKSPACE_MIN);
        let floor = MIN_EXPANDED_PANE_WIDTH.min(max_allowed.max(1)).max(1);
        return target.clamp(floor, max_allowed.max(floor));
    }
    // Expanded: hold preferred fixed. Caller collapses via sidebar_should_auto_collapse
    // when preferred + WORKSPACE_MIN no longer fits — never drip-compress here.
    preferred
}

/// Workspace min the daemon clamp should honor for this target width intent.
pub fn workspace_min_for_sidebar_target(force_expanded: bool) -> u16 {
    if force_expanded {
        PEEK_WORKSPACE_MIN
    } else {
        WORKSPACE_MIN_WIDTH
    }
}

/// True when the bar pane is currently the collapsed rail (not a full list).
pub fn is_collapsed_sidebar_width(width: u16) -> bool {
    width > 0 && width <= COLLAPSED_PANE_WIDTH
}

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
    let update_rows = if show_update_banner {
        UPDATE_BOX_ROWS
    } else {
        0
    };
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
    let _line_width = default_sidebar_line_width();
    let header_row = notepad_header_row_index(visible_sessions, sessions_expanded, &notepad_state);
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
        area.width
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
