pub struct ModelOption {
    pub id: &'static str,
    pub label: &'static str,
}

pub trait LaunchProvider: Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn default_model(&self) -> &'static str;
    fn models(&self) -> &'static [ModelOption];
    fn accepts_cli_prompt(&self) -> bool;
    fn build_quick_launch_command(&self) -> String;
    fn build_command(&self, model_id: &str, prompt: Option<&str>) -> String;
    fn build_resume_command(&self, model_hint: Option<&str>, agent_session_id: &str) -> String;
}

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}
