#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutMetrics {
    pub frame_width: u16,
    pub frame_height: u16,
    pub list_height: usize,
    pub list_top_y: u16,
    pub list_inner_x: u16,
    pub list_line_width: usize,
    pub toolbar_top_y: u16,
    pub toolbar_row_count: u16,
    pub update_banner_top_y: u16,
    pub update_banner_row_count: u16,
    pub settings_top_y: u16,
    pub settings_row_count: u16,
    pub leave_top_y: u16,
    pub leave_row_count: u16,
    pub notepad_top_y: u16,
    pub notepad_header_y: u16,
    pub notepad_body_top_y: u16,
    pub notepad_body_rows: u16,
    pub notepad_expanded: bool,
    pub sessions_title_y: u16,
}

#[derive(Debug, Clone)]
pub struct LayoutPlan {
    pub metrics: LayoutMetrics,
    pub settings_section_rows: u16,
    pub(crate) frame_margin_top: u16,
    pub(crate) frame_margin_h: u16,
}