use crate::agents::traits::launch::{shell_quote, LaunchProvider, ModelOption};

pub struct GrokLaunch;

impl LaunchProvider for GrokLaunch {
    fn id(&self) -> &'static str {
        "grok"
    }

    fn label(&self) -> &'static str {
        "Grok"
    }

    fn default_model(&self) -> &'static str {
        "grok-4.5"
    }

    fn models(&self) -> &'static [ModelOption] {
        // Keep in sync with `grok models` / ~/.grok/models_cache.json.
        &[
            ModelOption {
                id: "grok-4.5",
                label: "Grok 4.5",
            },
            ModelOption {
                id: "grok-composer-2.5-fast",
                label: "Composer 2.5",
            },
        ]
    }

    fn accepts_cli_prompt(&self) -> bool {
        true
    }

    fn build_quick_launch_command(&self) -> String {
        "grok".into()
    }

    fn build_command(&self, model_id: &str, prompt: Option<&str>) -> String {
        let model = shell_quote(model_id);
        match prompt.filter(|text| !text.trim().is_empty()) {
            Some(prompt) => format!("grok --model {model} {}", shell_quote(prompt)),
            None => format!("grok --model {model}"),
        }
    }

    fn build_resume_command(&self, model_hint: Option<&str>, agent_session_id: &str) -> String {
        let session_id = shell_quote(agent_session_id);
        match model_hint.filter(|model| !model.trim().is_empty()) {
            Some(model) => format!("grok --model {} --resume {session_id}", shell_quote(model)),
            None => format!("grok --resume {session_id}"),
        }
    }
}

pub const GROK_LAUNCH: GrokLaunch = GrokLaunch;
