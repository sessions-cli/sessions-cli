use std::path::{Path, PathBuf};

use crate::paths;

pub fn hook_binary(home: &Path) -> PathBuf {
    let candidate = if let Ok(path) = std::env::var("SESSIONS_BIN") {
        let path = path.trim();
        if !path.is_empty() {
            let candidate = PathBuf::from(path);
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    }
    .or_else(|| {
        let local = home.join(".local/bin/sessions");
        local.is_file().then_some(local)
    })
    .or_else(|| {
        let installed = paths::binary_path(home);
        installed.is_file().then_some(installed)
    })
    .unwrap_or_else(|| paths::resolve_binary(home));

    candidate.canonicalize().unwrap_or(candidate)
}

pub fn command_uses_binary(command: &str, expected: &str) -> bool {
    rewrite_notify_command(command, expected).is_none()
}

pub fn rewrite_notify_command(command: &str, expected: &str) -> Option<String> {
    if !is_sessions_notify_command(command) {
        return None;
    }
    let notify_idx = command.find(" notify")?;
    let prefix = &command[..notify_idx];
    let suffix = &command[notify_idx..];
    if prefix == expected {
        return None;
    }
    let base = prefix.rsplit('/').next().unwrap_or(prefix);
    if !base.contains("sessions") {
        return None;
    }
    Some(format!("{expected}{suffix}"))
}

pub(crate) fn is_sessions_notify_command(command: &str) -> bool {
    command.contains("sessions notify") || command.contains(" notify --event")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_notify_command_replaces_legacy_binary() {
        let updated = rewrite_notify_command(
            "/home/user/.grok/scripts/sessions notify --event prompt",
            "/home/user/.local/bin/sessions",
        )
        .expect("rewrite");
        assert_eq!(
            updated,
            "/home/user/.local/bin/sessions notify --event prompt"
        );
    }

    #[test]
    fn rewrite_notify_command_skips_current_binary() {
        assert!(rewrite_notify_command(
            "/home/user/.local/bin/sessions notify --event prompt",
            "/home/user/.local/bin/sessions",
        )
        .is_none());
    }
}