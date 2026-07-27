use crate::model::AgentState;

pub fn normalize_hook_event(event: &str) -> &str {
    match event {
        "user_prompt_submit" | "UserPromptSubmit" => "prompt",
        "pre_tool_use" | "PreToolUse" => "pre_tool",
        "post_tool_use" | "PostToolUse" => "post_tool",
        "post_tool_use_failure" | "PostToolUseFailure" => "tool_fail",
        "session_start" | "SessionStart" => "session_start",
        "stop" | "Stop" => "stop",
        "turn_complete" | "TurnComplete" => "turn_complete",
        // Grok notification when the agent needs the user (permission / plan / input).
        "approval_required" | "ApprovalRequired" => "approval_required",
        other => other,
    }
}

pub fn event_to_state(event: &str) -> Option<AgentState> {
    match normalize_hook_event(event) {
        "session_start" => Some(AgentState::Idle),
        "prompt" => Some(AgentState::Working),
        "pre_tool" => Some(AgentState::Approval),
        // True "needs assistance" from Grok notifications — not every PreToolUse.
        "approval_required" => Some(AgentState::Approval),
        "post_tool" => Some(AgentState::Working),
        "tool_fail" => Some(AgentState::Error),
        "stop" | "turn_complete" => Some(AgentState::Done),
        _ => None,
    }
}

/// Grok events that mark a thread complete (green highlight + completion bell).
pub fn marks_thread_complete(event: &str) -> bool {
    matches!(normalize_hook_event(event), "stop" | "turn_complete")
}

/// Events that should play the alert sound when successfully applied.
pub fn rings_alert(event: &str) -> bool {
    let event = normalize_hook_event(event);
    marks_thread_complete(event) || event == "approval_required"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentState;

    #[test]
    fn event_to_state_mapping() {
        assert_eq!(event_to_state("pre_tool"), Some(AgentState::Approval));
        assert_eq!(event_to_state("pre_tool_use"), Some(AgentState::Approval));
        assert_eq!(
            event_to_state("approval_required"),
            Some(AgentState::Approval)
        );
        assert_eq!(event_to_state("stop"), Some(AgentState::Done));
        assert_eq!(event_to_state("turn_complete"), Some(AgentState::Done));
        assert!(marks_thread_complete("turn_complete"));
        assert!(!marks_thread_complete("prompt"));
        assert!(!marks_thread_complete("approval_required"));
        assert!(rings_alert("turn_complete"));
        assert!(rings_alert("approval_required"));
        assert!(!rings_alert("pre_tool"));
    }
}
