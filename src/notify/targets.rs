use crate::config::Config;
use crate::daemon::tmux::{pane_to_window_index, window_to_pane_id};
use crate::session::{load_session_env, SessionEnv};

/// Grok's notification runner and stale `.env` files may omit or misreport
/// `TMUX_PANE`. Reconcile against the bound window index before sending.
pub(crate) fn enrich_notify_targets(
    config: &Config,
    session_env: &SessionEnv,
    tmux_pane_id: &mut Option<String>,
    tmux_session: &mut Option<String>,
    kitty_window_id: &mut Option<u64>,
) {
    let session = tmux_session
        .clone()
        .or_else(|| session_env.tmux_session.clone())
        .unwrap_or_else(|| config.tmux_session.clone());
    *tmux_session = Some(session.clone());

    let mut pane_from_managed = false;
    if let Ok(ssn_id) = std::env::var("SESSIONS_SESSION_ID") {
        if !ssn_id.is_empty() {
            if let Some(record) = crate::session::load_managed_record(&config.home, &ssn_id) {
                if kitty_window_id.is_none() {
                    *kitty_window_id = Some(record.window_index as u64);
                }
                if tmux_pane_id.is_none() {
                    if record.pane_id.is_some() {
                        pane_from_managed = true;
                    }
                    *tmux_pane_id = record.pane_id.clone();
                }
                if tmux_session.as_deref() != Some(record.tmux_session.as_str()) {
                    *tmux_session = Some(record.tmux_session.clone());
                }
            }
        }
    }

    if let Some(ref pane) = tmux_pane_id {
        if let Some(idx) = pane_to_window_index(&session, pane) {
            *kitty_window_id = Some(idx as u64);
            return;
        }
        if pane_from_managed {
            return;
        }
        *tmux_pane_id = None;
    }

    let window_index = session_env
        .window_index
        .or_else(|| {
            std::env::var("SESSIONS_WINDOW_INDEX")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .or_else(|| (*kitty_window_id).map(|idx| idx as u32));
    let Some(window_index) = window_index else {
        return;
    };

    if let Some(pane) = window_to_pane_id(&session, window_index) {
        *tmux_pane_id = Some(pane);
    }
    *kitty_window_id = Some(window_index as u64);
}

/// Completion hooks (`turn_complete` / `stop`) are fired by Grok's notification
/// runner, not inside the agent tmux pane. Prefer the bound session's `.env`
/// mapping over ambient `TMUX_PANE` / `KITTY_WINDOW_ID` from the parent shell.
pub(crate) fn resolve_notify_targets(
    event: &str,
    session_env: &SessionEnv,
    shell_pane: Option<String>,
    shell_tmux_session: Option<String>,
    shell_kitty_window_id: Option<u64>,
) -> (Option<String>, Option<String>, Option<u64>) {
    if matches!(event, "stop" | "turn_complete") {
        let pane = session_env.tmux_pane_id.clone().or(shell_pane);
        let session = session_env.tmux_session.clone().or(shell_tmux_session);
        let kitty = session_env.window_index.map(u64::from);
        return (pane, session, kitty);
    }

    let pane = shell_pane.or_else(|| session_env.tmux_pane_id.clone());
    let session = shell_tmux_session.or_else(|| session_env.tmux_session.clone());
    let kitty = shell_kitty_window_id.or_else(|| session_env.window_index.map(u64::from));
    (pane, session, kitty)
}

pub(crate) fn resolve_agent_session_from_pane(
    config: &Config,
    pane_id: Option<&str>,
    tmux_session: Option<&str>,
) -> Option<(String, SessionEnv)> {
    let pane_id = pane_id?;
    let tmux_session = tmux_session.unwrap_or(&config.tmux_session);
    let mut best: Option<(String, SessionEnv, std::time::SystemTime)> = None;
    let dir = config.grok_state_dir();
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("env") {
            continue;
        }
        let env = load_session_env(&path);
        if env.tmux_pane_id.as_deref() != Some(pane_id) {
            continue;
        }
        if env
            .tmux_session
            .as_deref()
            .is_some_and(|session| session != tmux_session)
        {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())?;
        let sid = path.file_stem()?.to_string_lossy().to_string();
        if best
            .as_ref()
            .map(|(_, _, prev)| modified > *prev)
            .unwrap_or(true)
        {
            best = Some((sid, env, modified));
        }
    }
    best.map(|(sid, env, _)| (sid, env))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tempfile::TempDir;

    #[test]
    fn resolve_notify_targets_prefers_session_env_for_turn_complete() {
        let session_env = SessionEnv {
            tmux_pane_id: Some("%49".into()),
            window_index: Some(6),
            tmux_session: Some("agents".into()),
            ..Default::default()
        };
        let (pane, session, kitty) = resolve_notify_targets(
            "turn_complete",
            &session_env,
            Some("%61".into()),
            Some("agents".into()),
            Some(1),
        );
        assert_eq!(pane.as_deref(), Some("%49"));
        assert_eq!(session.as_deref(), Some("agents"));
        assert_eq!(kitty, Some(6));
    }

    #[test]
    fn resolve_notify_targets_prefers_shell_pane_for_prompt() {
        let session_env = SessionEnv {
            tmux_pane_id: Some("%49".into()),
            window_index: Some(6),
            tmux_session: Some("agents".into()),
            ..Default::default()
        };
        let (pane, session, kitty) = resolve_notify_targets(
            "prompt",
            &session_env,
            Some("%61".into()),
            Some("agents".into()),
            Some(1),
        );
        assert_eq!(pane.as_deref(), Some("%61"));
        assert_eq!(session.as_deref(), Some("agents"));
        assert_eq!(kitty, Some(1));
    }

    #[test]
    fn enrich_notify_targets_uses_managed_launch_record() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let record = crate::session::ManagedLaunchRecord {
            sessions_session_id: "ssn_enrich".into(),
            launch_id: "lch_enrich".into(),
            agent: "grok".into(),
            tmux_session: "agents".into(),
            window_index: 15,
            pane_id: Some("%915".into()),
            initial_cwd: env!("CARGO_MANIFEST_DIR").into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            agent_session_id: None,
        };
        crate::session::save_managed_record(&config.home, &record).unwrap();

        let session_env = SessionEnv::default();
        let mut tmux_pane_id = None;
        let mut tmux_session = None;
        let mut kitty_window_id = None;
        std::env::set_var("SESSIONS_SESSION_ID", "ssn_enrich");
        enrich_notify_targets(
            &config,
            &session_env,
            &mut tmux_pane_id,
            &mut tmux_session,
            &mut kitty_window_id,
        );
        std::env::remove_var("SESSIONS_SESSION_ID");

        assert_eq!(tmux_session.as_deref(), Some("agents"));
        assert_eq!(kitty_window_id, Some(15));
        assert!(
            tmux_pane_id.is_some(),
            "pane should resolve from managed window index"
        );
    }

    #[test]
    fn resolve_agent_session_from_pane_finds_latest_env_file() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let state_dir = config.grok_state_dir();
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("older-session.env"),
            "TMUX_PANE=%9\nTMUX_SESSION=agents\nSESSIONS_WINDOW_INDEX=6\n",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            state_dir.join("newer-session.env"),
            "TMUX_PANE=%9\nTMUX_SESSION=agents\nSESSIONS_WINDOW_INDEX=6\n",
        )
        .unwrap();

        let resolved = resolve_agent_session_from_pane(&config, Some("%9"), Some("agents"));
        assert_eq!(resolved.map(|(sid, _)| sid), Some("newer-session".into()));
    }
}