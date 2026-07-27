use serde_json::Value;

pub fn read_hook_prompt(json_str: &str) -> String {
    let Ok(data) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return String::new();
    };
    for key in ["user_prompt", "prompt", "userPrompt"] {
        if let Some(val) = data.get(key).and_then(|v| v.as_str()) {
            let val = val.trim();
            if !val.is_empty() {
                return val.to_string();
            }
        }
    }
    String::new()
}

pub fn read_hook_prompt_stdin() -> String {
    use std::io::{IsTerminal, Read};

    if std::io::stdin().is_terminal() {
        return String::new();
    }
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        return String::new();
    }
    read_hook_prompt(&buf)
}

pub fn read_prompt_from_payload(payload: &Value) -> String {
    for key in ["user_prompt", "prompt", "userPrompt"] {
        if let Some(val) = payload.get(key).and_then(|v| v.as_str()) {
            let val = val.trim();
            if !val.is_empty() {
                return val.to_string();
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_hook_prompt_keys() {
        let json = r#"{"user_prompt": "fix the bug"}"#;
        assert_eq!(read_hook_prompt(json), "fix the bug");
    }
}
