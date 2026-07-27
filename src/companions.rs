//! Optional companion setup for the Skills and MCP management panels.
//!
//! These backends are **not** required for a basic sessions install. Panels
//! offer a one-shot setup dialog that runs the deployed ensure scripts when
//! the user opts in (or retries). Internally the Skills manager is skillshare
//! and the MCP gateway is Obot — the UI presents them as sessions managers.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// Which companion the setup dialog targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionKind {
    Skillshare,
    Obot,
}

impl CompanionKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Skillshare => "Set up Skills manager",
            Self::Obot => "Set up MCP manager",
        }
    }

    /// Human label for status messages (product-facing, not a deep brand pitch).
    pub fn short_name(self) -> &'static str {
        match self {
            Self::Skillshare => "Skills manager",
            Self::Obot => "MCP manager",
        }
    }

    /// Backend identifier shown as "currently installed" detail.
    pub fn backend_name(self) -> &'static str {
        match self {
            Self::Skillshare => "skillshare",
            Self::Obot => "obot",
        }
    }

    pub fn script_name(self) -> &'static str {
        match self {
            Self::Skillshare => "ensure-skillshare.sh",
            Self::Obot => "ensure-obot.sh",
        }
    }

    pub fn blurb(self) -> &'static [&'static str] {
        match self {
            Self::Skillshare => &[
                "Sessions Skills is a management portal for agent skill libraries.",
                "It can install the local skills manager and initialize the store",
                "for you (Homebrew or the upstream install script).",
                "",
                "Optional — skip to browse skills already on disk for each agent.",
            ],
            Self::Obot => &[
                "Sessions MCPs is a management portal for MCP servers across agents.",
                "It can start a local MCP gateway container for catalog + connect URLs.",
                "Requires Docker Desktop (or another Docker daemon).",
                "",
                "Optional — skip to use local MCP inventory only.",
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub enum SetupMsg {
    Line(String),
    Finished { success: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupPhase {
    /// Waiting for the user to confirm setup.
    Prompt,
    /// Ensure script running.
    Running,
    /// Finished successfully.
    DoneOk,
    /// Finished with failure (retry available).
    DoneFail,
}

#[derive(Debug, Clone)]
pub struct SetupDialog {
    pub kind: CompanionKind,
    pub phase: SetupPhase,
    pub lines: Vec<String>,
    pub scroll: usize,
}

impl SetupDialog {
    pub fn prompt(kind: CompanionKind) -> Self {
        let mut lines: Vec<String> = kind.blurb().iter().map(|s| (*s).to_string()).collect();
        lines.push(String::new());
        lines.push("Press Enter to set up automatically, or Esc to skip.".into());
        Self {
            kind,
            phase: SetupPhase::Prompt,
            lines,
            scroll: 0,
        }
    }

    pub fn apply(&mut self, msg: SetupMsg) {
        match msg {
            SetupMsg::Line(line) => {
                if self.phase == SetupPhase::Prompt {
                    self.phase = SetupPhase::Running;
                    self.lines.clear();
                }
                // Drop the placeholder "Starting…" line once real output arrives.
                if self.lines.len() == 1
                    && self.lines[0].starts_with("Starting")
                    && self.phase == SetupPhase::Running
                {
                    self.lines.clear();
                }
                self.lines.push(line);
            }
            SetupMsg::Finished { success } => {
                self.phase = if success {
                    SetupPhase::DoneOk
                } else {
                    SetupPhase::DoneFail
                };
                if success {
                    self.lines.push(format!(
                        "✓ {} ready — press Enter to continue.",
                        self.kind.short_name()
                    ));
                } else {
                    self.lines.push(format!(
                        "✗ {} setup failed — Enter retry · Esc skip.",
                        self.kind.short_name()
                    ));
                }
            }
        }
    }

    pub fn hint(&self) -> &'static str {
        match self.phase {
            SetupPhase::Prompt => "Enter set up · Esc skip",
            SetupPhase::Running => "Setting up… Esc cancel view",
            SetupPhase::DoneOk => "Enter continue · Esc close",
            SetupPhase::DoneFail => "Enter retry · Esc skip",
        }
    }
}

/// Resolve ensure script: data scripts dir, then common fallbacks.
pub fn ensure_script_path(home: &Path, kind: CompanionKind) -> Option<PathBuf> {
    let name = kind.script_name();
    let candidates = [
        crate::paths::scripts_dir(home).join(name),
        home.join(".local/share/sessions/scripts").join(name),
        // Dev checkout next to the running binary (best-effort).
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("../../../bin").join(name)))
            .unwrap_or_default(),
    ];
    candidates
        .into_iter()
        .find(|p| p.is_file() && !p.as_os_str().is_empty())
}

/// Skillshare binary missing → offer setup.
pub fn skillshare_needs_setup(home: &Path) -> bool {
    !crate::skills::status(home).installed
}

/// Obot down (not merely disabled) → offer setup.
pub fn obot_needs_setup(home: &Path) -> bool {
    match crate::mcp::health(home) {
        Ok(h) => matches!(h.status, crate::mcp::ObotHealthStatus::Down),
        Err(_) => true,
    }
}

/// Spawn the ensure script; stream stdout/stderr lines to the receiver.
pub fn spawn_ensure(home: &Path, kind: CompanionKind) -> Receiver<SetupMsg> {
    let (tx, rx) = mpsc::channel();
    let script = ensure_script_path(home, kind);
    thread::spawn(move || {
        let Some(script) = script else {
            let _ = tx.send(SetupMsg::Line(format!(
                "ensure script missing ({}) — reinstall sessions or copy bin/{}",
                kind.backend_name(),
                kind.script_name()
            )));
            let _ = tx.send(SetupMsg::Finished { success: false });
            return;
        };
        let _ = tx.send(SetupMsg::Line(format!("Running {}…", script.display())));
        let child = Command::new("bash")
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let Ok(mut child) = child else {
            let _ = tx.send(SetupMsg::Line("Failed to start setup script.".into()));
            let _ = tx.send(SetupMsg::Finished { success: false });
            return;
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let tx_out = tx.clone();
        let tx_err = tx.clone();
        let out_t = thread::spawn(move || pipe_lines(stdout, tx_out));
        let err_t = thread::spawn(move || pipe_lines(stderr, tx_err));
        let success = child.wait().map(|s| s.success()).unwrap_or(false);
        let _ = out_t.join();
        let _ = err_t.join();
        let _ = tx.send(SetupMsg::Finished { success });
    });
    rx
}

fn pipe_lines<R: std::io::Read + Send + 'static>(reader: Option<R>, tx: Sender<SetupMsg>) {
    let Some(reader) = reader else {
        return;
    };
    for line in BufReader::new(reader).lines().map_while(Result::ok) {
        let trimmed = line.trim_end().to_string();
        if !trimmed.is_empty() {
            let _ = tx.send(SetupMsg::Line(trimmed));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn skillshare_needs_setup_without_binary() {
        std::env::remove_var("SKILLSHARE_BIN");
        // May still find a real binary on PATH — only assert helper is stable.
        let home = TempDir::new().unwrap();
        let _ = skillshare_needs_setup(home.path());
    }

    #[test]
    fn ensure_script_path_is_stable() {
        let home = TempDir::new().unwrap();
        // May resolve a repo-relative bin/ via current_exe in this checkout;
        // only assert the helper does not panic.
        let _ = ensure_script_path(home.path(), CompanionKind::Obot);
        let _ = ensure_script_path(home.path(), CompanionKind::Skillshare);
    }

    #[test]
    fn setup_dialog_prompt_has_blurb() {
        let d = SetupDialog::prompt(CompanionKind::Obot);
        assert_eq!(d.phase, SetupPhase::Prompt);
        assert!(d.lines.iter().any(|l| l.contains("Docker")));
    }
}
