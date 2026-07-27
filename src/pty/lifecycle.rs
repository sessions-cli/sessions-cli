use crate::model::AgentState;

use super::classify::is_shell_binary;

pub fn infer_pane_state(binary: &str, pane_dead: bool, exit_status: Option<i32>) -> AgentState {
    let binary_lower = binary.trim().to_ascii_lowercase();
    if is_shell_binary(&binary_lower) {
        return AgentState::Idle;
    }
    if pane_dead {
        return match exit_status {
            Some(0) => AgentState::Done,
            Some(_) => AgentState::Error,
            None => AgentState::Idle,
        };
    }
    AgentState::Working
}

pub fn merge_lifecycle_state(stored: AgentState, polled: AgentState) -> AgentState {
    match (stored, polled) {
        (AgentState::Working, AgentState::Done) => AgentState::Working,
        (AgentState::Done, _) => AgentState::Done,
        (AgentState::Idle, AgentState::Done) => AgentState::Done,
        (AgentState::Idle, AgentState::Working) => AgentState::Working,
        (_, polled) => polled,
    }
}
