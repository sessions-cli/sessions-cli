//! Splash art for the New Chat pane.
//!
//! Direct Kitty: PNG pixels via the graphics protocol (bypasses the cell grid).
//! Everywhere else: braille waves from the PNG, solid blocks for the icon, and crisp
//! `sessions` text overlaid below the brand mark.

use crate::bar::art_encode::{self, ArtCellRatatui};
use crate::bar::ui::BG_BASE;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::StatefulImage;
use std::io::Cursor;
use std::process::Command;
use std::sync::Mutex;
use std::sync::OnceLock;

const SPLASH_PNG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/splash.png"));
/// Cap RGB payload so first-frame encode stays fast on wide workspace panes.
const MAX_ENCODE_PIXEL_WIDTH: u32 = 1_200;

/// Layout reference for an 80-col pane at 4/6 width with 8×16px cells.
pub const REFERENCE_PANE_COLS: u16 = 80;
pub const REFERENCE_CELL: (u16, u16) = (8, 16);
pub const REFERENCE_ART_COLS: u16 = 53;
pub const REFERENCE_ART_ROWS: u16 = 15;

#[derive(Clone, Copy, PartialEq, Eq)]
struct ArtFingerprint {
    width: u16,
    height: u16,
    cell_w: u16,
    cell_h: u16,
}

#[derive(Default)]
pub struct ArtCanvasState {
    protocol: Option<StatefulProtocol>,
    cells: Option<Vec<Vec<ArtCellRatatui>>>,
    mode: ArtRenderMode,
    cached_fingerprint: Option<ArtFingerprint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ArtRenderMode {
    #[default]
    None,
    /// Pixel graphics — only when attached directly to Kitty (not via tmux).
    Kitty,
    /// Braille / block Unicode art decoded from the PNG at canvas size.
    UnicodeArt,
}

/// Art spans 4/6 of the pane width, centered horizontally.
pub fn pane_fraction_width(pane_width: u16) -> u16 {
    ((pane_width as u32 * 4) / 6).max(1) as u16
}

/// Centered content column — same width as the new-session form.
pub fn panel_column_rect(pane: Rect) -> Rect {
    let width = pane_fraction_width(pane.width);
    let x = pane.x + pane.width.saturating_sub(width) / 2;
    Rect {
        x,
        y: pane.y,
        width,
        height: pane.height,
    }
}

pub fn art_canvas_rect(pane: Rect) -> Rect {
    art_canvas_rect_with_cell(pane, terminal_cell_size())
}

fn art_canvas_rect_with_cell(pane: Rect, cell: (u16, u16)) -> Rect {
    let width = pane_fraction_width(pane.width);
    let height = art_rows_for_width(width, cell);
    let x = pane.x + pane.width.saturating_sub(width) / 2;
    Rect {
        x,
        y: pane.y,
        width,
        height,
    }
}

pub fn canonical_art_dimensions(pane: Rect) -> (u16, u16) {
    let rect = art_canvas_rect(pane);
    (rect.width, rect.height)
}

/// True for direct Kitty graphics — skip repainting this rect so ratatui does not erase pixels.
pub fn uses_terminal_graphics(state: &ArtCanvasState) -> bool {
    state.mode == ArtRenderMode::Kitty && state.protocol.is_some()
}

/// Drop cached protocol/graphics so the canvas can be rebuilt (e.g. on resize).
pub fn reset_art_canvas(state: &mut ArtCanvasState) {
    *state = ArtCanvasState::default();
}

/// Clear cached terminal cell metrics (call on SIGWINCH / pane resize).
pub fn invalidate_terminal_cell_cache() {
    if let Some(cache) = TERMINAL_CELL_CACHE.get() {
        *cache.lock().expect("terminal cell cache") = None;
    }
}

fn art_fingerprint(pane: Rect) -> ArtFingerprint {
    let cell = terminal_cell_size();
    let rect = art_canvas_rect_with_cell(pane, cell);
    ArtFingerprint {
        width: rect.width,
        height: rect.height,
        cell_w: cell.0,
        cell_h: cell.1,
    }
}

/// True when art was never built or pane/cell dimensions no longer match the cache.
pub fn art_needs_rebuild(state: &ArtCanvasState, pane: Rect) -> bool {
    state.mode == ArtRenderMode::None || state.cached_fingerprint != Some(art_fingerprint(pane))
}

/// Build or refresh cached art when pane size or terminal cell metrics change.
pub fn ensure_art_canvas(state: &mut ArtCanvasState, pane: Rect) {
    if !art_needs_rebuild(state, pane) {
        return;
    }
    reset_art_canvas(state);
    init_art_canvas(state, pane);
}

/// Encode splash art for the current pane geometry.
pub fn init_art_canvas(state: &mut ArtCanvasState, pane: Rect) {
    let img = match ImageReader::new(Cursor::new(SPLASH_PNG))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.decode().ok())
    {
        Some(img) => img,
        None => return,
    };

    let cell = terminal_cell_size();
    let art = art_canvas_rect(pane);

    if direct_kitty_graphics_available() {
        let img = resize_for_canvas(img, art, cell);
        let picker = build_picker(cell, ProtocolType::Kitty);
        state.protocol = Some(picker.new_resize_protocol(img));
        state.mode = ArtRenderMode::Kitty;
    } else {
        state.cells = Some(art_encode::encode_mixed_art(&img, art.width, art.height));
        state.mode = ArtRenderMode::UnicodeArt;
    }
    state.cached_fingerprint = Some(ArtFingerprint {
        width: art.width,
        height: art.height,
        cell_w: cell.0,
        cell_h: cell.1,
    });
}

pub fn render_art_canvas(frame: &mut ratatui::Frame, area: Rect, state: &mut ArtCanvasState) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    if let Some(protocol) = &mut state.protocol {
        let widget = StatefulImage::default();
        frame.render_stateful_widget(widget, area, protocol);
        if let Some(Err(_)) = protocol.last_encoding_result() {
            state.protocol = None;
            state.mode = ArtRenderMode::None;
            state.cells = None;
            state.cached_fingerprint = None;
        }
        return;
    }

    if let Some(cells) = &state.cells {
        render_unicode_art(frame, area, cells);
    }
}

fn render_unicode_art(frame: &mut ratatui::Frame, area: Rect, cells: &[Vec<ArtCellRatatui>]) {
    let rows = area.height as usize;
    let cols = area.width as usize;
    let lines: Vec<Line> = (0..rows)
        .map(|row| {
            let src = cells.get(row);
            let mut spans = Vec::new();
            for col in 0..cols {
                let cell = src.and_then(|r| r.get(col));
                let (ch, fg) = cell
                    .map(|c| (c.ch, c.fg))
                    .unwrap_or((' ', Color::Rgb(0, 0, 0)));
                if ch == ' ' {
                    spans.push(Span::styled(" ".to_string(), Style::default().bg(BG_BASE)));
                } else {
                    spans.push(Span::styled(
                        ch.to_string(),
                        Style::default().fg(fg).bg(BG_BASE),
                    ));
                }
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn art_rows_for_width(width_cols: u16, cell: (u16, u16)) -> u16 {
    let (cell_w, cell_h) = cell;
    let pixel_w = (width_cols as u32).saturating_mul(cell_w as u32).max(1);
    // assets/splash.png is 1672×941 (landscape logo).
    let pixel_h = pixel_w.saturating_mul(941) / 1672;
    let rows = pixel_h.div_ceil(cell_h as u32).max(1) as u16;
    rows.clamp(1, 24)
}

/// Kitty pixel graphics only when not nested in tmux (passthrough is unreliable).
fn direct_kitty_graphics_available() -> bool {
    if std::env::var("TMUX").is_ok() {
        return false;
    }
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return true;
    }
    std::env::var("TERM").is_ok_and(|term| term.contains("kitty") || term.contains("xterm-kitty"))
}

fn build_picker(cell: (u16, u16), protocol: ProtocolType) -> Picker {
    let mut picker = Picker::from_fontsize(cell);
    picker.set_protocol_type(protocol);
    picker
}

static TERMINAL_CELL_CACHE: OnceLock<Mutex<Option<(u16, u16)>>> = OnceLock::new();

fn terminal_cell_size() -> (u16, u16) {
    let cache = TERMINAL_CELL_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().expect("terminal cell cache");
    if let Some(size) = *guard {
        return size;
    }
    let size = probe_terminal_cell_size();
    *guard = Some(size);
    size
}

fn probe_terminal_cell_size() -> (u16, u16) {
    if std::env::var("TMUX").is_ok() {
        if let Some(size) = tmux_client_cell_size() {
            return size;
        }
    }
    winsize_cell_size().unwrap_or((8, 16))
}

fn tmux_client_cell_size() -> Option<(u16, u16)> {
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "#{client_cell_width} #{client_cell_height}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace();
    let width: u16 = parts.next()?.parse().ok()?;
    let height: u16 = parts.next()?.parse().ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

#[cfg(unix)]
fn winsize_cell_size() -> Option<(u16, u16)> {
    use std::os::unix::io::AsRawFd;

    #[repr(C)]
    struct Winsize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }

    const TIOCGWINSZ: libc::c_ulong = 0x4008_7468;

    let mut winsize = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe {
        libc::ioctl(
            std::io::stdout().as_raw_fd(),
            TIOCGWINSZ,
            &mut winsize as *mut Winsize,
        )
    };
    if rc == -1 || winsize.ws_col == 0 || winsize.ws_row == 0 {
        return None;
    }
    let w = winsize.ws_xpixel / winsize.ws_col;
    let h = winsize.ws_ypixel / winsize.ws_row;
    (w > 0 && h > 0).then_some((w, h))
}

#[cfg(not(unix))]
fn winsize_cell_size() -> Option<(u16, u16)> {
    None
}

fn resize_for_canvas(img: DynamicImage, art: Rect, cell: (u16, u16)) -> DynamicImage {
    let (cell_w, cell_h) = cell;
    let mut target_w = (art.width as u32).saturating_mul(cell_w as u32).max(1);
    let mut target_h = (art.height as u32).saturating_mul(cell_h as u32).max(1);
    if target_w > MAX_ENCODE_PIXEL_WIDTH {
        target_h = target_h.saturating_mul(MAX_ENCODE_PIXEL_WIDTH) / target_w.max(1);
        target_w = MAX_ENCODE_PIXEL_WIDTH;
    }
    if img.width() == target_w && img.height() == target_h {
        return img;
    }
    img.resize_exact(target_w, target_h, FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_width_is_four_sixths_of_pane() {
        assert_eq!(pane_fraction_width(80), 53);
        assert_eq!(pane_fraction_width(114), 76);
        assert_eq!(pane_fraction_width(6), 4);
    }

    #[test]
    fn reference_canvas_matches_runtime_bounds() {
        let pane = Rect::new(0, 0, REFERENCE_PANE_COLS, 40);
        let rect = art_canvas_rect_with_cell(pane, REFERENCE_CELL);
        assert_eq!(rect.width, REFERENCE_ART_COLS);
        assert_eq!(rect.height, REFERENCE_ART_ROWS);
        assert_eq!(rect.x, 13);
    }

    #[test]
    fn resize_targets_cell_pixel_dimensions() {
        let img = DynamicImage::new_rgb8(100, 50);
        let art = Rect::new(0, 0, 10, 5);
        let resized = resize_for_canvas(img, art, (8, 16));
        assert_eq!(resized.width(), 80);
        assert_eq!(resized.height(), 80);
    }

    #[test]
    fn resize_caps_encode_pixels_on_wide_panes() {
        let img = DynamicImage::new_rgb8(100, 50);
        let art = Rect::new(0, 0, 200, 40);
        let resized = resize_for_canvas(img, art, (19, 35));
        assert_eq!(resized.width(), MAX_ENCODE_PIXEL_WIDTH);
    }

    #[test]
    fn kitty_disabled_inside_tmux() {
        std::env::set_var("TMUX", "1");
        std::env::set_var("TERM", "tmux-256color");
        assert!(!direct_kitty_graphics_available());
        std::env::remove_var("TMUX");
        std::env::remove_var("TERM");
    }

    #[test]
    fn art_needs_rebuild_when_pane_width_changes() {
        let pane_a = Rect::new(0, 0, 80, 40);
        let mut state = ArtCanvasState::default();
        init_art_canvas(&mut state, pane_a);
        assert!(!art_needs_rebuild(&state, pane_a));

        let pane_b = Rect::new(0, 0, 114, 40);
        assert!(art_needs_rebuild(&state, pane_b));
    }

    #[test]
    fn ensure_art_canvas_skips_work_when_fingerprint_matches() {
        let pane = Rect::new(0, 0, 80, 40);
        let mut state = ArtCanvasState::default();
        ensure_art_canvas(&mut state, pane);
        let cells = state.cells.clone();
        ensure_art_canvas(&mut state, pane);
        assert_eq!(state.cells, cells);
    }

    #[test]
    fn splash_png_encodes_to_unicode_grid() {
        let img = ImageReader::new(Cursor::new(SPLASH_PNG))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        let pane = Rect::new(0, 0, REFERENCE_PANE_COLS, 40);
        let rect = art_canvas_rect_with_cell(pane, REFERENCE_CELL);
        let grid = art_encode::encode_mixed_art(&img, rect.width, rect.height);
        assert!(!grid.is_empty());
        let text_row = (f32::from(rect.height) * art_encode::TEXT_ROW_FRAC).round() as usize;
        let line: String = grid[text_row].iter().map(|c| c.ch).collect();
        assert!(line.contains("sessions"));
    }
}
