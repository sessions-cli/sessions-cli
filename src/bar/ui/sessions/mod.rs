mod types;

pub use types::*;

use super::theme::*;
use crate::bar::group_order;
use crate::model::{AgentState, Session};
use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::{HashMap, HashSet};

pub use crate::bar::group_order::MAX_THREADS_PER_GROUP;

use super::{
    widgets::{
        chrome_row_prefix, completion_badge_style, empty_trailing_slot,
        format_completion_square_slot, format_spinner_slot, format_trailing_slot, full_width_spans,
        group_add_badge_style, render_full_width_row_backdrop, row_label_width,
        row_with_trailing_slot, run_spinner_glyph, spinner_badge_style, truncate, RenameState,
        GROUP_ADD_ICON, ROW_PRE_TRAILING_GAP, TRAILING_SLOT_WIDTH,
    },
    LayoutMetrics,
};
/// Trailing slot for time/spinner — times right-aligned (`17m`), spinner centered (` ⠿ `).
/// Lead (2) + index slot (2 digits + 2 spaces) before the label.
/// Matches the leading space in the `" sessions "` block title.
/// Note title text lines up with the `n` in `"▾ notes"` (chrome + chevron + space).
/// Group header add affordance — bracketed plus, classic dialog/menu TUI idiom.
/// Invisible hit target spans the pre-trailing gap, icon slot, and two label columns.
pub(crate) const GROUP_ADD_CLICK_WIDTH: usize = ROW_PRE_TRAILING_GAP + TRAILING_SLOT_WIDTH + 2;
/// Quick-launch cluster on pwd group hover — each badge is one trailing slot wide.
pub(crate) const GROUP_LAUNCH_BUTTON_WIDTH: usize = TRAILING_SLOT_WIDTH;
pub(crate) const GROUP_LAUNCH_MAX_BUTTONS: usize = crate::telemetry::config::GROUP_LAUNCH_MAX;

/// Default badges used by tests / fallback when config is unavailable.
pub fn default_group_launch_agents() -> Vec<String> {
    crate::telemetry::config::default_group_launch()
}

pub fn group_launch_trailing_width(agent_count: usize) -> usize {
    GROUP_LAUNCH_BUTTON_WIDTH * agent_count.min(GROUP_LAUNCH_MAX_BUTTONS)
}

/// Hit target for the launch cluster — gap + badges + two label columns of slack.
pub fn group_launch_click_width(agent_count: usize) -> usize {
    if agent_count == 0 {
        return 0;
    }
    ROW_PRE_TRAILING_GAP + group_launch_trailing_width(agent_count) + 2
}

/// Legacy constant for default 3-badge layout (tests / sessions title add).
pub(crate) const GROUP_LAUNCH_BUTTON_COUNT: usize = 3;
pub(crate) const GROUP_LAUNCH_TRAILING_WIDTH: usize =
    GROUP_LAUNCH_BUTTON_WIDTH * GROUP_LAUNCH_BUTTON_COUNT;
pub(crate) const GROUP_LAUNCH_CLICK_WIDTH: usize =
    ROW_PRE_TRAILING_GAP + GROUP_LAUNCH_TRAILING_WIDTH + 2;

/// Style for a one-letter agent badge on a group header — same as `[+]`.
pub(crate) fn group_launch_badge_style(_agent_id: &str, row_style: Style) -> Style {
    group_add_badge_style(row_style)
}

pub(crate) fn group_launch_badge_glyph(agent_id: &str) -> &'static str {
    crate::telemetry::config::group_launch_badge(agent_id)
}

pub(crate) fn group_header_label(label: &str, collapsed: bool) -> String {
    let chevron = if collapsed { "▸" } else { "▾" };
    format!("{chevron} {label}")
}

pub(crate) fn group_header_label_drag(
    label: &str,
    collapsed: bool,
    highlight: Option<GroupHighlight>,
) -> String {
    let base = group_header_label(label, collapsed);
    match highlight {
        Some(GroupHighlight::Source) => base.replacen("▸", "⠿", 1).replacen("▾", "⠿", 1),
        Some(GroupHighlight::Target) => format!("│ {base}"),
        None => base,
    }
}

pub fn build_rows(
    sessions: &[Session],
    expanded_groups: &HashSet<String>,
    folded_groups: &HashSet<String>,
    group_order: &[String],
) -> Vec<RowKind> {
    let mut rows = Vec::new();
    if sessions.is_empty() {
        rows.push(RowKind::Empty("No terminals".into()));
        return rows;
    }

    let mut by_dir: HashMap<String, Vec<Session>> = HashMap::new();
    for session in sessions {
        by_dir
            .entry(session.cwd_label.clone())
            .or_default()
            .push(session.clone());
    }

    let dirs = group_order::order_labels(by_dir.keys().cloned().collect(), group_order);

    for (dir_idx, cwd_label) in dirs.iter().enumerate() {
        let Some(mut group) = by_dir.remove(cwd_label) else {
            continue;
        };
        group.sort_by(|a, b| a.cmp_within_group(b));

        let expanded = expanded_groups.contains(cwd_label);
        let collapsed = folded_groups.contains(cwd_label);
        let total = group.len();

        if dir_idx > 0 {
            rows.push(RowKind::Empty(String::new()));
        }
        rows.push(RowKind::Group {
            label: cwd_label.clone(),
            collapsed,
        });
        if collapsed {
            continue;
        }
        let visible = group_order::visible_sessions_in_group(&group, expanded);
        let visible_count = visible.len();

        for session in visible {
            rows.push(RowKind::Session { session });
        }

        if total > MAX_THREADS_PER_GROUP {
            rows.push(RowKind::GroupToggle {
                cwd_label: cwd_label.clone(),
                expanded,
                hidden_count: total.saturating_sub(visible_count),
            });
        }
    }

    rows
}

pub fn selectable_indices(rows: &[RowKind]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, row)| {
            matches!(row, RowKind::Session { .. } | RowKind::GroupToggle { .. }).then_some(i)
        })
        .collect()
}

pub fn group_toggle_at(rows: &[RowKind], row_idx: usize) -> Option<&str> {
    match rows.get(row_idx) {
        Some(RowKind::GroupToggle { cwd_label, .. }) => Some(cwd_label.as_str()),
        _ => None,
    }
}

pub fn group_label_at(rows: &[RowKind], row_idx: usize) -> Option<&str> {
    match rows.get(row_idx) {
        Some(RowKind::Group { label, .. }) => Some(label.as_str()),
        _ => None,
    }
}

pub fn group_sections(rows: &[RowKind]) -> Vec<GroupSection> {
    let mut sections = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if let RowKind::Group { label, .. } = &rows[i] {
            let start = i;
            let label = label.clone();
            i += 1;
            while i < rows.len() {
                match &rows[i] {
                    RowKind::Group { .. } => break,
                    RowKind::Empty(label) if label.is_empty() => break,
                    _ => i += 1,
                }
            }
            sections.push(GroupSection {
                label,
                start,
                end: i - 1,
            });
        } else {
            i += 1;
        }
    }
    sections
}

fn section_index_for_row(
    sections: &[GroupSection],
    rows: &[RowKind],
    row_idx: usize,
) -> Option<usize> {
    for (idx, section) in sections.iter().enumerate() {
        if row_idx >= section.start && row_idx <= section.end {
            return Some(idx);
        }
    }
    if matches!(rows.get(row_idx), Some(RowKind::Empty(_))) {
        return sections.iter().position(|section| section.start > row_idx);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupHighlight {
    Target,
    Source,
}

pub(crate) fn group_section_highlight(
    sections: &[GroupSection],
    rows: &[RowKind],
    row_idx: usize,
    group_drag: &GroupDragState,
) -> Option<GroupHighlight> {
    // Grip (⠿) and target highlight only after the pointer leaves the source
    // section (`dragged`). Hold/pending presses must keep the normal chevron so
    // fold clicks never flash the drag icon — especially under IDE Drag noise.
    if !group_drag.active() || !group_drag.dragged {
        return None;
    }
    let section = sections.get(section_index_for_row(sections, rows, row_idx)?)?;
    let label = &section.label;

    if group_drag.source.as_deref() == Some(label.as_str()) {
        return Some(GroupHighlight::Source);
    }

    if group_drag.hover.as_deref() == Some(label.as_str()) {
        return Some(GroupHighlight::Target);
    }

    None
}

pub(crate) fn group_drag_row_backdrop(
    _sections: &[GroupSection],
    _rows: &[RowKind],
    _row_idx: usize,
    _group_drag: &GroupDragState,
) -> Option<Color> {
    None
}

pub(crate) fn session_row_is_selected(
    session: &Session,
    row_idx: usize,
    selected: usize,
    group_drag: &GroupDragState,
) -> bool {
    if group_drag.active() {
        return group_drag
            .preserved_session_id
            .as_deref()
            .is_some_and(|id| session.id == id);
    }
    row_idx == selected
}

pub(crate) fn group_toggle_row_is_selected(
    cwd_label: &str,
    row_idx: usize,
    selected: usize,
    group_drag: &GroupDragState,
) -> bool {
    if group_drag.active() {
        return group_drag
            .preserved_group_toggle
            .as_deref()
            .is_some_and(|label| label == cwd_label);
    }
    row_idx == selected
}

pub fn group_drag_target(rows: &[RowKind], row_idx: usize, source: &str) -> Option<String> {
    let sections = group_sections(rows);
    let source_idx = sections
        .iter()
        .position(|section| section.label == source)?;
    let target_idx = section_index_for_row(&sections, rows, row_idx)?;
    let target = &sections[target_idx];
    if target.label == source {
        return Some(source.to_string());
    }

    // Dropping on a pwd header always targets that directory group.
    if row_idx == target.start && matches!(rows.get(row_idx), Some(RowKind::Group { .. })) {
        return Some(target.label.clone());
    }

    let height = target.end - target.start + 1;
    let offset = row_idx.saturating_sub(target.start);
    let in_lower_half = offset > (height.saturating_sub(1)) / 2;

    if source_idx < target_idx {
        if in_lower_half {
            Some(target.label.clone())
        } else {
            Some(source.to_string())
        }
    } else if in_lower_half {
        Some(source.to_string())
    } else {
        Some(target.label.clone())
    }
}

pub(crate) fn close_mode_muted_style(state: AgentState, selected: bool, is_active: bool) -> Style {
    state_style(state, selected, is_active)
        .fg(CLOSE_MODE_FG)
        .remove_modifier(Modifier::BOLD)
}

pub(crate) fn state_style(_state: AgentState, highlighted: bool, _is_active: bool) -> Style {
    let bg = if highlighted { BG_SELECTED } else { BG_BASE };
    Style::default().fg(TEXT_SELECTED).bg(bg)
}

pub(crate) fn session_row_has_selected_backdrop(
    session: &Session,
    row_idx: usize,
    selected: usize,
    close_modifier_held: bool,
    hover_row: Option<usize>,
    rows: &[RowKind],
    group_drag: &GroupDragState,
) -> bool {
    if session_row_is_selected(session, row_idx, selected, group_drag) {
        return true;
    }
    if close_modifier_held {
        return false;
    }
    hover_row == Some(row_idx)
        && matches!(
            rows.get(row_idx),
            Some(RowKind::Session { .. } | RowKind::GroupToggle { .. })
        )
}

pub(crate) fn session_row_is_hovered(
    row_idx: usize,
    hover_row: Option<usize>,
    rows: &[RowKind],
) -> bool {
    hover_row == Some(row_idx)
        && matches!(
            rows.get(row_idx),
            Some(RowKind::Session { .. } | RowKind::GroupToggle { .. })
        )
}

pub(crate) fn session_row_base_style(
    session: &Session,
    row_idx: usize,
    selected: usize,
    close_modifier_held: bool,
    hover_row: Option<usize>,
    close_target: Option<usize>,
    rows: &[RowKind],
    group_drag: &GroupDragState,
) -> Style {
    let is_selected = session_row_is_selected(session, row_idx, selected, group_drag);
    let is_close_target =
        close_target_row(rows, close_modifier_held, close_target, selected, row_idx);
    if is_close_target {
        return Style::default()
            .fg(CLOSE_HOVER_FG)
            .bg(CLOSE_HOVER_BG)
            .add_modifier(Modifier::BOLD);
    }
    if close_modifier_held {
        let has_selected_backdrop = if group_drag.active() {
            false
        } else {
            session_row_has_selected_backdrop(
                session,
                row_idx,
                selected,
                close_modifier_held,
                hover_row,
                rows,
                group_drag,
            )
        };
        return close_mode_muted_style(
            session.sidebar_state(),
            has_selected_backdrop,
            session.is_active,
        );
    }
    if group_drag.active() {
        return state_style(session.sidebar_state(), false, session.is_active);
    }
    let is_hovered = session_row_is_hovered(row_idx, hover_row, rows);
    if is_selected && is_hovered {
        Style::default().fg(TEXT_SELECTED).bg(BG_HOVER_SELECTED)
    } else if is_selected {
        state_style(session.sidebar_state(), true, session.is_active)
    } else if is_hovered {
        Style::default().fg(TEXT_SELECTED).bg(BG_HIGHLIGHT)
    } else {
        state_style(session.sidebar_state(), false, session.is_active)
    }
}

pub(crate) fn session_row_backdrop_bg(
    session: &Session,
    row_idx: usize,
    selected: usize,
    close_modifier_held: bool,
    hover_row: Option<usize>,
    close_target: Option<usize>,
    rows: &[RowKind],
    sections: &[GroupSection],
    group_drag: &GroupDragState,
) -> Option<Color> {
    if group_drag.active() {
        return None;
    }
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
    let row_style = apply_group_highlight(
        base_row_style,
        group_section_highlight(sections, rows, row_idx, group_drag),
    );
    row_style.bg.filter(|&bg| bg != BG_BASE)
}

pub(crate) fn dim_color(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as u16 * 65 / 100) as u8,
            (g as u16 * 65 / 100) as u8,
            (b as u16 * 65 / 100) as u8,
        ),
        other => other,
    }
}

pub(crate) fn dim_style(style: Style) -> Style {
    let style = match style.fg {
        Some(fg) => style.fg(dim_color(fg)),
        None => style.fg(dim_color(TEXT_PRIMARY)),
    };
    style.remove_modifier(Modifier::BOLD)
}

pub(crate) fn apply_group_highlight(style: Style, highlight: Option<GroupHighlight>) -> Style {
    match highlight {
        Some(GroupHighlight::Source) => dim_style(style),
        Some(GroupHighlight::Target) => style.fg(TEXT_SELECTED).add_modifier(Modifier::BOLD),
        None => style,
    }
}

pub fn session_display_label(session: &Session) -> String {
    if crate::pty::is_console_session(&session.description, &session.title) {
        return crate::pty::CONSOLE_LABEL.to_string();
    }

    let title = session.title.trim();
    let description = session.description.trim();
    let app = crate::pty::parse_app(title);
    let thread = if !description.is_empty() {
        description.to_string()
    } else {
        crate::pty::parse_description(title)
    };

    if let Some(app) = app.filter(|name| crate::pty::is_agent_app(name)) {
        if !thread.is_empty() && thread != app && !crate::pty::is_weak_thread_name(&thread) {
            return format!("{app} · {thread}");
        }
        return app;
    }

    if !thread.is_empty() {
        return thread;
    }
    if !title.is_empty() {
        return title.to_string();
    }
    "session".to_string()
}

pub(crate) fn compact_session_label(session: &Session) -> String {
    session_display_label(session)
}

pub(crate) fn trailing_badge_style(row_style: Style) -> Style {
    Style::default()
        .fg(PATH_FG)
        .bg(row_style.bg.unwrap_or(BG_BASE))
}

fn format_time_ago(at: DateTime<Utc>) -> String {
    let secs = (Utc::now() - at).num_seconds().max(0);
    let mins = (secs / 60).max(1);
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}

fn time_badge(session: &Session, row_style: Style) -> Option<(String, Style)> {
    let at = session.time_badge_at()?;
    let ago = format_time_ago(at);
    Some((format_trailing_slot(&ago), trailing_badge_style(row_style)))
}

pub fn session_trailing_badge(
    session: &Session,
    row_style: Style,
    anim_frame: usize,
) -> (String, Style) {
    if session.thread_is_complete() {
        return (
            format_completion_square_slot(),
            completion_badge_style(row_style),
        );
    }
    if session.shows_run_spinner() {
        let glyph = run_spinner_glyph(anim_frame);
        return (format_spinner_slot(glyph), spinner_badge_style(row_style));
    }
    if let Some(badge) = time_badge(session, row_style) {
        return badge;
    }
    (format_trailing_slot(""), trailing_badge_style(row_style))
}

/// True when any session in the list would show a run spinner (ignores scroll).
pub fn needs_run_spinner_animation(sessions: &[Session]) -> bool {
    sessions.iter().any(Session::shows_run_spinner)
}

/// True when a **visible** list row needs the run spinner, given current scroll
/// and list body height. Off-screen working sessions do not drive continuous
/// animation (avoids burning the UI thread when many agents are working).
///
/// `rows` is the expanded-list row model (group headers + sessions). Viewport is
/// `[scroll, scroll + list_height)`.
pub fn needs_run_spinner_animation_in_viewport(
    rows: &[RowKind],
    scroll: usize,
    list_height: usize,
) -> bool {
    if list_height == 0 {
        return false;
    }
    rows.iter()
        .skip(scroll)
        .take(list_height)
        .any(|row| matches!(row, RowKind::Session { session } if session.shows_run_spinner()))
}

/// One status chip in the collapsed micro rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailStatusKind {
    Working,
    Approval,
    Error,
    Done,
    /// Focused session with no stronger status — so you still see "where you are".
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RailStatusItem {
    pub kind: RailStatusKind,
    pub is_active: bool,
    /// Index into the expanded sidebar `rows` list — same vertical slot as expanded mode.
    pub list_row: usize,
}

fn rail_status_kind_for(session: &Session) -> Option<RailStatusKind> {
    if session.state == AgentState::Error {
        return Some(RailStatusKind::Error);
    }
    if session.state == AgentState::Approval {
        return Some(RailStatusKind::Approval);
    }
    if session.shows_run_spinner() {
        return Some(RailStatusKind::Working);
    }
    if session.thread_is_complete() {
        return Some(RailStatusKind::Done);
    }
    if session.is_active {
        return Some(RailStatusKind::Active);
    }
    None
}

/// Status chips for the micro rail, **in sidebar list order** (not priority-sorted).
///
/// Each chip carries `list_row` so drawing can place it on the same Y as the
/// expanded session row (group headers leave empty gaps for alignment).
pub fn rail_status_items(rows: &[RowKind]) -> Vec<RailStatusItem> {
    rows.iter()
        .enumerate()
        .filter_map(|(list_row, row)| {
            let RowKind::Session { session } = row else {
                return None;
            };
            let kind = rail_status_kind_for(session)?;
            Some(RailStatusItem {
                kind,
                is_active: session.is_active,
                list_row,
            })
        })
        .collect()
}

/// Screen Y for a list row, or `None` if scrolled out of the visible body.
pub fn rail_item_screen_y(
    list_row: usize,
    scroll: usize,
    list_top_y: u16,
    list_height: usize,
) -> Option<u16> {
    if list_row < scroll {
        return None;
    }
    let offset = list_row - scroll;
    if offset >= list_height {
        return None;
    }
    Some(list_top_y.saturating_add(offset as u16))
}

/// Glyph + colors for a micro-rail status chip.
///
/// Collapsed rail is icon-only: colored foreground on base, **no** row/section
/// highlight fills (`WORKING_BG` / selection / active backdrop). Expanded mode
/// keeps those status backdrops on full session rows.
pub fn rail_status_glyph(kind: RailStatusKind, anim_frame: usize) -> (char, Style) {
    match kind {
        RailStatusKind::Working => (
            run_spinner_glyph(anim_frame).chars().next().unwrap_or('⠿'),
            Style::default().fg(Color::Rgb(120, 170, 255)).bg(BG_BASE),
        ),
        RailStatusKind::Approval => ('◆', Style::default().fg(WARM_ACCENT).bg(BG_BASE)),
        RailStatusKind::Error => (
            '!',
            Style::default().fg(Color::Rgb(255, 160, 160)).bg(BG_BASE),
        ),
        RailStatusKind::Done => ('■', Style::default().fg(DONE_FG).bg(BG_BASE)),
        // Focused session without a stronger status — dim marker, not a selection bar.
        RailStatusKind::Active => ('·', Style::default().fg(TEXT_SECONDARY).bg(BG_BASE)),
    }
}

fn sessions_title_label(sessions_expanded: bool) -> String {
    let chevron = if sessions_expanded { "▾" } else { "▸" };
    format!(" {chevron} sessions ")
}

fn sessions_title_line_with_add(
    sessions_expanded: bool,
    sessions_title_hover: bool,
    show_add: bool,
    line_width: usize,
) -> Line<'static> {
    let title_bg = if sessions_title_hover {
        BG_HIGHLIGHT
    } else {
        BG_BASE
    };
    let header_style = Style::default().fg(BRAND_FG).bg(title_bg);
    let title = sessions_title_label(sessions_expanded);
    let (trailing, trailing_style) = if show_add {
        (
            format_trailing_slot(GROUP_ADD_ICON),
            group_add_badge_style(header_style),
        )
    } else {
        empty_trailing_slot(header_style)
    };
    let title_len = title.chars().count();
    let trailing_area = ROW_PRE_TRAILING_GAP + TRAILING_SLOT_WIDTH;
    let label_width = line_width.saturating_sub(trailing_area);
    let pad = label_width.saturating_sub(title_len);
    Line::from(vec![
        Span::styled(title, header_style),
        Span::styled(" ".repeat(pad), header_style),
        Span::styled(" ".repeat(ROW_PRE_TRAILING_GAP), header_style),
        Span::styled(trailing, trailing_style),
    ])
}

pub(crate) fn sessions_block_title(
    close_modifier_held: bool,
    digit_buffer: &str,
    rename: Option<&RenameState>,
    delete_note_confirm: Option<&super::widgets::DeleteNoteConfirmState>,
    sessions_expanded: bool,
    sessions_title_hover: bool,
    sessions_title_add_hover: bool,
    line_width: usize,
    clipboard_notice: Option<&str>,
) -> Line<'static> {
    let title_bg = if sessions_title_hover {
        BG_HIGHLIGHT
    } else {
        BG_BASE
    };
    let base_style = Style::default().fg(BRAND_FG).bg(title_bg);
    let hint_style = Style::default().fg(TEXT_SECONDARY).bg(BG_BASE);
    let rename_style = Style::default().fg(RENAME_EDIT_FG).bg(BG_BASE);
    let delete_style = Style::default().fg(CLOSE_HOVER_FG).bg(BG_BASE);
    let title = sessions_title_label(sessions_expanded);
    if delete_note_confirm.is_some() {
        return Line::from(vec![
            Span::styled(" delete note ", delete_style.add_modifier(Modifier::BOLD)),
            Span::styled(" type yes · enter confirm · esc cancel ", hint_style),
        ]);
    }
    if rename.is_some() {
        return Line::from(vec![
            Span::styled(" rename ", rename_style.add_modifier(Modifier::BOLD)),
            Span::styled(" enter save · esc cancel ", hint_style),
        ]);
    }
    if !digit_buffer.is_empty() {
        return Line::from(Span::styled(format!(" go:{digit_buffer} "), base_style));
    }
    if close_modifier_held {
        return Line::from(vec![
            Span::styled(title, base_style),
            Span::styled(" hold d · enter delete · esc exit ", hint_style),
        ]);
    }
    if let Some(notice) = clipboard_notice.filter(|text| !text.is_empty()) {
        return Line::from(vec![
            Span::styled(title, base_style),
            Span::styled(format!(" {notice} "), hint_style),
        ]);
    }
    let show_add = sessions_title_hover || sessions_title_add_hover;
    if show_add && line_width > 0 {
        return sessions_title_line_with_add(
            sessions_expanded,
            sessions_title_hover,
            show_add,
            line_width,
        );
    }
    Line::from(Span::styled(title, base_style))
}

pub fn sessions_title_hit(y: u16, metrics: &LayoutMetrics) -> bool {
    y == metrics.sessions_title_y
}

pub fn sessions_title_add_click(column: u16, y: u16, metrics: &LayoutMetrics) -> bool {
    sessions_title_hit(y, metrics) && is_group_add_click(column, metrics)
}

pub fn sessions_title_add_hover_from_mouse(column: u16, y: u16, metrics: &LayoutMetrics) -> bool {
    sessions_title_add_click(column, y, metrics)
}

pub(crate) fn render_sessions_title_hover_overlay(
    frame: &mut Frame,
    pane_area: Rect,
    title_y: u16,
    title: Line<'static>,
) {
    render_full_width_row_backdrop(frame, pane_area, title_y, BG_HIGHLIGHT);
    let pad_style = Style::default().bg(BG_HIGHLIGHT);
    let padded = full_width_spans(title.spans, pane_area.width as usize, pad_style);
    frame.render_widget(
        Paragraph::new(padded),
        Rect {
            x: pane_area.x,
            y: title_y,
            width: pane_area.width,
            height: 1,
        },
    );
}

pub fn close_target_row(
    rows: &[RowKind],
    close_modifier_held: bool,
    close_target: Option<usize>,
    _selected: usize,
    row_idx: usize,
) -> bool {
    if !close_modifier_held {
        return false;
    }
    if !matches!(rows.get(row_idx), Some(RowKind::Session { .. })) {
        return false;
    }
    match close_target {
        Some(target) if matches!(rows.get(target), Some(RowKind::Session { .. })) => {
            row_idx == target
        }
        _ => false,
    }
}
pub fn pointer_in_list_body(column: u16, metrics: &LayoutMetrics) -> bool {
    if metrics.list_line_width == 0 {
        return false;
    }
    let left = metrics.list_inner_x;
    let right = left.saturating_add(metrics.list_line_width as u16);
    column >= left && column < right
}

pub fn pointer_in_list_viewport_y(y: u16, metrics: &LayoutMetrics) -> bool {
    let bottom = if metrics.update_banner_row_count > 0 {
        metrics.update_banner_top_y
    } else {
        metrics.settings_top_y
    };
    y >= metrics.list_top_y && y < bottom
}

pub fn row_from_mouse(
    y: u16,
    list_top_y: u16,
    body_height: usize,
    scroll: usize,
    total: usize,
) -> Option<usize> {
    if y < list_top_y {
        return None;
    }
    let rel = (y - list_top_y) as usize;
    if rel >= body_height {
        return None;
    }
    let row = scroll + rel;
    (row < total).then_some(row)
}

pub fn group_row_from_mouse(
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    total: usize,
    rows: &[RowKind],
) -> Option<usize> {
    row_from_mouse(y, metrics.list_top_y, metrics.list_height, scroll, total)
        .filter(|&row_idx| matches!(rows.get(row_idx), Some(RowKind::Group { .. })))
}

pub fn is_group_trailing_click(column: u16, metrics: &LayoutMetrics) -> bool {
    is_group_trailing_click_for(column, metrics, GROUP_LAUNCH_BUTTON_COUNT)
}

pub fn is_group_trailing_click_for(
    column: u16,
    metrics: &LayoutMetrics,
    agent_count: usize,
) -> bool {
    let click_w = group_launch_click_width(agent_count);
    if click_w == 0 {
        return false;
    }
    let click_start = metrics
        .list_inner_x
        .saturating_add(metrics.list_line_width.saturating_sub(click_w) as u16);
    column >= click_start
}

pub fn is_group_add_click(column: u16, metrics: &LayoutMetrics) -> bool {
    is_group_trailing_click(column, metrics)
}

/// Map a column on a group header to a configured agent id.
pub fn group_launch_agent_at(
    column: u16,
    metrics: &LayoutMetrics,
    agents: &[String],
) -> Option<String> {
    let count = agents.len().min(GROUP_LAUNCH_MAX_BUTTONS);
    if count == 0 || !is_group_trailing_click_for(column, metrics, count) {
        return None;
    }
    let trail_w = group_launch_trailing_width(count);
    let trail_start = metrics
        .list_inner_x
        .saturating_add(metrics.list_line_width.saturating_sub(trail_w) as u16);
    if column < trail_start {
        return agents.first().cloned();
    }
    let offset = column.saturating_sub(trail_start) as usize;
    let idx = (offset / GROUP_LAUNCH_BUTTON_WIDTH).min(count - 1);
    agents.get(idx).cloned()
}

pub fn group_add_click<'a>(
    column: u16,
    row: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    total: usize,
    rows: &'a [RowKind],
) -> Option<&'a str> {
    group_launch_click(
        column,
        row,
        metrics,
        scroll,
        total,
        rows,
        &default_group_launch_agents(),
    )
    .map(|(label, _)| label)
}

/// Pwd-group trailing badge click: returns `(cwd_label, agent_id)`.
pub fn group_launch_click<'a>(
    column: u16,
    row: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    total: usize,
    rows: &'a [RowKind],
    agents: &[String],
) -> Option<(&'a str, String)> {
    let agent_id = group_launch_agent_at(column, metrics, agents)?;
    let row_idx = group_row_from_mouse(row, metrics, scroll, total, rows)?;
    let label = group_label_at(rows, row_idx)?;
    Some((label, agent_id))
}

/// Group header line: on hover shows configured quick-launch badges.
pub(crate) fn group_header_line(
    label: &str,
    collapsed: bool,
    group_highlight: Option<GroupHighlight>,
    hovered: bool,
    line_width: usize,
    row_style: Style,
    agents: &[String],
) -> Line<'static> {
    let header = group_header_label_drag(label, collapsed, group_highlight);
    let agents: Vec<&str> = agents
        .iter()
        .map(String::as_str)
        .take(GROUP_LAUNCH_MAX_BUTTONS)
        .collect();
    if !hovered || agents.is_empty() {
        let (trailing, trailing_style) = empty_trailing_slot(row_style);
        return row_with_trailing_slot(
            chrome_row_prefix(),
            &header,
            trailing,
            line_width,
            row_style,
            trailing_style,
        );
    }
    let prefix = chrome_row_prefix();
    let prefix_width = prefix.chars().count();
    let trail_w = group_launch_trailing_width(agents.len());
    let label_width = line_width
        .saturating_sub(prefix_width)
        .saturating_sub(ROW_PRE_TRAILING_GAP)
        .saturating_sub(trail_w);
    let title = truncate(&header, label_width);
    let mut spans = vec![
        Span::styled(prefix, row_style),
        Span::styled(format!("{:<width$}", title, width = label_width), row_style),
        Span::styled(" ".repeat(ROW_PRE_TRAILING_GAP), row_style),
    ];
    for agent_id in agents {
        spans.push(Span::styled(
            group_launch_badge_glyph(agent_id),
            group_launch_badge_style(agent_id, row_style),
        ));
    }
    full_width_spans(spans, line_width, row_style)
}

pub(crate) fn scroll_session_count(rows: &[RowKind], scroll: usize) -> usize {
    rows.iter()
        .take(scroll)
        .filter(|row| matches!(row, RowKind::Session { .. }))
        .count()
}

pub fn list_text_point_from_mouse(
    column: u16,
    y: u16,
    metrics: &LayoutMetrics,
    scroll: usize,
    total_rows: usize,
    rows: &[RowKind],
    line_width: usize,
) -> Option<ListTextPoint> {
    if !pointer_in_list_body(column, metrics) {
        return None;
    }
    let row_idx = row_from_mouse(
        y,
        metrics.list_top_y,
        metrics.list_height,
        scroll,
        total_rows,
    )?;
    let rel_col = column.saturating_sub(metrics.list_inner_x) as usize;
    let text = list_row_visible_text(rows, row_idx, line_width);
    let char_idx = rel_col.min(text.chars().count());
    Some(ListTextPoint { row_idx, char_idx })
}

pub fn list_selected_plain_text(
    rows: &[RowKind],
    line_width: usize,
    anchor: ListTextPoint,
    head: ListTextPoint,
) -> String {
    let (start, end) = if (anchor.row_idx, anchor.char_idx) <= (head.row_idx, head.char_idx) {
        (anchor, head)
    } else {
        (head, anchor)
    };
    if start.row_idx == end.row_idx {
        let text = list_row_visible_text(rows, start.row_idx, line_width);
        return slice_chars(&text, start.char_idx, end.char_idx);
    }
    let mut parts = Vec::new();
    for row_idx in start.row_idx..=end.row_idx {
        let text = list_row_visible_text(rows, row_idx, line_width);
        let (from, to) = if row_idx == start.row_idx {
            (start.char_idx, text.chars().count())
        } else if row_idx == end.row_idx {
            (0, end.char_idx)
        } else {
            (0, text.chars().count())
        };
        let segment = slice_chars(&text, from, to);
        if !segment.is_empty() {
            parts.push(segment);
        }
    }
    parts.join("\n")
}

fn slice_chars(text: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    text.chars().skip(start).take(end - start).collect()
}

fn session_ordinal_for_row(rows: &[RowKind], row_idx: usize) -> usize {
    rows.iter()
        .take(row_idx + 1)
        .filter(|row| matches!(row, RowKind::Session { .. }))
        .count()
}

fn list_row_visible_text(rows: &[RowKind], row_idx: usize, line_width: usize) -> String {
    let Some(row) = rows.get(row_idx) else {
        return String::new();
    };
    match row {
        RowKind::Empty(label) => label.clone(),
        RowKind::Group { label, collapsed } => group_header_label(label, *collapsed),
        RowKind::GroupToggle {
            expanded,
            hidden_count,
            ..
        } => {
            if *expanded {
                "show less".to_string()
            } else {
                format!("show more (+{hidden_count})")
            }
        }
        RowKind::Session { session } => {
            let label_width = row_label_width(line_width);
            let label = if label_width < 12 {
                compact_session_label(session)
            } else {
                session_display_label(session)
            };
            let title = truncate(&label, label_width);
            let ordinal = session_ordinal_for_row(rows, row_idx);
            format!("  {ordinal:>2}  {title}")
        }
    }
}

#[cfg(test)]
mod time_tests {
    use super::format_time_ago;
    use chrono::Utc;

    #[test]
    fn format_time_ago_uses_minutes() {
        let at = Utc::now() - chrono::Duration::minutes(17);
        assert_eq!(format_time_ago(at), "17m");
    }

    #[test]
    fn format_time_ago_never_shows_seconds_and_floors_to_one_minute() {
        let fresh = Utc::now() - chrono::Duration::seconds(12);
        assert_eq!(format_time_ago(fresh), "1m");
        let zero = Utc::now();
        assert_eq!(format_time_ago(zero), "1m");
    }
}

#[cfg(test)]
mod spinner_viewport_tests {
    use super::{needs_run_spinner_animation_in_viewport, RowKind};
    use crate::model::{AgentState, Session};
    use chrono::Utc;

    fn working_session(tab_index: u32) -> Session {
        Session {
            id: format!("tmux:win:{tab_index}"),
            tab_index,
            state: AgentState::Working,
            last_event_at: Utc::now(),
            messaged_at: Some(Utc::now()),
            prompt_submitted: true,
            ..Default::default()
        }
    }

    fn idle_session(tab_index: u32) -> Session {
        Session {
            id: format!("tmux:win:{tab_index}"),
            tab_index,
            state: AgentState::Idle,
            last_event_at: Utc::now(),
            messaged_at: Some(Utc::now()),
            prompt_submitted: true,
            ..Default::default()
        }
    }

    #[test]
    fn viewport_spinner_false_when_working_row_scrolled_away() {
        let rows = vec![
            RowKind::Group {
                label: "~/tmp".into(),
                collapsed: false,
            },
            RowKind::Session {
                session: working_session(1),
            },
            RowKind::Session {
                session: idle_session(2),
            },
            RowKind::Session {
                session: idle_session(3),
            },
        ];
        // Working session is at index 1; viewport shows only rows 2.. with height 2.
        assert!(!needs_run_spinner_animation_in_viewport(&rows, 2, 2));
        // Same list with scroll covering the working row.
        assert!(needs_run_spinner_animation_in_viewport(&rows, 0, 2));
        assert!(needs_run_spinner_animation_in_viewport(&rows, 1, 1));
    }

    #[test]
    fn viewport_spinner_false_for_empty_height_or_idle_only() {
        let rows = vec![
            RowKind::Session {
                session: idle_session(1),
            },
            RowKind::Session {
                session: working_session(2),
            },
        ];
        assert!(!needs_run_spinner_animation_in_viewport(&rows, 0, 0));
        assert!(!needs_run_spinner_animation_in_viewport(&rows, 0, 1));
        assert!(needs_run_spinner_animation_in_viewport(&rows, 1, 1));
    }
}
