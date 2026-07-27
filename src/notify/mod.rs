pub mod events;
pub mod message;
pub mod payload;
mod targets;

use crate::config::Config;
use crate::model::NotifyMessage;
use crate::pty::detect_runtime_agent;
use crate::session::load_session_env;
use anyhow::Result;
use events::normalize_hook_event;
use message::build_notify_message;
use payload::read_hook_prompt_stdin;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use targets::{enrich_notify_targets, resolve_agent_session_from_pane, resolve_notify_targets};

pub fn run_notify(event: &str, payload: Option<&str>, use_stdin: bool) -> Result<()> {
    run_notify_inner(event, payload, use_stdin)
}

/// Fast path for `sessions notify` — avoids full Clap parsing when argv is well-formed.
pub fn try_fast_notify(args: &[String]) -> Result<()> {
    let mut event = None;
    let mut payload = None;
    let mut use_stdin = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--event" => {
                i += 1;
                event = args.get(i).cloned();
            }
            "--payload" => {
                i += 1;
                payload = args.get(i).map(|s| s.as_str());
            }
            "--stdin" => use_stdin = true,
            flag if flag.starts_with("--event=") => {
                event = Some(flag.trim_start_matches("--event=").to_string());
            }
            flag if flag.starts_with("--payload=") => {
                payload = Some(flag.trim_start_matches("--payload="));
            }
            other if event.is_none() && !other.starts_with('-') => {
                return Err(anyhow::anyhow!("unexpected arg: {other}"));
            }
            other => return Err(anyhow::anyhow!("unknown flag: {other}")),
        }
        i += 1;
    }
    let event = event.ok_or_else(|| anyhow::anyhow!("missing --event"))?;
    let normalized = normalize_hook_event(&event);
    let read_stdin = use_stdin || hook_reads_stdin(normalized, payload);
    run_notify_inner(normalized, payload, read_stdin)
}

pub fn hook_reads_stdin(event: &str, payload: Option<&str>) -> bool {
    payload.is_none() && matches!(normalize_hook_event(event), "prompt" | "session_start")
}

fn run_notify_inner(event: &str, payload: Option<&str>, use_stdin: bool) -> Result<()> {
    let config = Config::default();
    let event = normalize_hook_event(event);
    let shell_pane = std::env::var("TMUX_PANE")
        .or_else(|_| std::env::var("TMUX_PANE_ID"))
        .ok();
    let shell_tmux_session = std::env::var("TMUX_SESSION").ok();
    let shell_kitty_window_id = std::env::var("KITTY_WINDOW_ID")
        .ok()
        .and_then(|s| s.parse().ok());
    let kitty_pid = std::env::var("KITTY_PID").ok();
    let kitty_listen_on = std::env::var("KITTY_LISTEN_ON").ok();
    let mut cwd = std::env::var("PWD")
        .ok()
        .or_else(|| std::env::var("SESSIONS_INITIAL_CWD").ok());
    let sessions_session_id = std::env::var("SESSIONS_SESSION_ID")
        .ok()
        .filter(|value| !value.is_empty());
    let mut agent = std::env::var("SESSIONS_AGENT")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(detect_runtime_agent);

    let payload_value = if use_stdin && matches!(event, "prompt" | "session_start") {
        let prompt = read_hook_prompt_stdin();
        serde_json::json!({ "prompt": prompt })
    } else if let Some(p) = payload {
        serde_json::from_str(p).unwrap_or_else(|_| serde_json::json!({ "raw": p }))
    } else {
        serde_json::json!({})
    };

    // Explicit payload sessionId (OpenCode/Claude plugins, etc.) is authoritative.
    // Env-derived ids (GROK_SESSION_ID, CODEX_THREAD_ID, …) are fallbacks for hooks
    // that only set process env. Preferring env first let a stale GROK_SESSION_ID
    // hijack an OpenCode managed launch and group it under the wrong project.
    let payload_sid = payload_value
        .get("sessionId")
        .or_else(|| payload_value.get("session_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|sid| !sid.is_empty())
        .map(str::to_string);

    let mut agent_session_id =
        payload_sid.or_else(|| crate::agents::resolve_session_id_from_env().map(|(sid, _)| sid));
    let mut session_env = agent_session_id
        .as_deref()
        .map(|sid| load_session_env(&config.session_env_path(sid)))
        .unwrap_or_default();
    let mut tmux_pane_id;
    let mut tmux_session;
    let mut kitty_window_id;
    if agent_session_id.is_none() {
        tmux_pane_id = shell_pane.clone();
        tmux_session = shell_tmux_session.clone();
        kitty_window_id = shell_kitty_window_id;
        if let Some((sid, env)) = resolve_agent_session_from_pane(
            &config,
            tmux_pane_id.as_deref(),
            tmux_session.as_deref(),
        ) {
            agent_session_id = Some(sid);
            if tmux_pane_id.is_none() {
                tmux_pane_id = env.tmux_pane_id.clone();
            }
            if tmux_session.is_none() {
                tmux_session = env
                    .tmux_session
                    .clone()
                    .or_else(|| Some(config.tmux_session.clone()));
            }
            session_env = env.clone();
        }
    } else {
        (tmux_pane_id, tmux_session, kitty_window_id) = resolve_notify_targets(
            event,
            &session_env,
            shell_pane.clone(),
            shell_tmux_session.clone(),
            shell_kitty_window_id,
        );
    }

    // Agents that can't propagate tmux/kitty identity via environment (e.g.
    // OpenCode's background Bun plugin) embed them in the payload as fallback.
    if tmux_pane_id.is_none() {
        tmux_pane_id = payload_value
            .get("tmux_pane_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
    }
    if tmux_session.is_none() {
        tmux_session = payload_value
            .get("tmux_session")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
    }
    if kitty_window_id.is_none() {
        kitty_window_id = payload_value
            .get("kitty_window_id")
            .and_then(|v| v.as_u64())
            .filter(|&v| v > 0);
    }
    if agent.is_none() {
        agent = payload_value
            .get("agent")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
    }
    if cwd.is_none() {
        cwd = payload_value
            .get("cwd")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
    }

    enrich_notify_targets(
        &config,
        &session_env,
        &mut tmux_pane_id,
        &mut tmux_session,
        &mut kitty_window_id,
    );

    let sessions_session_id = sessions_session_id
        .or(session_env.sessions_session_id.clone())
        .or_else(|| {
            payload_value
                .get("sessions_session_id")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .or_else(|| {
            tmux_pane_id.as_ref().and_then(|pane| {
                let session = tmux_session.as_deref().unwrap_or(&config.tmux_session);
                crate::daemon::tmux::pane_sessions_session_id(session, pane)
            })
        });

    let msg = build_notify_message(
        event,
        agent.as_deref(),
        agent_session_id.as_deref(),
        kitty_window_id,
        tmux_pane_id.as_deref(),
        tmux_session.as_deref(),
        payload_value,
        cwd.as_deref(),
        kitty_pid.as_deref(),
        kitty_listen_on.as_deref(),
        sessions_session_id.as_deref(),
    );

    if try_send(&config.socket_path, &msg).is_err() {
        crate::daemon::metrics::record_notify_socket_failed();
        if spool_message(&config.spool_dir, &msg).is_err() {
            crate::daemon::metrics::record_notify_spool_failed();
        } else {
            crate::daemon::metrics::record_notify_deferred();
        }
    }
    Ok(())
}

fn try_send(socket_path: &Path, msg: &NotifyMessage) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path)?;
    let line = serde_json::to_string(msg)? + "\n";
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    Ok(())
}

static SPOOL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn spool_message(spool_dir: &Path, msg: &NotifyMessage) -> Result<()> {
    fs::create_dir_all(spool_dir)?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = SPOOL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let fallback = msg
        .tmux_pane_id
        .clone()
        .or_else(|| msg.kitty_window_id.map(|_| "unknown".to_string()));
    let session_id = msg
        .session_id
        .as_deref()
        .or(fallback.as_deref())
        .unwrap_or("unknown");
    let session_id = sanitize_filename_component(session_id);
    let path = spool_dir.join(format!("{ts}-{seq:04}-{session_id}.json"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    writeln!(file, "{}", serde_json::to_string(msg)?)?;
    Ok(())
}

fn sanitize_filename_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '_',
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".into()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NotifyMessage, NOTIFY_MESSAGE_TYPE};
    use tempfile::TempDir;

    #[test]
    fn spool_writes_file_when_socket_missing() {
        let dir = TempDir::new().unwrap();
        let msg = NotifyMessage {
            t: NOTIFY_MESSAGE_TYPE.into(),
            agent: Some("codex".into()),
            session_id: Some("test-session".into()),
            kitty_window_id: None,
            tmux_pane_id: Some("%1".into()),
            tmux_session: Some("agents".into()),
            event: "stop".into(),
            ts: 1,
            payload: serde_json::json!({}),
            cwd: None,
            kitty_pid: None,
            kitty_listen_on: None,

            sessions_session_id: None,
        };
        spool_message(dir.path(), &msg).unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
    }
}
