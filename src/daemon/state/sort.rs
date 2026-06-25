// WS-08: extracted from daemon/state.rs

use crate::config::Config;
use crate::daemon::state::identity::apply_agent_suppression;
use crate::model::Session;
use crate::session::group_order::{self, ordered_visible_sessions};
use std::collections::{HashMap, HashSet};

pub(crate) fn resolve_focus_target(
    config: &Config,
    sessions: &HashMap<String, Session>,
    suppressed: &HashSet<String>,
    window_index: u32,
    tab_index: Option<u32>,
) -> anyhow::Result<u32> {
    if let Some(index) = tab_index {
        return Ok(index);
    }
    let group_order = group_order::load(config).groups;
    let folded_groups = group_order::load_folded(config);
    ordered_visible_sessions(
        &sorted_sessions(sessions.values(), suppressed),
        &HashSet::new(),
        &folded_groups,
        &group_order,
    )
    .get(window_index.saturating_sub(1) as usize)
    .map(|session| session.tab_index)
    .ok_or_else(|| anyhow::anyhow!("session {window_index} is out of range"))
}

pub(crate) fn sorted_sessions<'a>(
    sessions: impl IntoIterator<Item = &'a Session>,
    suppressed: &HashSet<String>,
) -> Vec<Session> {
    let mut list: Vec<_> = sessions.into_iter().cloned().collect();
    for session in &mut list {
        if suppressed.contains(&session.id) {
            apply_agent_suppression(session);
        }
    }
    let mut by_dir: HashMap<String, Vec<Session>> = HashMap::new();
    for session in &list {
        by_dir
            .entry(session.cwd_label.clone())
            .or_default()
            .push(session.clone());
    }
    list.sort_by(|a, b| {
        if a.cwd_label == b.cwd_label {
            return a.cmp_within_group(b);
        }
        Session::cmp_groups(
            by_dir.get(&a.cwd_label).map(Vec::as_slice).unwrap_or(&[]),
            by_dir.get(&b.cwd_label).map(Vec::as_slice).unwrap_or(&[]),
        )
        .then_with(|| a.cwd_label.cmp(&b.cwd_label))
    });
    list
}
