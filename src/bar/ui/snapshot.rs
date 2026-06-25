//! Render-time view of sidebar state — bundles `draw()` inputs into one struct.

use super::{
    ContextMenu, DeleteNoteConfirmState, GroupDragState, RenameState, RowKind, ToolbarAction,
    UpdateBannerView,
};
use crate::bar::notepad::Note;
use crate::bar::ui::NoteDragState;
use chrono::{DateTime, Utc};
use std::collections::HashSet;

#[derive(Clone, Copy)]
pub struct SessionsView<'a> {
    pub rows: &'a [RowKind],
    pub selected: usize,
    pub scroll: usize,
    pub digit_buffer: &'a str,
    pub close_modifier_held: bool,
    pub hover_row: Option<usize>,
    pub close_target: Option<usize>,
    pub group_hover_row: Option<usize>,
    pub sessions_expanded: bool,
    pub folded_groups: &'a HashSet<String>,
    pub group_order: &'a [String],
    pub group_drag: &'a GroupDragState,
    pub sessions_title_hover: bool,
    pub sessions_title_add_hover: bool,
    pub anim_frame: usize,
}

#[derive(Clone, Copy)]
pub struct NotepadView<'a> {
    pub notes: &'a [Note],
    pub expanded: bool,
    pub notes_list_expanded: bool,
    pub active_note_index: Option<usize>,
    pub text: &'a str,
    pub cursor: usize,
    pub scroll: usize,
    pub focused: bool,
    pub section_header_hover: bool,
    pub section_add_hover: bool,
    pub note_hover: Option<usize>,
    pub note_drag: &'a NoteDragState,
    pub selection: Option<(usize, usize)>,
    pub last_saved_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy)]
pub struct ChromeView<'a> {
    pub toolbar_hover: Option<ToolbarAction>,
    pub coming_soon_frames: &'a [(ToolbarAction, usize)],
    pub settings_hover: bool,
    pub leave_hover: bool,
    pub workspace_settings_open: bool,
    pub workspace_new_session_open: bool,
}

#[derive(Clone, Copy)]
pub struct OverlayView<'a> {
    pub context_menu: Option<&'a ContextMenu>,
    pub rename: Option<&'a RenameState>,
    pub delete_note_confirm: Option<&'a DeleteNoteConfirmState>,
    pub clipboard_notice: Option<&'a str>,
    pub update_banner: Option<&'a UpdateBannerView>,
    pub update_upgrade_hover: bool,
    pub update_dismiss_hover: bool,
}

#[derive(Clone, Copy)]
pub struct SidebarSnapshot<'a> {
    pub sessions: SessionsView<'a>,
    pub notepad: NotepadView<'a>,
    pub chrome: ChromeView<'a>,
    pub overlay: OverlayView<'a>,
}