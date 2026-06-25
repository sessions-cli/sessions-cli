use crate::agents::traits::launch::{shell_quote, LaunchProvider, ModelOption};

pub struct CodexLaunch;

impl LaunchProvider for CodexLaunch {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn default_model(&self) -> &'static str {
        "gpt-5.4"
    }

    fn models(&self) -> &'static [ModelOption] {
        &[
            ModelOption {
                id: "gpt-5.4",
                label: "GPT-5.4",
            },
            ModelOption {
                id: "gpt-5.5",
                label: "GPT-5.5",
            },
            ModelOption {
                id: "gpt-5.3-codex",
                label: "GPT-5.3 Codex",
            },
            ModelOption {
                id: "o3",
                label: "o3",
            },
        ]
    }

    fn accepts_cli_prompt(&self) -> bool {
        true
    }

    fn build_quick_launch_command(&self) -> String {
        "codex".into()
    }

    fn build_command(&self, model_id: &str, prompt: Option<&str>) -> String {
        let model = shell_quote(model_id);
        match prompt.filter(|text| !text.trim().is_empty()) {
            Some(prompt) => format!("codex --model {model} {}", shell_quote(prompt)),
            None => format!("codex --model {model}"),
        }
    }

    fn build_resume_command(&self, model_hint: Option<&str>, agent_session_id: &str) -> String {
        let session_id = shell_quote(agent_session_id);
        match model_hint.filter(|model| !model.trim().is_empty()) {
            Some(model) => format!("codex resume --model {} {session_id}", shell_quote(model)),
            None => format!("codex resume {session_id}"),
        }
    }
}

pub const CODEX_LAUNCH: CodexLaunch = CodexLaunch;
