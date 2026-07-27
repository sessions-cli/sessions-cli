use crossterm::event::KeyModifiers;

/// macOS Command — crossterm may report SUPER, META, or ALT (Ghostty/tmux ESC prefixes).
pub fn has_command_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::META | KeyModifiers::ALT)
}

pub fn has_paste_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) || has_command_modifier(modifiers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_modifier_accepts_alt_for_ghostty_meta_prefix() {
        assert!(has_command_modifier(KeyModifiers::ALT));
        assert!(has_command_modifier(KeyModifiers::SUPER));
        assert!(!has_command_modifier(KeyModifiers::SHIFT));
    }
}
