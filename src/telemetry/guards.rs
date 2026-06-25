use anyhow::{bail, Result};

const FORBIDDEN_PATTERNS: &[&str] = &[
    "/Users/",
    "/home/",
    "\\Users\\",
    "prompt",
    "ssn_",
    "agent_session_id",
    "checkout_path",
    "cwd",
    "hostname",
    "username",
    "email",
];

pub fn reject_sensitive_strings(payload: &str) -> Result<()> {
    let lower = payload.to_lowercase();
    for pattern in FORBIDDEN_PATTERNS {
        if lower.contains(&pattern.to_lowercase()) {
            bail!("telemetry payload contains forbidden pattern: {pattern}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_user_paths() {
        assert!(reject_sensitive_strings(r#"{"cwd":"/Users/ethan/proj"}"#).is_err());
    }

    #[test]
    fn allows_safe_payload() {
        assert!(reject_sensitive_strings(
            r#"{"install_id":"550e8400-e29b-41d4-a716-446655440000","version":"0.1.0"}"#
        )
        .is_ok());
    }
}