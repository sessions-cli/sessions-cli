//! Bar overlay panels (new session, settings, automations, mcps, skills).

pub mod automations;
pub mod mcps;
pub mod new_session;
pub mod settings;
pub mod setup_dialog;
pub mod skills;

pub use automations::run_automations;
pub use mcps::run_mcps;
pub use new_session::run_new_session;
pub use settings::run_settings;
pub use skills::run_skills;

use ratatui::layout::Rect;

/// Match Grok's default terminal viewport padding (`~/.grok/pager.toml`
/// `[scrollback.layout]` defaults: `outer_hpad_* = 2`, `outer_vpad = 1`).
///
/// Used by full-pane management panels (MCP, Skills, Automations) so content
/// breathes the same way as a Grok chat pane. Do **not** apply this to the
/// sessions list or New Session form — those keep their existing layout.
pub const PANEL_OUTER_HPAD_LEFT: u16 = 2;
pub const PANEL_OUTER_HPAD_RIGHT: u16 = 2;
pub const PANEL_OUTER_VPAD: u16 = 1;

/// Inset a full pane by Grok-matching outer margins.
pub fn panel_content_rect(pane: Rect) -> Rect {
    let h_inset = PANEL_OUTER_HPAD_LEFT.saturating_add(PANEL_OUTER_HPAD_RIGHT);
    let v_inset = PANEL_OUTER_VPAD.saturating_mul(2);
    Rect {
        x: pane.x.saturating_add(PANEL_OUTER_HPAD_LEFT),
        y: pane.y.saturating_add(PANEL_OUTER_VPAD),
        width: pane.width.saturating_sub(h_inset),
        height: pane.height.saturating_sub(v_inset),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_content_rect_matches_grok_defaults() {
        let pane = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 40,
        };
        let c = panel_content_rect(pane);
        assert_eq!(c.x, 2);
        assert_eq!(c.y, 1);
        assert_eq!(c.width, 76); // 80 - 2 - 2
        assert_eq!(c.height, 38); // 40 - 1 - 1
    }

    #[test]
    fn panel_content_rect_preserves_pane_origin() {
        let pane = Rect {
            x: 10,
            y: 5,
            width: 50,
            height: 20,
        };
        let c = panel_content_rect(pane);
        assert_eq!(c.x, 12);
        assert_eq!(c.y, 6);
        assert_eq!(c.width, 46);
        assert_eq!(c.height, 18);
    }
}
