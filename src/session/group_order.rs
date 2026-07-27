use crate::config::Config;
use crate::model::Session;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub const MAX_THREADS_PER_GROUP: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SidebarGroupOrder {
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SidebarFoldedGroups {
    pub groups: Vec<String>,
}

pub fn load(config: &Config) -> SidebarGroupOrder {
    let path = config.sidebar_group_order_path();
    if !path.exists() {
        return SidebarGroupOrder::default();
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

pub fn save(config: &Config, order: &SidebarGroupOrder) -> Result<()> {
    atomic_write_json(&config.sidebar_group_order_path(), order)
}

pub fn load_folded(config: &Config) -> HashSet<String> {
    let path = config.sidebar_folded_groups_path();
    if !path.exists() {
        return HashSet::new();
    }
    match fs::read_to_string(&path)
        .ok()
        .and_then(|data| serde_json::from_str::<SidebarFoldedGroups>(&data).ok())
    {
        Some(folded) => folded.groups.into_iter().collect(),
        None => HashSet::new(),
    }
}

pub fn save_folded(config: &Config, folded: &HashSet<String>) -> Result<()> {
    let mut groups: Vec<String> = folded.iter().cloned().collect();
    groups.sort();
    atomic_write_json(
        &config.sidebar_folded_groups_path(),
        &SidebarFoldedGroups { groups },
    )
}

pub fn unique_labels(sessions: &[crate::model::Session]) -> Vec<String> {
    let mut labels: Vec<String> = sessions
        .iter()
        .map(|session| session.cwd_label.clone())
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// Append directories that are not yet in `order`. Does not remove labels for groups
/// that are temporarily absent from the snapshot (e.g. mid-restore) — pruning would
/// destroy saved drag order when `rebuild_rows` runs on partial daemon snapshots.
pub fn ensure_labels(order: &mut Vec<String>, labels: &[String]) {
    let mut missing: Vec<String> = labels
        .iter()
        .filter(|label| !order.iter().any(|saved| saved == *label))
        .cloned()
        .collect();
    missing.sort();
    order.extend(missing);
}

/// Drop labels that are not in `labels`. Only call with a complete live snapshot
/// (never mid-restore partial lists).
pub fn prune_to_labels(order: &mut Vec<String>, labels: &[String]) {
    let present: HashSet<&str> = labels.iter().map(String::as_str).collect();
    order.retain(|label| present.contains(label.as_str()));
}

/// PWD labels ordered by most recent user message in that directory (desc).
/// Used to seed an empty/polluted group-order file from real activity.
pub fn labels_by_recent_activity(sessions: &[Session]) -> Vec<String> {
    let mut best: HashMap<String, chrono::DateTime<chrono::Utc>> = HashMap::new();
    for session in sessions {
        let stamp = session.messaged_at.unwrap_or(session.last_event_at);
        best.entry(session.cwd_label.clone())
            .and_modify(|prev| {
                if stamp > *prev {
                    *prev = stamp;
                }
            })
            .or_insert(stamp);
    }
    let mut labels: Vec<(String, chrono::DateTime<chrono::Utc>)> = best.into_iter().collect();
    labels.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    labels.into_iter().map(|(label, _)| label).collect()
}

/// Reconcile saved drag order with a complete live session list and persist.
///
/// - If saved order is empty (or only junk after prune), seed by recent activity.
/// - Otherwise keep saved positions, append new dirs, drop dirs that are gone.
///
/// Returns `true` when the on-disk order changed.
pub fn reconcile_and_save(
    config: &Config,
    order: &mut Vec<String>,
    sessions: &[Session],
) -> Result<bool> {
    let live_labels = unique_labels(sessions);
    if live_labels.is_empty() {
        // Do not wipe a saved order while the sidebar is empty (boot / down mid-flight).
        return Ok(false);
    }
    let before = order.clone();
    prune_to_labels(order, &live_labels);
    if order.is_empty() {
        *order = labels_by_recent_activity(sessions);
    } else {
        ensure_labels(order, &live_labels);
    }
    if *order == before {
        return Ok(false);
    }
    save(
        config,
        &SidebarGroupOrder {
            groups: order.clone(),
        },
    )?;
    Ok(true)
}

pub fn visible_sessions_in_group(group: &[Session], expanded: bool) -> Vec<Session> {
    let total = group.len();
    if expanded || total <= MAX_THREADS_PER_GROUP {
        return group.to_vec();
    }

    group.iter().take(MAX_THREADS_PER_GROUP).cloned().collect()
}

/// Sidebar session rows in display order (folded pwd headers, show-more, saved group order).
pub fn ordered_visible_sessions(
    sessions: &[Session],
    expanded_groups: &HashSet<String>,
    folded_groups: &HashSet<String>,
    group_order: &[String],
) -> Vec<Session> {
    if sessions.is_empty() {
        return Vec::new();
    }

    let mut by_dir: HashMap<String, Vec<Session>> = HashMap::new();
    for session in sessions {
        by_dir
            .entry(session.cwd_label.clone())
            .or_default()
            .push(session.clone());
    }

    let mut effective_order = group_order.to_vec();
    ensure_labels(&mut effective_order, &unique_labels(sessions));
    let dirs = order_labels(by_dir.keys().cloned().collect(), &effective_order);

    let mut ordered = Vec::new();
    for cwd_label in dirs {
        if folded_groups.contains(&cwd_label) {
            continue;
        }
        let Some(mut group) = by_dir.remove(&cwd_label) else {
            continue;
        };
        group.sort_by(|a, b| a.cmp_within_group(b));
        let expanded = expanded_groups.contains(&cwd_label);
        ordered.extend(visible_sessions_in_group(&group, expanded));
    }
    ordered
}

pub fn order_labels(mut labels: Vec<String>, saved_order: &[String]) -> Vec<String> {
    labels.sort_by(|a, b| {
        let a_pos = saved_order.iter().position(|label| label == a);
        let b_pos = saved_order.iter().position(|label| label == b);
        match (a_pos, b_pos) {
            (Some(a_idx), Some(b_idx)) => a_idx.cmp(&b_idx),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.cmp(b),
        }
    });
    labels
}

pub fn preview_order(order: &[String], from: &str, to: &str) -> Vec<String> {
    let mut preview = order.to_vec();
    reorder(&mut preview, from, to);
    preview
}

pub fn reorder(order: &mut Vec<String>, from: &str, to: &str) {
    if from == to {
        return;
    }
    let Some(from_idx) = order.iter().position(|label| label == from) else {
        return;
    };
    let Some(to_idx) = order.iter().position(|label| label == to) else {
        return;
    };
    let item = order.remove(from_idx);
    let insert_at = if from_idx < to_idx {
        order
            .iter()
            .position(|label| label == to)
            .map(|idx| idx + 1)
            .unwrap_or(order.len())
    } else {
        order.iter().position(|label| label == to).unwrap_or(0)
    };
    order.insert(insert_at.min(order.len()), item);
}

fn atomic_write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let data = serde_json::to_string_pretty(value)?;
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentState, Session};
    use chrono::Utc;
    use tempfile::TempDir;

    fn sample_session(cwd_label: &str) -> Session {
        Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: String::new(),
            tmux_pane_id: String::new(),
            pane_pid: 0,
            agent_session_id: None,
            title: "app · thread".into(),
            description: "thread".into(),
            cwd: "/tmp".into(),
            cwd_label: cwd_label.into(),
            project: "app".into(),
            state: AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: Some(Utc::now()),
            prompt_submitted: true,
            title_manual: false,
            is_active: false,
            last_event_at: Utc::now(),
            ..Default::default()
        }
    }

    #[test]
    fn order_labels_uses_saved_positions() {
        let labels = vec![
            "~/projects/old".into(),
            "~/projects/new".into(),
            "~/projects/mid".into(),
        ];
        let saved = vec![
            "~/projects/new".into(),
            "~/projects/mid".into(),
            "~/projects/old".into(),
        ];
        assert_eq!(
            order_labels(labels, &saved),
            vec!["~/projects/new", "~/projects/mid", "~/projects/old"]
        );
    }

    #[test]
    fn ensure_labels_appends_new_directories_alphabetically() {
        let mut order = vec!["~/projects/b".into()];
        ensure_labels(
            &mut order,
            &[
                "~/projects/b".into(),
                "~/projects/a".into(),
                "~/projects/c".into(),
            ],
        );
        assert_eq!(order, vec!["~/projects/b", "~/projects/a", "~/projects/c"]);
    }

    #[test]
    fn ensure_labels_preserves_saved_order_when_snapshot_is_partial() {
        let mut order = vec![
            "~/projects/b".into(),
            "~/projects/a".into(),
            "~/projects/c".into(),
        ];
        ensure_labels(&mut order, &["~/projects/a".into()]);
        assert_eq!(order, vec!["~/projects/b", "~/projects/a", "~/projects/c"]);
    }

    #[test]
    fn prune_to_labels_drops_absent_directories() {
        let mut order = vec!["~/projects/b".into(), "~/a".into(), "~/projects/a".into()];
        prune_to_labels(&mut order, &["~/projects/a".into(), "~/projects/b".into()]);
        assert_eq!(order, vec!["~/projects/b", "~/projects/a"]);
    }

    #[test]
    fn reconcile_and_save_seeds_activity_order_when_saved_is_junk() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut older = sample_session("~/projects/old");
        older.messaged_at = Some(Utc::now() - chrono::Duration::hours(2));
        let mut newer = sample_session("~/projects/new");
        newer.messaged_at = Some(Utc::now());
        let sessions = vec![older, newer];
        let mut order = vec!["~/b".into(), "~/a".into()];
        assert!(reconcile_and_save(&config, &mut order, &sessions).unwrap());
        assert_eq!(order, vec!["~/projects/new", "~/projects/old"]);
        assert_eq!(load(&config).groups, order);
    }

    #[test]
    fn reconcile_and_save_keeps_drag_order_for_live_labels() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let sessions = vec![
            sample_session("~/projects/a"),
            sample_session("~/projects/b"),
            sample_session("~/projects/c"),
        ];
        let mut order = vec![
            "~/projects/c".into(),
            "~/projects/a".into(),
            "~/gone".into(),
        ];
        assert!(reconcile_and_save(&config, &mut order, &sessions).unwrap());
        assert_eq!(order, vec!["~/projects/c", "~/projects/a", "~/projects/b"]);
    }

    #[test]
    fn preview_order_does_not_mutate_saved_order() {
        let saved = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let preview = preview_order(&saved, "a", "d");
        assert_eq!(preview, vec!["b", "c", "d", "a"]);
        assert_eq!(saved, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn reorder_moves_group_down_after_target() {
        let mut order = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        reorder(&mut order, "a", "d");
        assert_eq!(order, vec!["b", "c", "d", "a"]);
    }

    #[test]
    fn reorder_moves_group_up_before_target() {
        let mut order = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        reorder(&mut order, "d", "a");
        assert_eq!(order, vec!["d", "a", "b", "c"]);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let order = SidebarGroupOrder {
            groups: vec!["~/projects/a".into(), "~/projects/b".into()],
        };
        save(&config, &order).unwrap();
        assert_eq!(load(&config), order);
    }

    #[test]
    fn unique_labels_dedupes_and_sorts() {
        let sessions = vec![
            sample_session("~/b"),
            sample_session("~/a"),
            sample_session("~/b"),
        ];
        assert_eq!(unique_labels(&sessions), vec!["~/a", "~/b"]);
    }

    #[test]
    fn ordered_visible_sessions_skips_folded_pwd_groups() {
        let mut first = sample_session("~/a");
        first.tab_index = 1;
        let mut second = sample_session("~/b");
        second.tab_index = 2;
        let sessions = vec![first, second];
        let mut folded = HashSet::new();
        folded.insert("~/a".into());
        let visible = ordered_visible_sessions(&sessions, &HashSet::new(), &folded, &[]);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].tab_index, 2);
    }

    #[test]
    fn save_and_load_folded_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.home = dir.path().to_path_buf();
        let mut folded = HashSet::new();
        folded.insert("~/projects/a".into());
        folded.insert("~/projects/b".into());
        save_folded(&config, &folded).unwrap();
        assert_eq!(load_folded(&config), folded);
    }
}
