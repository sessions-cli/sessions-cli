//! tmux `display-popup` geometry for workspace panels (card-sized, centered).

const FIELD_INNER_HEIGHT: u16 = 1;
const PROMPT_INNER_HEIGHT: u16 = 4;
const SECTION_GAP: u16 = 1;

/// Settings popup inset — matches sidebar section pad.
pub const PANEL_SECTION_PAD: u16 = 2;
/// New-chat card border (2) + horizontal padding (`SESSION_BLOCK_PAD_*`).
pub const MODAL_CARD_WIDTH_PAD: u16 = 4;
/// New-chat card border (2) + bottom padding (1).
pub const MODAL_CARD_HEIGHT_PAD: u16 = 3;

fn field_block_height(inner: u16) -> u16 {
    inner.saturating_add(2)
}

fn dropdown_field_height() -> u16 {
    field_block_height(FIELD_INNER_HEIGHT)
}

fn modal_content_height() -> u16 {
    let workspace_h = dropdown_field_height();
    let agent_h = dropdown_field_height();
    let model_h = dropdown_field_height();
    let prompt_h = field_block_height(PROMPT_INNER_HEIGHT);
    let button_h = field_block_height(FIELD_INNER_HEIGHT);
    1 + 1
        + 1
        + workspace_h
        + SECTION_GAP
        + 1
        + agent_h
        + SECTION_GAP
        + 1
        + model_h
        + SECTION_GAP
        + 1
        + prompt_h
        + SECTION_GAP
        + button_h
        + 2
        + 1
}

fn form_fraction_width(pane_width: u16) -> u16 {
    ((pane_width as u32 * 4) / 6).max(40) as u16
}

/// Visible card height — dropdown menus overlay inside the card; no extra slack
/// below the form (slack made the card look top-aligned in the pane).
pub fn new_session_card_height(pane_height: u16) -> u16 {
    modal_content_height()
        .saturating_add(MODAL_CARD_HEIGHT_PAD)
        .min(pane_height.max(1))
}

/// Card-sized popup for `sessions new-session` over the workspace pane.
pub fn new_session_popup_size(workspace_w: u16, workspace_h: u16) -> (u16, u16) {
    let width = form_fraction_width(workspace_w).saturating_add(MODAL_CARD_WIDTH_PAD);
    let height = new_session_card_height(workspace_h);
    (width.max(1), height.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_popup_fits_workspace_pane() {
        let (w, h) = new_session_popup_size(120, 50);
        assert!(w <= 120);
        assert!(h <= 50);
        assert!(w >= 40);
    }

    #[test]
    fn new_session_card_height_omits_dropdown_slack() {
        let (_, h) = new_session_popup_size(120, 80);
        assert_eq!(h, new_session_card_height(80));
        assert!(
            h < 80,
            "card should be shorter than the pane for vertical centering"
        );
    }
}
