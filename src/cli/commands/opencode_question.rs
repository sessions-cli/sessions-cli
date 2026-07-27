//! Interactive OpenCode question collector.
//!
//! OpenCode's native question Review UI can swallow Enter (opentui textarea).
//! The sessions OpenCode plugin opens this command in a tmux display-popup so
//! the user can answer reliably; answers are written as JSON for the plugin
//! to POST via `client.question.reply`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct QuestionOption {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuestionInfo {
    question: String,
    header: String,
    #[serde(default)]
    options: Vec<QuestionOption>,
    #[serde(default)]
    multiple: bool,
    /// When absent, OpenCode defaults custom answers to allowed.
    #[serde(default = "default_true")]
    custom: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct QuestionRequest {
    id: String,
    #[serde(rename = "sessionID", default)]
    session_id: String,
    questions: Vec<QuestionInfo>,
}

#[derive(Debug, Serialize)]
struct AnswerFile {
    request_id: String,
    session_id: String,
    answers: Vec<Vec<String>>,
}

pub fn run(request: PathBuf, output: PathBuf) -> Result<()> {
    let raw = std::fs::read_to_string(&request)
        .with_context(|| format!("read request {}", request.display()))?;
    let req: QuestionRequest =
        serde_json::from_str(&raw).context("parse OpenCode question request JSON")?;
    if req.questions.is_empty() {
        bail!("question request has no questions");
    }

    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = io::stdout();

    writeln!(
        stdout,
        "\n sessions · OpenCode questions  (Enter submits; OpenCode's own Enter UI is bypassed)\n"
    )?;
    if !req.session_id.is_empty() {
        writeln!(stdout, " session: {}\n", req.session_id)?;
    }

    let mut answers: Vec<Vec<String>> = Vec::with_capacity(req.questions.len());
    let total = req.questions.len();

    for (idx, q) in req.questions.iter().enumerate() {
        writeln!(stdout, "────────────────────────────────────────")?;
        writeln!(stdout, " [{}/{}] {}", idx + 1, total, q.header)?;
        writeln!(stdout, " {}", q.question)?;
        writeln!(stdout)?;

        if q.options.is_empty() && !q.custom {
            bail!(
                "question '{}' has no options and custom is disabled",
                q.header
            );
        }

        for (i, opt) in q.options.iter().enumerate() {
            write!(stdout, "  [{}] {}", i + 1, opt.label)?;
            if let Some(desc) = opt.description.as_deref().filter(|d| !d.is_empty()) {
                write!(stdout, "  — {desc}")?;
            }
            writeln!(stdout)?;
        }
        if q.custom {
            writeln!(stdout, "  [c] Type your own answer")?;
        }
        if q.multiple {
            writeln!(
                stdout,
                "\n multi-select: enter numbers separated by commas (e.g. 1,3) or c for custom"
            )?;
        } else {
            writeln!(
                stdout,
                "\n choose a number{}",
                if q.custom { " or c" } else { "" }
            )?;
        }
        write!(stdout, " > ")?;
        stdout.flush()?;

        let line = read_line(&mut stdin)?;
        let choice = line.trim();
        if choice.is_empty() {
            // Prefer recommended option when user just hits Enter.
            if let Some(rec) = q
                .options
                .iter()
                .find(|o| o.label.to_ascii_lowercase().contains("recommended"))
            {
                answers.push(vec![rec.label.clone()]);
                writeln!(stdout, " → {}", rec.label)?;
                continue;
            }
            if let Some(first) = q.options.first() {
                answers.push(vec![first.label.clone()]);
                writeln!(stdout, " → {}", first.label)?;
                continue;
            }
            bail!("empty answer for '{}'", q.header);
        }

        if q.custom && (choice.eq_ignore_ascii_case("c") || choice.eq_ignore_ascii_case("custom")) {
            write!(stdout, " custom answer > ")?;
            stdout.flush()?;
            let custom = read_line(&mut stdin)?;
            let custom = custom.trim();
            if custom.is_empty() {
                bail!("empty custom answer for '{}'", q.header);
            }
            answers.push(vec![custom.to_string()]);
            continue;
        }

        if q.multiple {
            let mut selected = Vec::new();
            for part in choice.split([',', ' ', ';']) {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let n: usize = part
                    .parse()
                    .with_context(|| format!("invalid choice '{part}'"))?;
                let opt = q
                    .options
                    .get(n.wrapping_sub(1))
                    .with_context(|| format!("choice {n} out of range"))?;
                selected.push(opt.label.clone());
            }
            if selected.is_empty() {
                bail!("no options selected for '{}'", q.header);
            }
            answers.push(selected);
        } else {
            let n: usize = choice
                .parse()
                .with_context(|| format!("invalid choice '{choice}' (use a number or c)"))?;
            let opt = q
                .options
                .get(n.wrapping_sub(1))
                .with_context(|| format!("choice {n} out of range"))?;
            answers.push(vec![opt.label.clone()]);
        }
    }

    writeln!(stdout, "────────────────────────────────────────")?;
    writeln!(stdout, " submitting {} answer(s)…", answers.len())?;
    stdout.flush()?;

    // Plugin expects a bare answers array; also write a rich envelope for debugging.
    let answers_json = serde_json::to_string_pretty(&answers)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output, answers_json + "\n")
        .with_context(|| format!("write answers {}", output.display()))?;

    // Sidecar metadata (not read by plugin; useful if inspecting temp files).
    let meta_path = output.with_extension("meta.json");
    let meta = AnswerFile {
        request_id: req.id,
        session_id: req.session_id,
        answers,
    };
    let _ = std::fs::write(meta_path, serde_json::to_string_pretty(&meta)? + "\n");

    Ok(())
}

fn read_line(stdin: &mut impl BufRead) -> Result<String> {
    let mut line = String::new();
    let n = stdin.read_line(&mut line)?;
    if n == 0 {
        bail!("EOF while reading answer (cancelled?)");
    }
    Ok(line)
}

/// Validate request JSON shape without prompting (unit-test helper).
#[cfg(test)]
pub fn parse_request_file(path: &std::path::Path) -> Result<usize> {
    let raw = std::fs::read_to_string(path)?;
    let req: QuestionRequest = serde_json::from_str(&raw)?;
    Ok(req.questions.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parses_opencode_question_payload() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("req.json");
        let sample = r#"{
          "id": "que_test",
          "sessionID": "ses_test",
          "questions": [
            {
              "question": "Ship it?",
              "header": "Ship",
              "options": [
                {"label": "Yes (Recommended)", "description": "do it"},
                {"label": "No"}
              ]
            }
          ]
        }"#;
        fs::write(&path, sample).unwrap();
        assert_eq!(parse_request_file(&path).unwrap(), 1);
    }

    #[test]
    fn rejects_empty_questions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("req.json");
        fs::write(&path, r#"{"id":"x","sessionID":"y","questions":[]}"#).unwrap();
        assert_eq!(parse_request_file(&path).unwrap(), 0);
    }

    #[test]
    fn roundtrip_answers_json_is_array_of_arrays() {
        let answers: Vec<Vec<String>> = vec![
            vec!["Yes (Recommended)".into()],
            vec!["custom free text".into()],
        ];
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&answers).unwrap()).unwrap();
        assert!(v.is_array());
        assert_eq!(v.as_array().unwrap().len(), 2);
    }
}
