use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionEnv {
    pub tmux_pane_id: Option<String>,
    pub window_index: Option<u32>,
    pub tmux_session: Option<String>,
    pub sessions_session_id: Option<String>,
    pub managed_agent: Option<String>,
}

pub fn load_session_env(path: &Path) -> SessionEnv {
    let Ok(data) = std::fs::read_to_string(path) else {
        return SessionEnv::default();
    };
    let mut env = SessionEnv::default();
    for line in data.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "TMUX_PANE" | "TMUX_PANE_ID" if env.tmux_pane_id.is_none() => {
                env.tmux_pane_id = Some(value.to_string());
            }
            "SESSIONS_WINDOW_INDEX" => {
                env.window_index = value.parse().ok();
            }
            "TMUX_SESSION" if env.tmux_session.is_none() => {
                env.tmux_session = Some(value.to_string());
            }
            "SESSIONS_SESSION_ID" if env.sessions_session_id.is_none() => {
                env.sessions_session_id = Some(value.to_string());
            }
            "SESSIONS_AGENT" if env.managed_agent.is_none() => {
                env.managed_agent = Some(value.to_string());
            }
            _ => {}
        }
    }
    env
}

#[derive(Debug, Clone)]
struct PaneSessionEntry {
    pane_id: String,
    window_index: Option<u32>,
    tmux_session: Option<String>,
    modified: SystemTime,
    sid: String,
}

#[derive(Debug, Default)]
pub struct PaneSessionIndex {
    entries: Vec<PaneSessionEntry>,
}

/// Scan agent state dir once and index session env files by tmux pane.
pub fn pane_session_index(home: &Path) -> PaneSessionIndex {
    let dir = crate::paths::state_dir(home);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return PaneSessionIndex::default(),
    };
    let mut index = PaneSessionIndex::default();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("env") {
            continue;
        }
        let Some(sid) = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
        else {
            continue;
        };
        let env = load_session_env(&path);
        let Some(pane_id) = env.tmux_pane_id.clone() else {
            continue;
        };
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        index.entries.push(PaneSessionEntry {
            pane_id,
            window_index: env.window_index,
            tmux_session: env.tmux_session,
            modified,
            sid,
        });
    }
    index
}

impl PaneSessionIndex {
    fn matches(
        entry: &PaneSessionEntry,
        pane_id: &str,
        window_index: u32,
        tmux_session: &str,
    ) -> bool {
        entry.pane_id == pane_id
            && entry.window_index.is_none_or(|index| index == window_index)
            && entry
                .tmux_session
                .as_deref()
                .is_none_or(|session| session == tmux_session)
    }
}

pub fn session_id_from_index(
    index: &PaneSessionIndex,
    pane_id: &str,
    window_index: u32,
    tmux_session: &str,
) -> Option<String> {
    let mut best: Option<(SystemTime, String)> = None;
    for entry in &index.entries {
        if !PaneSessionIndex::matches(entry, pane_id, window_index, tmux_session) {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(best_modified, _)| entry.modified > *best_modified)
        {
            best = Some((entry.modified, entry.sid.clone()));
        }
    }
    best.map(|(_, sid)| sid)
}

pub fn session_id_for_pane(
    home: &Path,
    pane_id: &str,
    window_index: u32,
    tmux_session: &str,
) -> Option<String> {
    let index = pane_session_index(home);
    session_id_from_index(&index, pane_id, window_index, tmux_session)
}

pub fn session_env_path(home: &Path, sid: &str) -> PathBuf {
    crate::paths::state_dir(home).join(format!("{sid}.env"))
}

/// True when the session env file was written at or after the pane process started.
/// Rejects stale pane→sid bindings left over from a prior agent process in the same tmux pane.
pub fn session_env_is_live_for_pane(home: &Path, sid: &str, pane_pid: u32) -> bool {
    if pane_pid == 0 {
        return true;
    }
    let env_path = session_env_path(home, sid);
    let Ok(meta) = std::fs::metadata(&env_path) else {
        return false;
    };
    let Ok(env_modified) = meta.modified() else {
        return true;
    };
    let Some(proc_start) = crate::process::process_start_time(pane_pid) else {
        return true;
    };
    // session_start may trail the first poll by a beat
    env_modified + Duration::from_secs(2) >= proc_start
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn session_env_is_live_for_current_process() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let sid = "sid-live";
        std::fs::create_dir_all(crate::paths::state_dir(home)).unwrap();
        let env_path = session_env_path(home, sid);
        std::fs::write(&env_path, "TMUX_PANE=%1\n").unwrap();
        let pid = std::process::id();
        assert!(session_env_is_live_for_pane(home, sid, pid));
    }

    #[test]
    fn load_session_env_parses_pane_and_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.env");
        std::fs::write(
            &path,
            "TMUX_PANE=%25\nSESSIONS_WINDOW_INDEX=13\nTMUX_SESSION=agents\n",
        )
        .unwrap();
        let env = load_session_env(&path);
        assert_eq!(env.tmux_pane_id.as_deref(), Some("%25"));
        assert_eq!(env.window_index, Some(13));
        assert_eq!(env.tmux_session.as_deref(), Some("agents"));
    }

    #[test]
    fn load_session_env_roundtrip_parses_sessions_session_id_and_agent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.env");
        let payload = concat!(
            "TMUX_PANE=%42\n",
            "TMUX_PANE_ID=%42\n",
            "SESSIONS_WINDOW_INDEX=7\n",
            "TMUX_SESSION=agents\n",
            "SESSIONS_SESSION_ID=ssn_roundtrip\n",
            "SESSIONS_AGENT=grok\n",
        );
        std::fs::write(&path, payload).unwrap();
        let env = load_session_env(&path);
        assert_eq!(env.tmux_pane_id.as_deref(), Some("%42"));
        assert_eq!(env.window_index, Some(7));
        assert_eq!(env.tmux_session.as_deref(), Some("agents"));
        assert_eq!(env.sessions_session_id.as_deref(), Some("ssn_roundtrip"));
        assert_eq!(env.managed_agent.as_deref(), Some("grok"));
    }

    #[test]
    fn session_env_is_stale_when_older_than_process() {
        use std::process::Command;

        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let sid = "sid-stale";
        std::fs::create_dir_all(crate::paths::state_dir(home)).unwrap();
        let env_path = session_env_path(home, sid);
        std::fs::write(&env_path, "TMUX_PANE=%1\n").unwrap();
        Command::new("/usr/bin/touch")
            .args(["-t", "197001010000", env_path.to_str().unwrap()])
            .status()
            .unwrap();
        let pid = std::process::id();
        if crate::process::process_start_time(pid).is_some() {
            assert!(!session_env_is_live_for_pane(home, sid, pid));
        }
    }
}

pub fn encode_session_cwd(cwd: &str) -> String {
    cwd.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}
