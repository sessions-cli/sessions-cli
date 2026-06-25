use crate::agents::traits::launch::{shell_quote, LaunchProvider, ModelOption};

pub struct ClaudeLaunch;

impl LaunchProvider for ClaudeLaunch {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude"
    }

    fn default_model(&self) -> &'static str {
        "sonnet"
    }

    fn models(&self) -> &'static [ModelOption] {
        &[
            ModelOption {
                id: "sonnet",
                label: "Sonnet",
            },
            ModelOption {
                id: "opus",
                label: "Opus",
            },
            ModelOption {
                id: "haiku",
                label: "Haiku",
            },
        ]
    }

    fn accepts_cli_prompt(&self) -> bool {
        true
    }

    fn build_quick_launch_command(&self) -> String {
        self.build_command(self.default_model(), None)
    }

    fn build_command(&self, model_id: &str, prompt: Option<&str>) -> String {
        let model = shell_quote(model_id);
        match prompt.filter(|text| !text.trim().is_empty()) {
            Some(prompt) => format!("claude --model {model} {}", shell_quote(prompt)),
            None => format!("claude --model {model}"),
        }
    }

    fn build_resume_command(&self, model_hint: Option<&str>, agent_session_id: &str) -> String {
        let session_id = shell_quote(agent_session_id);
        match model_hint.filter(|model| !model.trim().is_empty()) {
            Some(model) => format!(
                "claude --model {} --resume {session_id}",
                shell_quote(model)
            ),
            None => format!("claude --resume {session_id}"),
        }
    }
}

pub const CLAUDE_LAUNCH: ClaudeLaunch = ClaudeLaunch;
