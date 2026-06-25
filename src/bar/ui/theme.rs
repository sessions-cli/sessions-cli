//! Sidebar color palette, animation timing, and chrome constants.

use ratatui::style::Color;

pub const BG_BASE: Color = Color::Rgb(0, 0, 0);
pub const BG_PANEL: Color = Color::Rgb(7, 7, 7);
pub const BG_HIGHLIGHT: Color = Color::Rgb(20, 20, 20);
/// Pwd group being dragged — lifted from base so the source reads as "picked up".
pub const BG_DRAG_SOURCE: Color = Color::Rgb(32, 32, 38);
/// Pwd group drop target while shuffling — distinct from row hover/selection.
pub const BG_DRAG_TARGET: Color = Color::Rgb(42, 54, 78);
/// Selected sidebar row — lighter backdrop so white text pops against black rows.
pub const BG_SELECTED: Color = Color::Rgb(58, 58, 62);
/// Selected row while the pointer is also over it — slightly brighter than selection alone.
pub const BG_HOVER_SELECTED: Color = Color::Rgb(72, 72, 76);
/// Default foreground for dimmed / fallback spans.
pub const TEXT_PRIMARY: Color = Color::Rgb(176, 176, 176);
/// Session row label — soft off-white (~90% brightness); selection is backdrop-only.
pub const TEXT_SELECTED: Color = Color::Rgb(223, 223, 223);
pub const TEXT_SECONDARY: Color = Color::Rgb(153, 153, 153);
pub const PATH_FG: Color = Color::Rgb(110, 110, 110);
/// Dim scrim over the live workspace pane behind the new-session panel.
pub const WORKSPACE_SCRIM_BG: Color = Color::Rgb(0, 0, 0);
pub const WORKSPACE_SCRIM_FG: Color = Color::Rgb(82, 82, 82);
/// Show more/less — between path metadata and group path headers.
pub const GROUP_TOGGLE_FG: Color = Color::Rgb(131, 131, 131);
pub const WORKING_BG: Color = Color::Rgb(24, 74, 160);
/// Light mint accent for completed-thread highlight (#bbf7d0).
pub const DONE_GREEN: Color = Color::Rgb(187, 247, 208);
/// Backward-compat alias for [`DONE_GREEN`].
pub const GROK_GREEN: Color = DONE_GREEN;
/// Rich emerald for the completion ■ in the trailing badge.
pub const DONE_FG: Color = Color::Rgb(22, 163, 74);
pub const APPROVAL_BG: Color = Color::Rgb(160, 108, 24);
pub const ERROR_BG: Color = Color::Rgb(140, 38, 38);
pub const ACTIVE_BG: Color = Color::Rgb(34, 34, 34);
pub const WARM_ACCENT: Color = Color::Rgb(255, 196, 92);
/// Active inline rename row — warm tint distinct from selection/hover.
pub const RENAME_EDIT_BG: Color = Color::Rgb(38, 32, 18);
pub const RENAME_EDIT_FG: Color = Color::Rgb(255, 210, 120);
/// Inverted title while the whole rename buffer is selected (replace-on-type).
pub const RENAME_SELECT_FG: Color = RENAME_EDIT_BG;
pub const RENAME_SELECT_BG: Color = RENAME_EDIT_FG;
pub(crate) const NOTEPAD_EDIT_BG: Color = BG_HIGHLIGHT;
pub(crate) const NOTEPAD_EDIT_FG: Color = TEXT_SELECTED;
/// Expanded todo with a linked session — shared backdrop across title, body, and session row.
pub(crate) const TODO_LINKED_BG: Color = BG_PANEL;
pub(crate) const NOTEPAD_SELECT_FG: Color = NOTEPAD_EDIT_BG;
pub(crate) const NOTEPAD_SELECT_BG: Color = TEXT_SELECTED;
pub const RENAME_SAVED_FG: Color = Color::Rgb(134, 239, 172);
/// Sessions block title — chrome only, darker than path/time metadata.
pub const BRAND_FG: Color = Color::Rgb(76, 76, 76);
pub const CLOSE_HOVER_BG: Color = Color::Rgb(92, 18, 18);
pub const CLOSE_HOVER_FG: Color = Color::Rgb(255, 96, 96);
pub const CLOSE_MODE_FG: Color = Color::Rgb(118, 118, 118);
/// Braille dots13 — Grok run spinner (cli-spinners).
pub const RUN_SPINNER_FRAMES: [&str; 8] = ["⣼", "⣹", "⢻", "⠿", "⡟", "⣏", "⣧", "⣶"];
pub const RUN_SPINNER_INTERVAL_MS: u64 = 200;
pub const COMING_SOON_INTERVAL_MS: u64 = 75;
pub const COMING_SOON_GLITCH_FRAMES: usize = 3;
pub const COMING_SOON_DECODE_FRAMES: usize = 7;
pub const COMING_SOON_HOLD_FRAMES: usize = 28;
pub const COMING_SOON_RESTORE_FRAMES: usize = 7;
pub const COMING_SOON_CYCLE_FRAMES: usize = COMING_SOON_GLITCH_FRAMES
    + COMING_SOON_DECODE_FRAMES
    + COMING_SOON_HOLD_FRAMES
    + COMING_SOON_RESTORE_FRAMES;
pub const COMING_SOON_CYCLE_MS: u64 = COMING_SOON_CYCLE_FRAMES as u64 * COMING_SOON_INTERVAL_MS;
pub(crate) const COMING_SOON_TARGET: &str = "Coming soon";
pub(crate) const COMING_SOON_HOLD_PLAIN_FRAMES: usize = 6;
pub(crate) const COMING_SOON_DOT_STEP_FRAMES: usize = 7;
pub(crate) const COMING_SOON_TINT: Color = Color::Rgb(8, 8, 8);
pub(crate) const COMING_SOON_PULSE: Color = Color::Rgb(20, 20, 20);
pub(crate) const COMING_SOON_BRAILLE: [char; 16] = [
    '⠁', '⠃', '⠇', '⠏', '⠟', '⠿', '⡀', '⡆', '⡿', '⣀', '⣇', '⣟', '⣿', '⢀', '⢿', '⣻',
];