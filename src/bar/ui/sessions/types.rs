use crate::model::Session;

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum RowKind {
    Empty(String),
    Group {
        label: String,
        collapsed: bool,
    },
    Session {
        session: Session,
    },
    GroupToggle {
        cwd_label: String,
        expanded: bool,
        hidden_count: usize,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupDragState {
    pub source: Option<String>,
    pub hover: Option<String>,
    /// True once the pointer moves during a press — distinguishes click-to-fold from drag.
    pub dragged: bool,
    /// Session row to keep selected while preview reorder shifts row indices.
    pub preserved_session_id: Option<String>,
    /// Show-more row to keep selected when no session was selected at drag start.
    pub preserved_group_toggle: Option<String>,
    pub pending_click_label: Option<String>,
    pub pressed_at: Option<std::time::Instant>,
    pub pressed_row: Option<u16>,
}

impl GroupDragState {
    pub fn active(&self) -> bool {
        self.source.is_some()
    }

    pub fn pending(&self) -> bool {
        self.pending_click_label.is_some() && self.source.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSection {
    pub label: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListTextPoint {
    pub row_idx: usize,
    pub char_idx: usize,
}