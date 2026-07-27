//! Findings classification for automation runs (inbox policy).

use super::schema::RunOutcome;

/// Marker agents may print so empty runs auto-archive:
/// `AUTOMATION_RESULT: findings` or `AUTOMATION_RESULT: empty`
pub const RESULT_MARKER: &str = "AUTOMATION_RESULT:";

/// Classify run output. Missing marker → Findings (safe default: don't hide work).
pub fn classify_output(text: &str) -> RunOutcome {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(RESULT_MARKER) {
            let value = rest.trim().to_ascii_lowercase();
            if value.starts_with("empty") || value == "none" || value == "noop" {
                return RunOutcome::Empty;
            }
            if value.starts_with("findings") || value.starts_with("report") {
                return RunOutcome::Findings;
            }
        }
    }
    RunOutcome::Findings
}

/// Hint appended to prompts in the editor so agents can signal empty runs.
pub fn prompt_result_hint() -> &'static str {
    "\n\nWhen finished, end with exactly one of:\nAUTOMATION_RESULT: findings\nAUTOMATION_RESULT: empty\n(Use empty only if there is nothing worth reporting.)"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_marker() {
        assert_eq!(
            classify_output("all good\nAUTOMATION_RESULT: empty\n"),
            RunOutcome::Empty
        );
    }

    #[test]
    fn findings_marker() {
        assert_eq!(
            classify_output("AUTOMATION_RESULT: findings"),
            RunOutcome::Findings
        );
    }

    #[test]
    fn default_findings() {
        assert_eq!(classify_output("just a report"), RunOutcome::Findings);
    }
}
