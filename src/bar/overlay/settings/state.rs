//! Settings overlay state and row model.

use crate::bar::art_canvas;
use crate::bar::group_order::MAX_THREADS_PER_GROUP;
use crate::config::Config;
use crate::hooks;
use crate::telemetry::config::SessionsConfig;
use crate::version::VERSION;
use crossterm::event::{self, KeyCode, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::block::Padding;
use ratatui::widgets::{Block, Borders};
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Section,
    Config,
    Toggle,
    Action,
    Shortcut,
}

pub struct SettingsRow {
    pub kind: RowKind,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct PanelTargets {
    pub cta: Rect,
    pub close: Rect,
    pub row_rects: Vec<(usize, Rect)>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PanelHover {
    pub cta: bool,
    pub close: bool,
    pub row: Option<usize>,
}

pub const CTA_BUTTON_LABEL: &str = "Close";
pub const CTA_BUTTON_WIDTH: u16 = 14;
pub const CTA_BUTTON_ROWS: u16 = 3;
pub const CLOSE_BUTTON_COLS: u16 = 3;
pub const CLOSE_BUTTON_LABEL: &str = "×";
pub const TITLE_ROWS: u16 = 2;
pub const HEADER_LIST_GAP: u16 = 1;
pub const CTA_TOP_GAP: u16 = 1;
pub const HINT_TOP_GAP: u16 = 2;
pub const HINT_ROWS: u16 = 1;
const SECTION_GAP: u16 = 2;
pub const SECTION_TAIL_GAP: u16 = 1;
pub const OVERLAY_BORDER_FG: Color = Color::Rgb(56, 56, 56);
pub const OVERLAY_MIN_WIDTH: u16 = 40;
pub const OVERLAY_MIN_HEIGHT: u16 = 10;
pub const OVERLAY_TITLE_ROWS: u16 = 1;
pub const OVERLAY_HINT_ROWS: u16 = 1;
pub const OVERLAY_PAD_LEFT: u16 = 3;
pub const OVERLAY_PAD_RIGHT: u16 = 3;
pub const OVERLAY_PAD_TOP: u16 = 2;
pub const OVERLAY_PAD_BOTTOM: u16 = 2;

pub fn overlay_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .padding(Padding::new(
            OVERLAY_PAD_LEFT,
            OVERLAY_PAD_RIGHT,
            OVERLAY_PAD_TOP,
            OVERLAY_PAD_BOTTOM,
        ))
}

#[derive(Debug, Clone)]
pub enum OverlayMsg {
    Line(String),
    Finished { success: bool },
}

#[derive(Debug, Clone)]
pub struct SettingsOverlay {
    pub title: String,
    pub lines: Vec<String>,
    pub running: bool,
    pub finished: bool,
    pub success: Option<bool>,
    pub scroll: usize,
}

impl SettingsOverlay {
    fn info(title: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            lines,
            running: false,
            finished: true,
            success: Some(true),
            scroll: 0,
        }
    }

    pub(crate) fn running(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lines: vec!["Starting…".into()],
            running: true,
            finished: false,
            success: None,
            scroll: 0,
        }
    }

    pub(crate) fn apply(&mut self, msg: OverlayMsg) {
        match msg {
            OverlayMsg::Line(line) => {
                if self.lines.len() == 1 && self.lines[0] == "Starting…" {
                    self.lines.clear();
                }
                self.lines.push(line);
            }
            OverlayMsg::Finished { success } => {
                self.running = false;
                self.finished = true;
                self.success = Some(success);
                if self.lines.is_empty() {
                    self.lines.push(if success {
                        "Done.".into()
                    } else {
                        "Failed.".into()
                    });
                }
            }
        }
    }

    pub fn hint(&self) -> &'static str {
        if self.running {
            "Running…"
        } else {
            "Esc close"
        }
    }

    pub fn visible_body_rows(&self, inner_height: u16) -> usize {
        inner_height
            .saturating_sub(OVERLAY_TITLE_ROWS + OVERLAY_HINT_ROWS)
            .max(1) as usize
    }

    pub fn max_scroll(&self, inner_height: u16) -> usize {
        let visible = self.visible_body_rows(inner_height);
        self.lines.len().saturating_sub(visible)
    }
}

pub struct SettingsLayout {
    pub header: Rect,
    pub header_list_gap: Rect,
    pub list: Rect,
    pub cta_top_gap: Rect,
    pub cta: Rect,
    pub hint_top_gap: Rect,
    pub hint: Rect,
}

pub fn list_line_count(rows: &[SettingsRow]) -> u16 {
    build_list_layout(rows).len() as u16
}

pub fn panel_content_height(rows: &[SettingsRow]) -> u16 {
    TITLE_ROWS
        + HEADER_LIST_GAP
        + list_line_count(rows)
        + CTA_TOP_GAP
        + CTA_BUTTON_ROWS
        + HINT_TOP_GAP
        + HINT_ROWS
}

pub fn settings_layout(inner: Rect, rows: &[SettingsRow]) -> SettingsLayout {
    let list_lines = list_line_count(rows);
    let desired = panel_content_height(rows);
    let content_y = if desired <= inner.height {
        inner.y + inner.height.saturating_sub(desired) / 2
    } else {
        inner.y
    };

    let mut y = content_y;
    let x = inner.x;
    let w = inner.width;

    let header = Rect {
        x,
        y,
        width: w,
        height: TITLE_ROWS,
    };
    y = y.saturating_add(TITLE_ROWS);
    let header_list_gap = Rect {
        x,
        y,
        width: w,
        height: HEADER_LIST_GAP,
    };
    y = y.saturating_add(HEADER_LIST_GAP);
    let list = Rect {
        x,
        y,
        width: w,
        height: list_lines,
    };
    y = y.saturating_add(list_lines);
    let cta_top_gap = Rect {
        x,
        y,
        width: w,
        height: CTA_TOP_GAP,
    };
    y = y.saturating_add(CTA_TOP_GAP);
    let cta_row = Rect {
        x,
        y,
        width: w,
        height: CTA_BUTTON_ROWS,
    };
    let cta_x = cta_row
        .x
        .saturating_add(cta_row.width.saturating_sub(CTA_BUTTON_WIDTH) / 2);
    let cta = Rect {
        x: cta_x,
        y: cta_row.y,
        width: CTA_BUTTON_WIDTH.min(cta_row.width),
        height: cta_row.height,
    };
    y = y.saturating_add(CTA_BUTTON_ROWS);
    let hint_top_gap = Rect {
        x,
        y,
        width: w,
        height: HINT_TOP_GAP,
    };
    y = y.saturating_add(HINT_TOP_GAP);
    let hint = Rect {
        x,
        y,
        width: w,
        height: HINT_ROWS,
    };

    SettingsLayout {
        header,
        header_list_gap,
        list,
        cta_top_gap,
        cta,
        hint_top_gap,
        hint,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListLine {
    Gap,
    Row(usize),
}

pub fn build_list_layout(rows: &[SettingsRow]) -> Vec<ListLine> {
    let mut lines = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        if row.kind == RowKind::Section {
            if idx > 0 {
                for _ in 0..SECTION_GAP {
                    lines.push(ListLine::Gap);
                }
            }
            lines.push(ListLine::Row(idx));
            for _ in 0..SECTION_TAIL_GAP {
                lines.push(ListLine::Gap);
            }
        } else {
            lines.push(ListLine::Row(idx));
        }
    }
    lines
}
fn truncate_detail(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx + 1 >= max_chars.saturating_sub(1) {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub fn update_rows(home: &std::path::Path) -> Vec<SettingsRow> {
    let mut rows = vec![SettingsRow {
        kind: RowKind::Config,
        label: "Installed version".into(),
        detail: VERSION.to_string(),
    }];
    let cfg = SessionsConfig::load(home).unwrap_or_default();
    if let Some(info) = cfg.cached_update() {
        if let Some(version) = info.available_version {
            rows.push(SettingsRow {
                kind: RowKind::Config,
                label: "Available".into(),
                detail: format!("{version} ({})", info.urgency.as_str()),
            });
        }
        if !info.message.is_empty() {
            rows.push(SettingsRow {
                kind: RowKind::Config,
                label: "Release notes".into(),
                detail: truncate_detail(&info.message, 42),
            });
        }
        rows.push(SettingsRow {
            kind: RowKind::Action,
            label: "Install update".into(),
            detail: "↵".into(),
        });
    } else {
        rows.push(SettingsRow {
            kind: RowKind::Config,
            label: "Status".into(),
            detail: "Up to date".into(),
        });
    }
    rows
}

pub fn build_rows(config: &Config, hook_summary: &str) -> Vec<SettingsRow> {
    let mut rows = vec![
        SettingsRow {
            kind: RowKind::Section,
            label: "General".into(),
            detail: String::new(),
        },
        SettingsRow {
            kind: RowKind::Config,
            label: "Notes directory".into(),
            detail: config.sidebar_notepad_dir().display().to_string(),
        },
    ];
    rows.extend(update_rows(&config.home));
    rows.push(SettingsRow {
        kind: RowKind::Action,
        label: "Agent hooks".into(),
        detail: hook_summary.to_string(),
    });
    rows.extend([
        SettingsRow {
            kind: RowKind::Section,
            label: "Notifications".into(),
            detail: String::new(),
        },
        SettingsRow {
            kind: RowKind::Config,
            label: "Completion bell".into(),
            detail: "On".into(),
        },
        SettingsRow {
            kind: RowKind::Config,
            label: "Done highlight".into(),
            detail: "On".into(),
        },
        SettingsRow {
            kind: RowKind::Config,
            label: "Threads per group".into(),
            detail: MAX_THREADS_PER_GROUP.to_string(),
        },
        SettingsRow {
            kind: RowKind::Section,
            label: "Shortcuts".into(),
            detail: String::new(),
        },
    ]);

    for shortcut in SESSION_SHORTCUTS {
        rows.push(SettingsRow {
            kind: RowKind::Shortcut,
            label: shortcut.key.into(),
            detail: shortcut.desc.into(),
        });
    }

    rows
}

struct ShortcutEntry {
    key: &'static str,
    desc: &'static str,
}

const SESSION_SHORTCUTS: &[ShortcutEntry] = &[
    ShortcutEntry {
        key: "Enter",
        desc: "open session",
    },
    ShortcutEntry {
        key: "j / k",
        desc: "move selection",
    },
    ShortcutEntry {
        key: "1-9, 0, 11+",
        desc: "focus by number",
    },
    ShortcutEntry {
        key: "hold d",
        desc: "delete mode (sessions & notes)",
    },
    ShortcutEntry {
        key: "⌘N",
        desc: "new session (toolbar)",
    },
    ShortcutEntry {
        key: "⌘T",
        desc: "new raw terminal",
    },
    ShortcutEntry {
        key: "⌘G",
        desc: "new grok session",
    },
    ShortcutEntry {
        key: "⌘C",
        desc: "new claude session",
    },
    ShortcutEntry {
        key: "⌘X",
        desc: "new codex session",
    },
    ShortcutEntry {
        key: "⌘O",
        desc: "new opencode session",
    },
    ShortcutEntry {
        key: "⌘S",
        desc: "search (toolbar)",
    },
    ShortcutEntry {
        key: "⌘A",
        desc: "automations (toolbar)",
    },
    ShortcutEntry {
        key: "⌘M",
        desc: "MCPs (toolbar)",
    },
    ShortcutEntry {
        key: "⌘K",
        desc: "skills (toolbar)",
    },
    ShortcutEntry {
        key: "⌘,",
        desc: "toggle settings",
    },
    ShortcutEntry {
        key: "Ctrl+q",
        desc: "back to terminal",
    },
    ShortcutEntry {
        key: "Ctrl-g 1..9,0",
        desc: "tmux window focus",
    },
    ShortcutEntry {
        key: "Ctrl-g o",
        desc: "cycle panes",
    },
    ShortcutEntry {
        key: "Ctrl-g m",
        desc: "toggle mouse",
    },
    ShortcutEntry {
        key: "Esc",
        desc: "close settings",
    },
];

pub fn first_selectable(rows: &[SettingsRow]) -> usize {
    rows.iter()
        .position(|row| row.kind != RowKind::Section)
        .unwrap_or(0)
}

pub fn sync_panel_mouse_cursor(panel_hover: &PanelHover) {
    use crate::bar::mouse_cursor::{self, MouseCursorShape};
    let shape = if panel_hover.cta || panel_hover.close {
        MouseCursorShape::Pointer
    } else {
        MouseCursorShape::Default
    };
    let _ = mouse_cursor::set_mouse_cursor(shape);
}

pub fn row_opens_overlay(row: &SettingsRow) -> bool {
    matches!(
        row.label.as_str(),
        "Install update" | "Agent hooks" | "Release notes" | "Notes directory"
    )
}

fn release_notes_lines(home: &Path) -> Option<Vec<String>> {
    let cfg = SessionsConfig::load(home).ok()?;
    let info = cfg.cached_update()?;
    if info.message.is_empty() {
        return None;
    }
    Some(info.message.lines().map(str::to_string).collect())
}

fn format_hooks_lines(summary: &hooks::SetupSummary) -> Vec<String> {
    let mut lines = Vec::new();
    if summary.configured.is_empty() && summary.skipped.is_empty() && summary.failed.is_empty() {
        lines.push("No detected agents to configure.".into());
        return lines;
    }
    if !summary.configured.is_empty() {
        lines.push(format!("Configured: {}", summary.configured.join(", ")));
    }
    if !summary.skipped.is_empty() {
        lines.push(format!("Already set: {}", summary.skipped.join(", ")));
    }
    for (agent, err) in &summary.failed {
        lines.push(format!("{agent}: {err}"));
    }
    lines
}

fn pipe_reader_lines<R: io::Read + Send + 'static>(
    reader: Option<R>,
    tx: mpsc::Sender<OverlayMsg>,
) {
    let Some(reader) = reader else {
        return;
    };
    for line in BufReader::new(reader).lines().map_while(Result::ok) {
        let _ = tx.send(OverlayMsg::Line(line));
    }
}

fn spawn_upgrade_job(home: &Path) -> Receiver<OverlayMsg> {
    let (tx, rx) = mpsc::channel();
    let binary = crate::paths::resolve_binary(home);
    std::thread::spawn(move || {
        let child = Command::new(&binary)
            .arg("upgrade")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let Ok(mut child) = child else {
            let _ = tx.send(OverlayMsg::Line("Failed to start upgrade.".into()));
            let _ = tx.send(OverlayMsg::Finished { success: false });
            return;
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let tx_out = tx.clone();
        let tx_err = tx.clone();
        let out_thread = std::thread::spawn(move || pipe_reader_lines(stdout, tx_out));
        let err_thread = std::thread::spawn(move || pipe_reader_lines(stderr, tx_err));
        let success = child.wait().map(|status| status.success()).unwrap_or(false);
        let _ = out_thread.join();
        let _ = err_thread.join();
        let _ = tx.send(OverlayMsg::Finished { success });
    });
    rx
}

fn spawn_hooks_job(home: &Path) -> Receiver<OverlayMsg> {
    let (tx, rx) = mpsc::channel();
    let home = home.to_path_buf();
    std::thread::spawn(move || {
        let summary = hooks::setup_detected(&home);
        let success = summary.failed.is_empty();
        for line in format_hooks_lines(&summary) {
            let _ = tx.send(OverlayMsg::Line(line));
        }
        let _ = tx.send(OverlayMsg::Finished { success });
    });
    rx
}

pub fn open_row_overlay(
    config: &Config,
    row: &SettingsRow,
) -> Option<(SettingsOverlay, Receiver<OverlayMsg>)> {
    match row.label.as_str() {
        "Install update" => Some((SettingsOverlay::running("Upgrading sessions"), spawn_upgrade_job(&config.home))),
        "Agent hooks" => Some((SettingsOverlay::running("Agent hooks"), spawn_hooks_job(&config.home))),
        "Release notes" => {
            let lines = release_notes_lines(&config.home)?;
            Some((SettingsOverlay::info("Release notes", lines), mpsc::channel().1))
        }
        "Notes directory" => Some((
            SettingsOverlay::info("Notes directory", vec![config.sidebar_notepad_dir().display().to_string()]),
            mpsc::channel().1,
        )),
        _ => None,
    }
}
pub fn overlay_rect(pane: Rect, overlay: &SettingsOverlay) -> Rect {
    let width = art_canvas::pane_fraction_width(pane.width)
        .max(OVERLAY_MIN_WIDTH)
        .min(pane.width);
    let line_rows = overlay.lines.len().max(4) as u16;
    let height = line_rows
        .saturating_add(OVERLAY_TITLE_ROWS + OVERLAY_HINT_ROWS)
        .saturating_add(2 + OVERLAY_PAD_TOP + OVERLAY_PAD_BOTTOM)
        .max(OVERLAY_MIN_HEIGHT)
        .min(pane.height.saturating_sub(2));
    let x = pane.x + pane.width.saturating_sub(width) / 2;
    let y = pane.y + pane.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

pub fn truncate_overlay_line(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx + 1 >= max_chars.saturating_sub(1) {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub fn overlay_inner_height(pane: Rect, overlay: &SettingsOverlay) -> u16 {
    let rect = overlay_rect(pane, overlay);
    overlay_block().inner(rect).height
}

pub fn drain_overlay(rx: &Receiver<OverlayMsg>, overlay: &mut SettingsOverlay, pane: Rect) {
    while let Ok(msg) = rx.try_recv() {
        overlay.apply(msg);
    }
    overlay.scroll = overlay.max_scroll(overlay_inner_height(pane, overlay));
}

pub fn handle_overlay_key(
    overlay: &mut SettingsOverlay,
    key: event::KeyEvent,
    pane: Rect,
) -> bool {
    if key.kind == KeyEventKind::Release {
        return false;
    }
    if overlay.running {
        return true;
    }
    let inner_h = overlay_inner_height(pane, overlay);
    match key.code {
        KeyCode::Esc => true,
        KeyCode::Up | KeyCode::Char('k') => {
            overlay.scroll = overlay.scroll.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            overlay.scroll = (overlay.scroll + 1).min(overlay.max_scroll(inner_h));
            true
        }
        _ => true,
    }
}
pub fn list_row_rects(list_area: Rect, rows: &[SettingsRow]) -> Vec<(usize, Rect)> {
    let mut rects = Vec::new();
    let mut y = list_area.y;
    for line in build_list_layout(rows) {
        if y >= list_area.y.saturating_add(list_area.height) {
            break;
        }
        match line {
            ListLine::Gap => {}
            ListLine::Row(idx) if rows[idx].kind != RowKind::Section => {
                rects.push((
                    idx,
                    Rect {
                        x: list_area.x,
                        y,
                        width: list_area.width,
                        height: 1,
                    },
                ));
            }
            ListLine::Row(_) => {}
        }
        y = y.saturating_add(1);
    }
    rects
}

pub fn move_selection(rows: &[SettingsRow], selected: &mut usize, delta: i32) {
    let selectable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(idx, row)| (row.kind != RowKind::Section).then_some(idx))
        .collect();
    if selectable.is_empty() {
        return;
    }
    let pos = selectable
        .iter()
        .position(|&idx| idx == *selected)
        .unwrap_or(0) as i32;
    let next = (pos + delta).clamp(0, selectable.len() as i32 - 1);
    *selected = selectable[next as usize];
}
