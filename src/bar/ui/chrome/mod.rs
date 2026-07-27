mod types;

pub use types::*;

use super::theme::*;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph};
use ratatui::Frame;
use std::collections::HashSet;

use super::{
    layout::{
        BOTTOM_CHROME_ROWS, RAIL_COLLAPSE_LABEL, TOOLBAR_BUTTON_ROWS, TOOLBAR_SECTION_PAD,
        UPDATE_BOX_ROWS,
    },
    widgets::{
        format_trailing_slot, full_width_spans, render_full_width_row_backdrop,
        row_label_width_after_prefix, row_prefix, row_with_trailing_slot, truncate,
    },
    LayoutMetrics,
};

/// Top-right chrome: `[collapse]` text only (no chevron).
pub fn collapse_control_label() -> String {
    RAIL_COLLAPSE_LABEL.to_string()
}

/// Screen Y for the top pad row above the toolbar buttons (sidebar top chrome).
pub fn collapse_control_y(metrics: &LayoutMetrics) -> u16 {
    metrics.toolbar_top_y.saturating_sub(TOOLBAR_SECTION_PAD)
}

/// Character width of the collapse control (for right-align + hit testing).
pub fn collapse_control_width() -> usize {
    collapse_control_label().chars().count()
}

/// Left column of the right-aligned collapse control within the list content width.
pub fn collapse_control_start_x(metrics: &LayoutMetrics) -> Option<u16> {
    let w = collapse_control_width();
    if metrics.list_line_width == 0 || w == 0 || w > metrics.list_line_width {
        return None;
    }
    Some(
        metrics
            .list_inner_x
            .saturating_add(metrics.list_line_width.saturating_sub(w) as u16),
    )
}

pub fn collapse_control_hit(column: u16, y: u16, metrics: &LayoutMetrics) -> bool {
    if y != collapse_control_y(metrics) {
        return false;
    }
    let Some(start) = collapse_control_start_x(metrics) else {
        return false;
    };
    let end = start.saturating_add(collapse_control_width() as u16);
    column >= start && column < end
}

pub fn collapse_control_hover_from_mouse(column: u16, y: u16, metrics: &LayoutMetrics) -> bool {
    collapse_control_hit(column, y, metrics)
}

/// Paint `[collapse]` top-right above the toolbar (top pad of the chrome section).
///
/// Text-only control: muted grey by default, session-row white ([`TEXT_SELECTED`]) on
/// hover so it feels clickable. No background fill or chevron.
pub fn render_collapse_control(frame: &mut Frame, metrics: &LayoutMetrics, hovered: bool) {
    let y = collapse_control_y(metrics);
    let Some(start_x) = collapse_control_start_x(metrics) else {
        return;
    };
    let w = collapse_control_width() as u16;
    if w == 0 {
        return;
    }
    let fg = if hovered { TEXT_SELECTED } else { PATH_FG };
    let style = Style::default().fg(fg).bg(BG_BASE);
    let line = Line::from(Span::styled(collapse_control_label(), style));
    frame.render_widget(
        Paragraph::new(line),
        Rect {
            x: start_x,
            y,
            width: w,
            height: 1,
        },
    );
}

fn coming_soon_frame_for(
    frames: &[(ToolbarAction, usize)],
    action: ToolbarAction,
) -> Option<usize> {
    frames
        .iter()
        .find(|(active, _)| *active == action)
        .map(|(_, frame)| *frame)
}

struct ToolbarButton {
    icon: &'static str,
    label: &'static str,
    shortcut: &'static str,
    action: ToolbarAction,
}

const TOOLBAR_BUTTONS: &[ToolbarButton] = &[
    ToolbarButton {
        icon: "+",
        label: "New session",
        shortcut: "⌘+N",
        action: ToolbarAction::NewSession,
    },
    ToolbarButton {
        icon: "⌕",
        label: "Search",
        shortcut: "⌘+S",
        action: ToolbarAction::Search,
    },
    ToolbarButton {
        icon: "↻",
        label: "Automations",
        shortcut: "⌘+A",
        action: ToolbarAction::Automations,
    },
    ToolbarButton {
        icon: "◇",
        label: "MCPs",
        shortcut: "⌘+M",
        action: ToolbarAction::Mcps,
    },
    ToolbarButton {
        icon: "△",
        label: "Skills",
        shortcut: "⌘+K",
        action: ToolbarAction::Skills,
    },
];

const SETTINGS_BUTTON: ToolbarButton = ToolbarButton {
    icon: "⚙",
    label: "Settings",
    shortcut: "",
    action: ToolbarAction::Settings,
};

const LEAVE_BUTTON: ToolbarButton = ToolbarButton {
    icon: "↩",
    label: "Leave sessions",
    shortcut: "⌃Q",
    action: ToolbarAction::Leave,
};
pub(crate) fn chrome_row_backdrop_bg(hovered: bool, active: bool) -> Option<Color> {
    if hovered && active {
        Some(BG_HOVER_SELECTED)
    } else if active {
        Some(BG_SELECTED)
    } else if hovered {
        Some(BG_HIGHLIGHT)
    } else {
        None
    }
}

/// Whether a toolbar button should show the selected backdrop for the open panel.
pub(crate) fn toolbar_action_is_active(
    action: ToolbarAction,
    workspace_new_session_open: bool,
    workspace_automations_open: bool,
    workspace_mcps_open: bool,
    workspace_skills_open: bool,
) -> bool {
    match action {
        ToolbarAction::NewSession => workspace_new_session_open,
        ToolbarAction::Automations => workspace_automations_open,
        ToolbarAction::Mcps => workspace_mcps_open,
        ToolbarAction::Skills => workspace_skills_open,
        ToolbarAction::Search | ToolbarAction::Settings | ToolbarAction::Leave => false,
    }
}

pub(crate) fn chrome_button_style(hovered: bool, active: bool) -> Style {
    let bg = chrome_row_backdrop_bg(hovered, active).unwrap_or(BG_BASE);
    Style::default().fg(TEXT_SELECTED).bg(bg)
}

fn chrome_coming_soon_style(hovered: bool, active: bool, pulse: bool) -> Style {
    let bg = if hovered && active {
        BG_HOVER_SELECTED
    } else if active || pulse {
        COMING_SOON_PULSE
    } else if hovered {
        BG_HIGHLIGHT
    } else {
        COMING_SOON_TINT
    };
    Style::default().fg(TEXT_SELECTED).bg(bg)
}

fn coming_soon_braille_glyph(seed: usize) -> char {
    COMING_SOON_BRAILLE[seed % COMING_SOON_BRAILLE.len()]
}

fn is_coming_soon_noise(ch: char) -> bool {
    COMING_SOON_BRAILLE.contains(&ch) || matches!(ch, '·' | '…')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComingSoonPhase {
    Glitch,
    Decode,
    Hold,
    Restore,
}

fn coming_soon_phase(frame: usize) -> (ComingSoonPhase, usize) {
    let mut rem = frame;
    if rem < COMING_SOON_GLITCH_FRAMES {
        return (ComingSoonPhase::Glitch, rem);
    }
    rem -= COMING_SOON_GLITCH_FRAMES;
    if rem < COMING_SOON_DECODE_FRAMES {
        return (ComingSoonPhase::Decode, rem);
    }
    rem -= COMING_SOON_DECODE_FRAMES;
    if rem < COMING_SOON_HOLD_FRAMES {
        return (ComingSoonPhase::Hold, rem);
    }
    rem -= COMING_SOON_HOLD_FRAMES;
    (ComingSoonPhase::Restore, rem)
}

fn corrupt_text(text: &str, frame: usize, corruption: usize) -> String {
    text.chars()
        .enumerate()
        .map(|(idx, ch)| {
            let seed = frame.wrapping_mul(19).wrapping_add(idx.wrapping_mul(37));
            if seed % 11 < corruption || seed % 17 == 0 {
                coming_soon_braille_glyph(seed.wrapping_add(idx))
            } else {
                ch
            }
        })
        .collect()
}

fn glitch_label(base: &str, local_frame: usize) -> String {
    corrupt_text(base, local_frame, if local_frame == 0 { 5 } else { 8 })
}

fn coming_soon_reveal_order(len: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    order.sort_by_key(|idx| {
        idx.saturating_mul(8)
            .saturating_add((idx.wrapping_mul(5) + 1) % 3)
    });
    order
}

fn coming_soon_revealed_indices(len: usize, reveal_count: usize) -> HashSet<usize> {
    coming_soon_reveal_order(len)
        .into_iter()
        .take(reveal_count)
        .collect()
}

fn decode_text_label(target: &str, local_frame: usize, decode_frames: usize) -> String {
    let target_chars: Vec<char> = target.chars().collect();
    let len = target_chars.len();
    if len == 0 {
        return String::new();
    }
    let reveal_count = if local_frame >= decode_frames.saturating_sub(1) {
        len
    } else {
        ((local_frame + 1) * len) / decode_frames
    };
    let revealed = coming_soon_revealed_indices(len, reveal_count);
    target_chars
        .iter()
        .enumerate()
        .map(|(idx, ch)| {
            if revealed.contains(&idx) {
                *ch
            } else {
                coming_soon_braille_glyph(local_frame.wrapping_mul(5) + idx)
            }
        })
        .collect()
}

fn decode_coming_soon_label(label_width: usize, local_frame: usize) -> String {
    truncate(
        &decode_text_label(COMING_SOON_TARGET, local_frame, COMING_SOON_DECODE_FRAMES),
        label_width,
    )
}

fn hold_coming_soon_label(label_width: usize, local_frame: usize) -> String {
    let dots = if local_frame < COMING_SOON_HOLD_PLAIN_FRAMES {
        ""
    } else {
        let step = (local_frame - COMING_SOON_HOLD_PLAIN_FRAMES) / COMING_SOON_DOT_STEP_FRAMES;
        match step.min(2) {
            0 => ".",
            1 => "..",
            _ => "...",
        }
    };
    truncate(&format!("{COMING_SOON_TARGET}{dots}"), label_width)
}

fn restore_label(base: &str, label_width: usize, local_frame: usize) -> String {
    let base_chars: Vec<char> = base.chars().collect();
    if base_chars.is_empty() {
        return String::new();
    }
    if local_frame >= COMING_SOON_RESTORE_FRAMES - 1 {
        return truncate(base, label_width);
    }
    let progress = local_frame + 1;
    let reveal_count = (progress * base_chars.len()) / COMING_SOON_RESTORE_FRAMES;
    let revealed = coming_soon_revealed_indices(base_chars.len(), reveal_count);
    let ghost = truncate(&format!("{COMING_SOON_TARGET}..."), label_width);
    let ghost_chars: Vec<char> = ghost.chars().collect();
    let out: String = base_chars
        .iter()
        .enumerate()
        .map(|(idx, ch)| {
            if revealed.contains(&idx) {
                *ch
            } else if idx < ghost_chars.len() && progress <= COMING_SOON_RESTORE_FRAMES / 2 {
                ghost_chars[idx]
            } else {
                coming_soon_braille_glyph(local_frame.wrapping_mul(5) + idx)
            }
        })
        .collect();
    truncate(&out, label_width)
}

pub(crate) fn coming_soon_label_text(base: &str, label_width: usize, frame: usize) -> String {
    if label_width == 0 {
        return String::new();
    }
    let (phase, local_frame) = coming_soon_phase(frame);
    let text = match phase {
        ComingSoonPhase::Glitch => glitch_label(base, local_frame),
        ComingSoonPhase::Decode => decode_coming_soon_label(label_width, local_frame),
        ComingSoonPhase::Hold => hold_coming_soon_label(label_width, local_frame),
        ComingSoonPhase::Restore => restore_label(base, label_width, local_frame),
    };
    truncate(&text, label_width)
}

fn coming_soon_char_style(ch: char, bg: Color, phase: ComingSoonPhase) -> Style {
    if is_coming_soon_noise(ch) {
        Style::default().fg(PATH_FG).bg(bg)
    } else if ch.is_ascii_alphabetic() {
        Style::default().fg(TEXT_PRIMARY).bg(bg)
    } else if phase == ComingSoonPhase::Glitch {
        Style::default().fg(TEXT_SECONDARY).bg(bg)
    } else {
        Style::default().fg(TEXT_PRIMARY).bg(bg)
    }
}

pub(crate) fn coming_soon_label_spans(
    base: &str,
    label_width: usize,
    frame: usize,
    row_style: Style,
) -> Vec<Span<'static>> {
    if label_width == 0 {
        return Vec::new();
    }
    let (phase, _) = coming_soon_phase(frame);
    let text = coming_soon_label_text(base, label_width, frame);
    let bg = row_style.bg.unwrap_or(BG_BASE);
    let pad_style = Style::default().bg(bg);
    if matches!(phase, ComingSoonPhase::Hold | ComingSoonPhase::Restore)
        && !text.chars().any(is_coming_soon_noise)
    {
        vec![Span::styled(
            format!("{:<width$}", text, width = label_width),
            Style::default().fg(TEXT_SELECTED).bg(bg),
        )]
    } else {
        let chars: Vec<char> = text.chars().collect();
        let mut spans: Vec<Span<'static>> = chars
            .iter()
            .map(|ch| Span::styled(ch.to_string(), coming_soon_char_style(*ch, bg, phase)))
            .collect();
        let pad = label_width.saturating_sub(chars.len());
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), pad_style));
        }
        spans
    }
}

fn toolbar_coming_soon_item(
    button: &ToolbarButton,
    width: usize,
    hovered: bool,
    active: bool,
    pulse: bool,
    frame: usize,
) -> ListItem<'static> {
    let style = chrome_coming_soon_style(hovered, active, pulse);
    let prefix = row_prefix(" ", Some(button.icon));
    let prefix_width = prefix.chars().count();
    let label_width = row_label_width_after_prefix(width, prefix_width);
    let trailing = format_trailing_slot(button.shortcut);
    let trailing_style = toolbar_trailing_style(style);
    let mut spans = Vec::new();
    spans.push(Span::styled(prefix, style));
    spans.extend(coming_soon_label_spans(
        button.label,
        label_width,
        frame,
        style,
    ));
    spans.push(Span::styled("  ", style));
    spans.push(Span::styled(trailing, trailing_style));
    ListItem::new(full_width_spans(spans, width, style))
}

fn toolbar_trailing_style(row_style: Style) -> Style {
    Style::default()
        .fg(PATH_FG)
        .bg(row_style.bg.unwrap_or(BG_BASE))
        .remove_modifier(Modifier::BOLD)
}

fn chrome_button_item(
    button: &ToolbarButton,
    width: usize,
    hovered: bool,
    active: bool,
) -> ListItem<'static> {
    let style = chrome_button_style(hovered, active);
    ListItem::new(row_with_trailing_slot(
        row_prefix(" ", Some(button.icon)),
        button.label,
        format_trailing_slot(button.shortcut),
        width,
        style,
        toolbar_trailing_style(style),
    ))
}

#[derive(Clone, Copy)]
enum ChromeSectionAnchor {
    Top,
    Bottom,
}

fn fill_chrome_section_padding(frame: &mut Frame, section_area: Rect) {
    if section_area.height == 0 || TOOLBAR_SECTION_PAD == 0 {
        return;
    }
    let pad_style = Style::default().bg(BG_BASE);
    frame.render_widget(
        Block::default().style(pad_style),
        Rect {
            x: section_area.x,
            y: section_area.y,
            width: section_area.width,
            height: TOOLBAR_SECTION_PAD,
        },
    );
    frame.render_widget(
        Block::default().style(pad_style),
        Rect {
            x: section_area.x,
            y: section_area
                .y
                .saturating_add(section_area.height)
                .saturating_sub(TOOLBAR_SECTION_PAD),
            width: section_area.width,
            height: TOOLBAR_SECTION_PAD,
        },
    );
}

fn render_chrome_section(
    frame: &mut Frame,
    pane_area: Rect,
    section_area: Rect,
    session_inner: Rect,
    button_rows: u16,
    anchor: ChromeSectionAnchor,
    items: Vec<ListItem<'static>>,
    row_backdrops: &[Option<Color>],
) {
    if section_area.height == 0 {
        return;
    }
    fill_chrome_section_padding(frame, section_area);
    let button_height = section_area
        .height
        .saturating_sub(TOOLBAR_SECTION_PAD * 2)
        .min(button_rows);
    if button_height == 0 {
        return;
    }
    let row_y = match anchor {
        ChromeSectionAnchor::Top => section_area.y.saturating_add(TOOLBAR_SECTION_PAD),
        ChromeSectionAnchor::Bottom => section_area
            .y
            .saturating_add(section_area.height)
            .saturating_sub(TOOLBAR_SECTION_PAD)
            .saturating_sub(button_height),
    };
    for (row_idx, bg) in row_backdrops.iter().enumerate() {
        if let Some(bg) = bg {
            render_full_width_row_backdrop(
                frame,
                pane_area,
                row_y.saturating_add(row_idx as u16),
                *bg,
            );
        }
    }
    let row_area = Rect {
        x: session_inner.x,
        y: row_y,
        width: session_inner.width,
        height: button_height,
    };
    frame.render_widget(
        List::new(items).style(Style::default().bg(BG_BASE).remove_modifier(Modifier::BOLD)),
        row_area,
    );
}

pub(crate) fn render_toolbar(
    frame: &mut Frame,
    pane_area: Rect,
    toolbar_area: Rect,
    session_inner: Rect,
    toolbar_hover: Option<ToolbarAction>,
    workspace_new_session_open: bool,
    workspace_automations_open: bool,
    workspace_mcps_open: bool,
    workspace_skills_open: bool,
    coming_soon_frames: &[(ToolbarAction, usize)],
) {
    let width = session_inner.width as usize;
    let button_height = toolbar_area
        .height
        .saturating_sub(TOOLBAR_SECTION_PAD * 2)
        .min(TOOLBAR_BUTTON_ROWS);
    let buttons: Vec<_> = TOOLBAR_BUTTONS
        .iter()
        .take(button_height as usize)
        .collect();
    let mut row_backdrops = Vec::with_capacity(buttons.len());
    let items: Vec<ListItem> = buttons
        .iter()
        .map(|button| {
            let hovered = toolbar_hover == Some(button.action);
            // Selected backdrop follows the open workspace panel (right-hand content).
            let active = toolbar_action_is_active(
                button.action,
                workspace_new_session_open,
                workspace_automations_open,
                workspace_mcps_open,
                workspace_skills_open,
            );
            let anim_frame = coming_soon_frame_for(coming_soon_frames, button.action);
            if let Some(frame) = anim_frame {
                row_backdrops.push(chrome_coming_soon_style(hovered, active, true).bg);
                toolbar_coming_soon_item(button, width, hovered, active, true, frame)
            } else {
                row_backdrops.push(chrome_row_backdrop_bg(hovered, active));
                chrome_button_item(button, width, hovered, active)
            }
        })
        .collect();
    render_chrome_section(
        frame,
        pane_area,
        toolbar_area,
        session_inner,
        TOOLBAR_BUTTON_ROWS,
        ChromeSectionAnchor::Top,
        items,
        &row_backdrops,
    );
}

pub(crate) fn render_bottom_chrome(
    frame: &mut Frame,
    pane_area: Rect,
    settings_area: Rect,
    session_inner: Rect,
    settings_hover: bool,
    settings_active: bool,
    leave_hover: bool,
) {
    let width = session_inner.width as usize;
    let row_backdrops = [
        chrome_row_backdrop_bg(settings_hover, settings_active),
        chrome_row_backdrop_bg(leave_hover, false),
    ];
    let items = vec![
        chrome_button_item(&SETTINGS_BUTTON, width, settings_hover, settings_active),
        chrome_button_item(&LEAVE_BUTTON, width, leave_hover, false),
    ];
    render_chrome_section(
        frame,
        pane_area,
        settings_area,
        session_inner,
        BOTTOM_CHROME_ROWS,
        ChromeSectionAnchor::Bottom,
        items,
        &row_backdrops,
    );
}

pub fn render_workspace_scrim(frame: &mut Frame, area: Rect, lines: &[String]) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let style = Style::default()
        .fg(WORKSPACE_SCRIM_FG)
        .bg(WORKSPACE_SCRIM_BG)
        .add_modifier(Modifier::DIM);
    let width = area.width as usize;
    for row in 0..area.height as usize {
        let y = area.y.saturating_add(row as u16);
        let line = lines.get(row).map(String::as_str).unwrap_or("");
        let display = truncate(line, width);
        let pad = width.saturating_sub(display.chars().count());
        let spans = vec![
            Span::styled(display, style),
            Span::styled(" ".repeat(pad), style),
        ];
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
        );
    }
}

fn truncate_label(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        return label.to_string();
    }
    let mut out = String::new();
    for (idx, ch) in label.chars().enumerate() {
        if idx + 1 >= max_chars.saturating_sub(1) {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn update_action_row(label: &str, icon: &str, width: usize, hovered: bool) -> ListItem<'static> {
    let style = if hovered {
        Style::default().fg(TEXT_SELECTED).bg(BG_HIGHLIGHT)
    } else {
        Style::default().fg(TEXT_SECONDARY).bg(BG_PANEL)
    };
    ListItem::new(row_with_trailing_slot(
        row_prefix(" ", Some(icon)),
        label,
        String::new(),
        width,
        style,
        style,
    ))
}

pub(crate) fn render_update_box(
    frame: &mut Frame,
    pane_area: Rect,
    banner: &UpdateBannerView,
    box_rect: Rect,
    upgrade_hover: bool,
    dismiss_hover: bool,
) {
    if box_rect.width == 0 || box_rect.height == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(BG_PANEL)),
        box_rect,
    );

    let accent = if banner.critical {
        WARM_ACCENT
    } else {
        DONE_GREEN
    };
    let prefix = if banner.critical { "!" } else { "↑" };
    let width = box_rect.width as usize;
    let rows = UPDATE_BOX_ROWS.min(box_rect.height);

    if rows > 0 {
        let message_rect = Rect {
            x: box_rect.x,
            y: box_rect.y,
            width: box_rect.width,
            height: 1,
        };
        let max_label = box_rect.width.saturating_sub(4) as usize;
        let message = truncate_label(&banner.label, max_label);
        let line = Line::from(vec![
            Span::styled(
                format!(" {prefix} "),
                Style::default()
                    .fg(accent)
                    .bg(BG_PANEL)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(message, Style::default().fg(TEXT_PRIMARY).bg(BG_PANEL)),
        ]);
        frame.render_widget(Paragraph::new(line), message_rect);
    }

    if rows > 1 {
        if upgrade_hover {
            render_full_width_row_backdrop(
                frame,
                pane_area,
                box_rect.y.saturating_add(1),
                BG_HIGHLIGHT,
            );
        }
        let upgrade_rect = Rect {
            x: box_rect.x,
            y: box_rect.y.saturating_add(1),
            width: box_rect.width,
            height: 1,
        };
        frame.render_widget(
            List::new(vec![update_action_row("Update", "↑", width, upgrade_hover)])
                .style(Style::default().bg(BG_PANEL)),
            upgrade_rect,
        );
    }
    if rows > 2 {
        if dismiss_hover {
            render_full_width_row_backdrop(
                frame,
                pane_area,
                box_rect.y.saturating_add(2),
                BG_HIGHLIGHT,
            );
        }
        let dismiss_rect = Rect {
            x: box_rect.x,
            y: box_rect.y.saturating_add(2),
            width: box_rect.width,
            height: 1,
        };
        frame.render_widget(
            List::new(vec![update_action_row(
                "Remind me later",
                " ",
                width,
                dismiss_hover,
            )])
            .style(Style::default().bg(BG_PANEL)),
            dismiss_rect,
        );
    }
}

pub fn update_banner_action_from_mouse(
    y: u16,
    metrics: &LayoutMetrics,
) -> Option<UpdateBannerAction> {
    if metrics.update_banner_row_count == 0 {
        return None;
    }
    let top = metrics.update_banner_top_y;
    let bottom = top.saturating_add(metrics.update_banner_row_count);
    if y < top || y >= bottom {
        return None;
    }
    match y.saturating_sub(top) {
        1 => Some(UpdateBannerAction::Upgrade),
        2 => Some(UpdateBannerAction::Dismiss),
        _ => None,
    }
}

pub fn update_banner_hover_from_mouse(y: u16, metrics: &LayoutMetrics) -> (bool, bool) {
    match update_banner_action_from_mouse(y, metrics) {
        Some(UpdateBannerAction::Upgrade) => (true, false),
        Some(UpdateBannerAction::Dismiss) => (false, true),
        None => (false, false),
    }
}

pub fn toolbar_action_from_mouse(y: u16, metrics: &LayoutMetrics) -> Option<ToolbarAction> {
    if y < metrics.toolbar_top_y {
        return None;
    }
    let rel = y.saturating_sub(metrics.toolbar_top_y);
    if rel >= metrics.toolbar_row_count {
        return None;
    }
    TOOLBAR_BUTTONS
        .get(rel as usize)
        .map(|button| button.action)
}

pub fn settings_action_from_mouse(y: u16, metrics: &LayoutMetrics) -> bool {
    if y < metrics.settings_top_y {
        return false;
    }
    y.saturating_sub(metrics.settings_top_y) < metrics.settings_row_count
}

pub fn leave_action_from_mouse(y: u16, metrics: &LayoutMetrics) -> bool {
    if y < metrics.leave_top_y {
        return false;
    }
    y.saturating_sub(metrics.leave_top_y) < metrics.leave_row_count
}
