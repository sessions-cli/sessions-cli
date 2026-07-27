use crate::agents::traits::launch::{shell_quote, LaunchProvider, ModelOption};

pub struct OpenCodeLaunch;

impl LaunchProvider for OpenCodeLaunch {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn default_model(&self) -> &'static str {
        "default"
    }

    fn models(&self) -> &'static [ModelOption] {
        &[ModelOption {
            id: "default",
            label: "Default",
        }]
    }

    fn accepts_cli_prompt(&self) -> bool {
        true
    }

    fn build_quick_launch_command(&self) -> String {
        "opencode".into()
    }

    fn build_command(&self, _model_id: &str, prompt: Option<&str>) -> String {
        match prompt.filter(|text| !text.trim().is_empty()) {
            Some(prompt) => format!("opencode --prompt {}", shell_quote(prompt)),
            None => "opencode".into(),
        }
    }

    fn build_resume_command(&self, model_hint: Option<&str>, agent_session_id: &str) -> String {
        let session_id = shell_quote(agent_session_id);
        match model_hint.filter(|model| !model.trim().is_empty()) {
            Some(model) => format!(
                "opencode --model {} --session {session_id}",
                shell_quote(model)
            ),
            None => format!("opencode --session {session_id}"),
        }
    }
}

pub const OPENCODE_LAUNCH: OpenCodeLaunch = OpenCodeLaunch;
