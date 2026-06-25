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
        other => other,
    }
}

pub fn event_to_state(event: &str) -> Option<AgentState> {
    match normalize_hook_event(event) {
        "session_start" => Some(AgentState::Idle),
        "prompt" => Some(AgentState::Working),
        "pre_tool" => Some(AgentState::Approval),
        "post_tool" => Some(AgentState::Working),
        "tool_fail" => Some(AgentState::Error),
        "stop" | "turn_complete" => Some(AgentState::Done),
        _ => None,
    }
}

/// Grok events that mark a thread complete (green highlight + bell).
pub fn marks_thread_complete(event: &str) -> bool {
    matches!(normalize_hook_event(event), "stop" | "turn_complete")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentState;

    #[test]
    fn event_to_state_mapping() {
        assert_eq!(event_to_state("pre_tool"), Some(AgentState::Approval));
        assert_eq!(event_to_state("pre_tool_use"), Some(AgentState::Approval));
        assert_eq!(event_to_state("stop"), Some(AgentState::Done));
        assert_eq!(event_to_state("turn_complete"), Some(AgentState::Done));
        assert!(marks_thread_complete("turn_complete"));
        assert!(!marks_thread_complete("prompt"));
    }
}