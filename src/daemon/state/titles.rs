// WS-08: extracted from daemon/state.rs

use crate::config::Config;
use crate::daemon::state::identity::same_agent_session;
use crate::model::Session;
use std::path::Path;
use tracing::warn;

pub(crate) fn resolve_renamed_title(session: &Session, input: &str) -> (String, String, String) {
    let input = input.trim();
    if input.contains(" · ") {
        let description = crate::pty::parse_description(input);
        let project = crate::pty::resolve_agent_app(input);
        return (input.to_string(), description, project);
    }

    if crate::pty::is_console_session(&session.description, &session.title) {
        let project = session.project.clone();
        return (input.to_string(), input.to_string(), project);
    }

    let app = crate::pty::parse_app(&session.title)
        .filter(|app| crate::pty::is_agent_app(app))
        .unwrap_or_else(|| session.project.clone());
    let title = crate::pty::format_session_title(&app, input);
    let project = crate::pty::resolve_agent_app(&title);
    (title, input.to_string(), project)
}
pub(crate) fn agent_session_title_is_placeholder(session: &Session) -> bool {
    session.agent_session_id.is_some()
        && (crate::pty::is_weak_thread_name(&session.description)
            || crate::pty::is_machine_derived_thread(&session.description)
            || crate::pty::is_bootstrap_sidebar_thread(&session.description)
            || !crate::pty::is_sticky_thread_title(&session.description))
}

pub(crate) fn persist_agent_session_title(config: &Config, session: &Session) {
    let Some(sid) = session.agent_session_id.as_deref() else {
        return;
    };
    if !crate::pty::is_sticky_thread_title(&session.description)
        || crate::pty::is_bootstrap_sidebar_thread(&session.description)
    {
        return;
    }
    let path = config.session_title_path(sid);
    let result = path
        .parent()
        .map(std::fs::create_dir_all)
        .transpose()
        .and_then(|_| std::fs::write(&path, format!("{}\n", session.title)));
    if let Err(err) = result {
        warn!("write session title failed for {sid}: {err}");
    }
}

/// Poll often resolves titles before the agent session id is attached — restore and
/// persist once we know the binding.
pub(crate) fn ensure_agent_session_title(
    config: &Config,
    existing: &Session,
    session: &mut Session,
) {
    if session_has_manual_title(config, existing) || session_has_manual_title(config, session) {
        return;
    }
    if session.agent_session_id.is_none() {
        return;
    }
    if crate::pty::is_sticky_thread_title(&session.description)
        && !crate::pty::is_bootstrap_sidebar_thread(&session.description)
    {
        persist_agent_session_title(config, session);
        return;
    }
    if same_agent_session(existing, session)
        && crate::pty::is_sticky_thread_title(&existing.description)
        && !crate::pty::is_bootstrap_sidebar_thread(&existing.description)
    {
        session.title = existing.title.clone();
        session.description = existing.description.clone();
        session.project = existing.project.clone();
        persist_agent_session_title(config, session);
        return;
    }
    if let Some((title, description, project)) = load_auto_persisted_title_identity(config, session)
    {
        session.title = title;
        session.description = description;
        session.project = project;
        persist_agent_session_title(config, session);
    }
}
pub(crate) fn session_has_manual_title(config: &Config, session: &Session) -> bool {
    session.title_manual || manual_title_marker_exists(config, session.tab_index)
}

pub(crate) fn manual_title_marker_exists(config: &Config, tab_index: u32) -> bool {
    config.session_title_manual_path_for_tab(tab_index).exists()
}

pub(crate) fn apply_manual_title_identity(
    config: &Config,
    existing: &Session,
    fresh: &mut Session,
) -> bool {
    if !session_has_manual_title(config, existing) {
        return false;
    }
    let manual_title =
        std::fs::read_to_string(config.session_title_path_for_tab(existing.tab_index))
            .ok()
            .map(|data| data.trim().to_string())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| existing.title.clone());
    if tab_manual_title_is_stale_for_poll(fresh, &manual_title) {
        let _ = clear_manual_title_files_for_tab(config, existing.tab_index);
        fresh.title_manual = false;
        return false;
    }
    if restore_manual_title_from_tab(config, existing.tab_index, fresh) {
        return true;
    }
    if restore_manual_title_from_existing(existing, fresh) {
        fresh.title_manual = true;
        return true;
    }
    false
}

pub(crate) fn restore_manual_title_from_existing(existing: &Session, fresh: &mut Session) -> bool {
    if !existing.title_manual || existing.title.trim().is_empty() {
        return false;
    }
    fresh.title = existing.title.clone();
    fresh.description = existing.description.clone();
    fresh.project = existing.project.clone();
    true
}

pub(crate) fn restore_manual_title_from_tab(
    config: &Config,
    tab_index: u32,
    session: &mut Session,
) -> bool {
    if !manual_title_marker_exists(config, tab_index) {
        return false;
    }
    let Ok(data) = std::fs::read_to_string(config.session_title_path_for_tab(tab_index)) else {
        return false;
    };
    let manual_title = data.trim();
    if tab_manual_title_is_stale_for_poll(session, manual_title) {
        let _ = clear_manual_title_files_for_tab(config, tab_index);
        return false;
    }
    if !restore_title_from_tab_index(config, tab_index, session) {
        return false;
    }
    session.title_manual = true;
    true
}

pub(crate) fn reset_unconfirmed_agent_title(config: &Config, session: &mut Session) {
    let app = session
        .agent_session_id
        .as_deref()
        .and_then(|sid| {
            crate::agents::infer_agent_for_session(&config.home, &session.cwd, sid)
                .map(|agent| agent.id().to_string())
        })
        .filter(|app| crate::pty::is_agent_app(app))
        .or_else(|| {
            crate::pty::parse_app(&session.title).filter(|app| crate::pty::is_agent_app(app))
        })
        .unwrap_or_else(|| "grok".into());
    let (title, description) = crate::pty::session_names(&app, "?");
    session.title = title;
    session.description = description;
    session.project = app;
    session.title_manual = false;
}

pub(crate) fn load_auto_persisted_title_identity(
    config: &Config,
    session: &Session,
) -> Option<(String, String, String)> {
    if manual_title_marker_exists(config, session.tab_index) {
        return None;
    }
    let mut scratch = session.clone();
    let agent_session_id = session.agent_session_id.clone();
    if let Some(sid) = agent_session_id.as_deref() {
        let lookup_cwd = crate::agents::disk_lookup_cwd(&config.home, &session.cwd, Some(sid));
        let commenced = session.messaged_at.is_some()
            || crate::agents::session_has_commenced(&config.home, &lookup_cwd, sid);
        if commenced {
            if let Some((summary, agent)) =
                crate::agents::load_session_summary(&config.home, &lookup_cwd, sid)
            {
                if let Some(thread) = crate::agents::thread_title_from_summary(&summary, agent) {
                    if !crate::pty::is_sticky_thread_title(&thread) {
                        return None;
                    }
                    let app = if crate::pty::is_agent_app(agent) {
                        agent.to_string()
                    } else {
                        crate::pty::resolve_agent_app(&session.title)
                    };
                    let (title, description) = crate::pty::session_names(&app, &thread);
                    return Some((title, description, app));
                }
            }
            if restore_title_from_disk(config, sid, &mut scratch)
                && crate::pty::is_sticky_thread_title(&scratch.description)
                && !crate::pty::is_bootstrap_sidebar_thread(&scratch.description)
            {
                return Some((
                    scratch.title.clone(),
                    scratch.description.clone(),
                    scratch.project.clone(),
                ));
            }
        }
    }
    None
}

pub(crate) fn write_manual_session_title_files(
    config: &Config,
    tab_index: u32,
    title: &str,
) -> std::io::Result<()> {
    let payload = format!("{title}\n");
    std::fs::create_dir_all(config.grok_state_dir())?;
    std::fs::write(config.session_title_path_for_tab(tab_index), &payload)?;
    std::fs::write(config.session_title_manual_path_for_tab(tab_index), "1\n")?;
    Ok(())
}

pub(crate) fn clear_manual_title_files_for_tab(
    config: &Config,
    tab_index: u32,
) -> std::io::Result<()> {
    let title_path = config.session_title_path_for_tab(tab_index);
    let manual_path = config.session_title_manual_path_for_tab(tab_index);
    if title_path.exists() {
        std::fs::remove_file(title_path)?;
    }
    if manual_path.exists() {
        std::fs::remove_file(manual_path)?;
    }
    Ok(())
}
pub(crate) fn tab_manual_title_is_stale_for_poll(polled: &Session, manual_title: &str) -> bool {
    if polled.agent_session_id.is_some() {
        return false;
    }
    if !is_foreground_tool_label(manual_title) {
        return false;
    }
    let manual_desc = crate::pty::parse_description(manual_title);
    polled.description != manual_desc && !polled.title.eq_ignore_ascii_case(manual_title)
}

pub(crate) fn is_foreground_tool_label(title: &str) -> bool {
    if title.contains(" · ") {
        return false;
    }
    let desc = crate::pty::parse_description(title);
    let lower = desc.to_ascii_lowercase();
    crate::pty::is_agent_app(&lower) || crate::pty::get_app_profile(&lower).is_some()
}

pub(crate) fn restore_title_from_tab_index(
    config: &Config,
    tab_index: u32,
    session: &mut Session,
) -> bool {
    let Ok(data) = std::fs::read_to_string(config.session_title_path_for_tab(tab_index)) else {
        return false;
    };
    let title = data.trim();
    if crate::pty::is_weak_session_title(title) {
        return false;
    }
    session.title = title.to_string();
    session.description = crate::pty::parse_description(title);
    session.project = crate::pty::resolve_agent_app(title);
    true
}
pub(crate) fn restore_title_from_disk(
    config: &Config,
    agent_session_id: &str,
    session: &mut Session,
) -> bool {
    let Ok(data) = std::fs::read_to_string(config.session_title_path(agent_session_id)) else {
        return false;
    };
    let title = data.trim();
    if crate::pty::is_weak_session_title(title)
        || crate::pty::is_machine_derived_thread(&crate::pty::parse_description(title))
    {
        return false;
    }
    session.title = title.to_string();
    session.description = crate::pty::parse_description(title);
    session.project = crate::pty::resolve_agent_app(title);
    true
}

pub(crate) fn path_leaf(path: &str, home: &Path) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "~".into();
    }
    if trimmed == home.to_string_lossy().as_ref() {
        return "~".into();
    }
    Path::new(trimmed)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| trimmed.to_string())
}
