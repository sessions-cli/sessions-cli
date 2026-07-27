//! Terminal color environment for sessions TUIs and workspace panes.
//!
//! Agent shells / CI often export `NO_COLOR=1` and friends. When `sessions up`
//! starts the tmux server from that environment, every pane inherits monochrome
//! forever — sidebar (ratatui/crossterm), grok/codex TUIs, and plain shells.
//!
//! Call [`force_process_color_env`] in-process before drawing a TUI, and prefix
//! shell launches with [`shell_env_prefix`] / [`shell_exports`] so children start clean.
//!
//! ## `FORCE_COLOR` level (not a boolean)
//!
//! Chalk / `supports-color` (and Grok's detector) treat `FORCE_COLOR` as a
//! **color level**, not "force on":
//!
//! | Value | Level |
//! |-------|--------|
//! | `0` / `false` | no color |
//! | `1` / `true` | **basic** (16 ANSI colors) |
//! | `2` | 256-color |
//! | `3` | **truecolor** (24-bit) |
//!
//! `FORCE_COLOR=1` **overrides** `COLORTERM=truecolor` and locks tools into
//! basic — Grok then reports `color basic` and hides Oscura Midnight / TokyoNight.
//! Always use `FORCE_COLOR=3` when forcing color for full RGB themes.

/// Chalk / supports-color truecolor level. Must stay `3` (not `1` = basic).
pub const FORCE_COLOR_TRUECOLOR: &str = "3";

/// Clear monochrome kill-switches and force truecolor-friendly flags in **this** process.
pub fn force_process_color_env() {
    for key in ["NO_COLOR", "PIP_NO_COLOR"] {
        std::env::remove_var(key);
    }
    // Positive force flags used by various CLIs; crossterm keys off NO_COLOR + COLORTERM.
    std::env::set_var("CLICOLOR", "1");
    std::env::set_var("CLICOLOR_FORCE", "1");
    // Level 3 = truecolor. Level 1 = basic only (overrides COLORTERM) — see module docs.
    std::env::set_var("FORCE_COLOR", FORCE_COLOR_TRUECOLOR);
    match std::env::var("COLORTERM") {
        Ok(v) if !v.is_empty() => {}
        _ => std::env::set_var("COLORTERM", "truecolor"),
    }
    if std::env::var_os("CARGO_TERM_COLOR").as_deref() == Some(std::ffi::OsStr::new("never")) {
        std::env::set_var("CARGO_TERM_COLOR", "always");
    }
    if std::env::var_os("NPM_CONFIG_COLOR").as_deref() == Some(std::ffi::OsStr::new("false")) {
        std::env::remove_var("NPM_CONFIG_COLOR");
    }
}

/// `env …` prefix so a child binary/shell starts without monochrome flags.
///
/// Use as: `{prefix} /path/to/cmd args…` — do **not** put shell `exec` between
/// `env` and the program (`env` would treat `exec` as argv0).
pub fn shell_env_prefix() -> &'static str {
    // FORCE_COLOR=3 (truecolor). Never use FORCE_COLOR=1 — that is "basic" in chalk/Grok.
    "env -u NO_COLOR -u PIP_NO_COLOR CLICOLOR=1 CLICOLOR_FORCE=1 FORCE_COLOR=3 \
COLORTERM=truecolor CARGO_TERM_COLOR=always"
}

/// Shell statements to inject at the start of `zsh -lc` scripts (exports + unsets).
pub fn shell_exports() -> &'static str {
    "unset NO_COLOR PIP_NO_COLOR NPM_CONFIG_COLOR; \
export CLICOLOR=1 CLICOLOR_FORCE=1 FORCE_COLOR=3 COLORTERM=truecolor CARGO_TERM_COLOR=always"
}

/// Global tmux env keys to unset (monochrome / never-color).
pub const TMUX_UNSET_KEYS: &[&str] = &[
    "NO_COLOR",
    "PIP_NO_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "FORCE_COLOR",
    "CARGO_TERM_COLOR",
    "NPM_CONFIG_COLOR",
];

/// Global tmux env keys to set for colorful panes.
pub const TMUX_SET_PAIRS: &[(&str, &str)] = &[
    ("COLORTERM", "truecolor"),
    ("CLICOLOR", "1"),
    ("CLICOLOR_FORCE", "1"),
    ("FORCE_COLOR", FORCE_COLOR_TRUECOLOR),
    ("CARGO_TERM_COLOR", "always"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_process_color_env_clears_no_color() {
        std::env::set_var("NO_COLOR", "1");
        std::env::set_var("CLICOLOR", "0");
        std::env::set_var("CLICOLOR_FORCE", "0");
        std::env::set_var("FORCE_COLOR", "0");
        std::env::set_var("CARGO_TERM_COLOR", "never");
        std::env::set_var("NPM_CONFIG_COLOR", "false");
        std::env::remove_var("COLORTERM");
        force_process_color_env();
        assert!(std::env::var_os("NO_COLOR").is_none());
        assert_eq!(std::env::var("CLICOLOR").unwrap(), "1");
        assert_eq!(std::env::var("CLICOLOR_FORCE").unwrap(), "1");
        assert_eq!(std::env::var("FORCE_COLOR").unwrap(), "3");
        assert_eq!(std::env::var("CARGO_TERM_COLOR").unwrap(), "always");
        assert!(std::env::var_os("NPM_CONFIG_COLOR").is_none());
        assert_eq!(std::env::var("COLORTERM").unwrap(), "truecolor");
    }

    #[test]
    fn force_color_is_truecolor_level_not_basic() {
        // Regression: FORCE_COLOR=1 → chalk/Grok "basic", hides truecolor themes.
        assert_eq!(FORCE_COLOR_TRUECOLOR, "3");
        assert!(shell_env_prefix().contains("FORCE_COLOR=3"));
        assert!(!shell_env_prefix().contains("FORCE_COLOR=1"));
        assert!(shell_exports().contains("FORCE_COLOR=3"));
        assert!(!shell_exports().contains("FORCE_COLOR=1"));
        assert!(TMUX_SET_PAIRS
            .iter()
            .any(|&(k, v)| k == "FORCE_COLOR" && v == "3"));
    }

    #[test]
    fn shell_env_prefix_unsets_no_color() {
        let prefix = shell_env_prefix();
        assert!(prefix.contains("-u NO_COLOR"));
        assert!(prefix.contains("COLORTERM=truecolor"));
        assert!(!prefix.contains(" exec "));
    }

    #[test]
    fn shell_exports_unsets_no_color() {
        let exports = shell_exports();
        assert!(exports.contains("unset NO_COLOR"));
        assert!(exports.contains("COLORTERM=truecolor"));
    }
}
