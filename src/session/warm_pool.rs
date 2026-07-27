//! Pre-hydrated agent windows so PWD `+` / `[G]` / `[O]` feel instant.
//!
//! Cold `grok`/`opencode` TUI start is multi-second (black pane). The pool keeps
//! one detached spare per (agent, cwd) for recently used groups. Claim focuses
//! an already-running pane; refill happens in the background.

use crate::agents::build_quick_launch_command;
use crate::config::Config;
use crate::daemon::tmux::{
    self, claim_pool_window, create_managed_window_in_cwd_with_pool, is_pool_window_name,
    list_windows, CreatedWindow, POOL_WINDOW_NAME_PREFIX,
};
use crate::session::managed::{
    new_launch_id, new_sessions_session_id, save_managed_record, ManagedLaunchRecord,
};
use crate::session::manifest::{append_entry, ManifestSource};
use crate::session::LaunchSpec;
use anyhow::Result;
use chrono::Utc;
use std::collections::HashSet;
use std::path::Path;
use tracing::{debug, warn};

/// Agents that benefit from pre-hydration (slow TUI cold start).
const POOLABLE_AGENTS: &[&str] = &["grok", "opencode"];

/// Cap total detached spares so a large sidebar does not spawn dozens of agents.
const MAX_POOL_WINDOWS: usize = 8;

fn normalize_cwd(cwd: &str) -> String {
    cwd.trim_end_matches('/').to_string()
}

fn cwd_matches(a: &str, b: &str) -> bool {
    normalize_cwd(a) == normalize_cwd(b)
}

pub fn is_poolable_agent(agent_id: &str) -> bool {
    POOLABLE_AGENTS.contains(&agent_id)
}

fn pool_window_name(agent_id: &str) -> String {
    format!("{POOL_WINDOW_NAME_PREFIX}{agent_id}")
}

/// Claim a ready spare for `agent_id` in `cwd`, or cold-create like today.
pub fn claim_or_create_quick_agent(
    config: &Config,
    cwd: &str,
    agent_id: &str,
    source: ManifestSource,
    focus: bool,
) -> Result<CreatedWindow> {
    if is_poolable_agent(agent_id) {
        match try_claim(config, cwd, agent_id, source, focus) {
            Ok(Some(created)) => {
                schedule_refill(config, cwd, agent_id);
                return Ok(created);
            }
            Ok(None) => {}
            Err(err) => {
                warn!("warm pool claim failed for {agent_id} @ {cwd}: {err}");
            }
        }
    }
    let created = crate::session::create_quick_agent(config, cwd, agent_id, source, focus)?;
    if is_poolable_agent(agent_id) {
        schedule_refill(config, cwd, agent_id);
    }
    Ok(created)
}

fn schedule_refill(config: &Config, cwd: &str, agent_id: &str) {
    let cfg = config.clone();
    let cwd = cwd.to_string();
    let agent = agent_id.to_string();
    std::thread::spawn(move || {
        if let Err(err) = ensure_spare(&cfg, &cwd, &agent) {
            debug!("warm pool refill failed for {agent} @ {cwd}: {err}");
        }
    });
}

fn try_claim(
    config: &Config,
    cwd: &str,
    agent_id: &str,
    source: ManifestSource,
    focus: bool,
) -> Result<Option<CreatedWindow>> {
    if !tmux::session_exists(&config.tmux_session) {
        return Ok(None);
    }
    let windows = list_windows(&config.tmux_session)?;
    let Some(win) = windows.into_iter().find(|w| {
        w.pool
            && !w.pane_dead
            && cwd_matches(&w.cwd, cwd)
            && w.sessions_session_id.is_some()
            && agent_matches_window(w, agent_id)
    }) else {
        return Ok(None);
    };
    let ssn = win
        .sessions_session_id
        .clone()
        .unwrap_or_else(new_sessions_session_id);
    let title = crate::pty::format_session_title(agent_id, "?");
    claim_pool_window(
        &config.tmux_session,
        win.index,
        &ssn,
        agent_id,
        &title,
        focus,
    )?;

    let record = ManagedLaunchRecord {
        sessions_session_id: ssn.clone(),
        launch_id: new_launch_id(),
        agent: agent_id.to_string(),
        tmux_session: config.tmux_session.clone(),
        window_index: win.index,
        pane_id: Some(win.pane_id.clone()),
        initial_cwd: normalize_cwd(cwd),
        created_at: Utc::now().to_rfc3339(),
        agent_session_id: None,
        pool: false,
    };
    let _ = save_managed_record(&config.home, &record);

    let spec = LaunchSpec {
        sessions_session_id: ssn.clone(),
        source,
        cwd: normalize_cwd(cwd),
        agent: agent_id.to_string(),
        launch_command: build_quick_launch_command(agent_id),
        workspace_index: None,
        focus,
        window_name: Some(title),
        bootstrap_new_session: false,
        model_id: None,
        user_prompt: None,
    };
    append_entry(config, spec.to_manifest_entry(Path::new(&config.home)))?;
    let _ = crate::session::live_snapshot::remember(config, &ssn);

    Ok(Some(CreatedWindow {
        index: win.index,
        pane_id: win.pane_id,
        sessions_session_id: ssn,
    }))
}

fn agent_matches_window(win: &tmux::TmuxWindow, agent_id: &str) -> bool {
    if win.name == pool_window_name(agent_id) {
        return true;
    }
    if is_pool_window_name(&win.name) {
        return win.name.ends_with(agent_id);
    }
    false
}

/// Ensure one detached spare exists for (agent, cwd). No-op if full or already present.
pub fn ensure_spare(config: &Config, cwd: &str, agent_id: &str) -> Result<()> {
    if !is_poolable_agent(agent_id) {
        return Ok(());
    }
    if !tmux::session_exists(&config.tmux_session) {
        return Ok(());
    }
    let windows = list_windows(&config.tmux_session)?;
    let pool_count = windows.iter().filter(|w| w.pool && !w.pane_dead).count();
    if pool_count >= MAX_POOL_WINDOWS {
        return Ok(());
    }
    let have = windows.iter().any(|w| {
        w.pool && !w.pane_dead && cwd_matches(&w.cwd, cwd) && agent_matches_window(w, agent_id)
    });
    if have {
        return Ok(());
    }
    seed_spare(config, cwd, agent_id)
}

fn seed_spare(config: &Config, cwd: &str, agent_id: &str) -> Result<()> {
    let ssn = new_sessions_session_id();
    let launch = build_quick_launch_command(agent_id);
    if launch.is_empty() {
        return Ok(());
    }
    let name = pool_window_name(agent_id);
    let created = create_managed_window_in_cwd_with_pool(
        &config.tmux_session,
        cwd,
        Some(agent_id),
        Some(launch.as_str()),
        &name,
        false,
        &ssn,
        false,
        true,
    )?;
    let record = ManagedLaunchRecord {
        sessions_session_id: ssn,
        launch_id: new_launch_id(),
        agent: agent_id.to_string(),
        tmux_session: config.tmux_session.clone(),
        window_index: created.index,
        pane_id: Some(created.pane_id),
        initial_cwd: normalize_cwd(cwd),
        created_at: Utc::now().to_rfc3339(),
        agent_session_id: None,
        pool: true,
    };
    let _ = save_managed_record(&config.home, &record);
    debug!(
        "warm pool: seeded {agent_id} @ {} → win {}",
        normalize_cwd(cwd),
        created.index
    );
    Ok(())
}

/// Called from the daemon poll loop: top up spares for live group cwds.
pub fn maintain(config: &Config, live_cwds: &[String]) {
    if !tmux::session_exists(&config.tmux_session) {
        return;
    }
    let mut seen = HashSet::new();
    for cwd in live_cwds {
        let key = normalize_cwd(cwd);
        if key.is_empty() || !seen.insert(key.clone()) {
            continue;
        }
        for agent in POOLABLE_AGENTS {
            if let Err(err) = ensure_spare(config, &key, agent) {
                warn!("warm pool maintain {agent} @ {key}: {err}");
            }
        }
        if let Ok(windows) = list_windows(&config.tmux_session) {
            let n = windows.iter().filter(|w| w.pool && !w.pane_dead).count();
            if n >= MAX_POOL_WINDOWS {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poolable_agents_are_slow_tuis() {
        assert!(is_poolable_agent("grok"));
        assert!(is_poolable_agent("opencode"));
        assert!(!is_poolable_agent("console"));
        assert!(!is_poolable_agent("codex"));
    }

    #[test]
    fn pool_window_name_has_prefix() {
        assert!(is_pool_window_name(&pool_window_name("grok")));
    }

    #[test]
    fn cwd_normalize_strips_trailing_slash() {
        assert!(cwd_matches("/tmp/foo/", "/tmp/foo"));
        assert!(!cwd_matches("/tmp/foo", "/tmp/bar"));
    }
}
