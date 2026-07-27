//! On-disk automation store under `$SESSIONS_DATA_DIR/state/automations/`.

use super::schema::{Automation, AutomationRun, AutomationState};
use crate::config::Config;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::PathBuf;

const JITTER_SALT_FILE: &str = ".run-jitter-salt";

/// Root directory for all automations.
pub fn automations_dir(config: &Config) -> PathBuf {
    config.automations_dir()
}

pub fn automation_dir(config: &Config, id: &str) -> PathBuf {
    automations_dir(config).join(id)
}

pub fn definition_path(config: &Config, id: &str) -> PathBuf {
    automation_dir(config, id).join("automation.toml")
}

pub fn state_path(config: &Config, id: &str) -> PathBuf {
    automation_dir(config, id).join("state.json")
}

pub fn runs_dir(config: &Config, id: &str) -> PathBuf {
    automation_dir(config, id).join("runs")
}

pub fn run_path(config: &Config, automation_id: &str, run_id: &str) -> PathBuf {
    runs_dir(config, automation_id).join(format!("{run_id}.json"))
}

fn is_live_automations_config(config: &Config) -> bool {
    config.automations_dir() == Config::default().automations_dir()
}

/// Panic if tests accidentally target the live user automations dir.
#[cfg(test)]
pub fn assert_isolated_automations_config(config: &Config) {
    if is_live_automations_config(config) {
        panic!(
            "automation persistence must use an isolated config in tests — \
             set config.home to a tempfile, never Config::default() alone. \
             Live path would be: {}",
            config.automations_dir().display()
        );
    }
}

pub fn ensure_root(config: &Config) -> Result<()> {
    fs::create_dir_all(automations_dir(config))
        .with_context(|| format!("create {}", automations_dir(config).display()))?;
    let _ = load_or_create_jitter_salt(config)?;
    Ok(())
}

pub fn load_or_create_jitter_salt(config: &Config) -> Result<String> {
    let path = automations_dir(config).join(JITTER_SALT_FILE);
    if path.exists() {
        let salt = fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?
            .trim()
            .to_string();
        if !salt.is_empty() {
            return Ok(salt);
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let salt = uuid::Uuid::new_v4().to_string();
    fs::write(&path, format!("{salt}\n")).with_context(|| format!("write {}", path.display()))?;
    Ok(salt)
}

pub fn list_automations(config: &Config) -> Result<Vec<Automation>> {
    let root = automations_dir(config);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if id.starts_with('.') {
            continue;
        }
        match load_automation(config, &id) {
            Ok(a) => out.push(a),
            Err(err) => tracing::warn!("skip automation {id}: {err}"),
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

pub fn load_automation(config: &Config, id: &str) -> Result<Automation> {
    let path = definition_path(config, id);
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let a: Automation =
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    if a.id != id {
        // Tolerate mismatch by preferring directory name for identity on disk.
        tracing::debug!(
            "automation id field {} differs from dir {}; using dir id",
            a.id,
            id
        );
    }
    Ok(a)
}

pub fn save_automation(config: &Config, automation: &Automation) -> Result<()> {
    ensure_root(config)?;
    let dir = automation_dir(config, &automation.id);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    fs::create_dir_all(runs_dir(config, &automation.id))?;
    let path = definition_path(config, &automation.id);
    let raw = toml::to_string_pretty(automation).context("serialize automation.toml")?;
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, raw).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
    Ok(())
}

pub fn delete_automation(config: &Config, id: &str) -> Result<()> {
    let dir = automation_dir(config, id);
    if !dir.exists() {
        bail!("automation not found: {id}");
    }
    fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
    Ok(())
}

pub fn load_state(config: &Config, id: &str) -> Result<AutomationState> {
    let path = state_path(config, id);
    if !path.exists() {
        return Ok(AutomationState::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

pub fn save_state(config: &Config, id: &str, state: &AutomationState) -> Result<()> {
    let dir = automation_dir(config, id);
    fs::create_dir_all(&dir)?;
    let path = state_path(config, id);
    let raw = serde_json::to_string_pretty(state)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn save_run(config: &Config, run: &AutomationRun) -> Result<()> {
    let dir = runs_dir(config, &run.automation_id);
    fs::create_dir_all(&dir)?;
    let path = run_path(config, &run.automation_id, &run.id);
    let raw = serde_json::to_string_pretty(run)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn load_run(config: &Config, automation_id: &str, run_id: &str) -> Result<AutomationRun> {
    let path = run_path(config, automation_id, run_id);
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(serde_json::from_str(&raw)?)
}

/// List runs for one automation (newest first). Cap keeps list UIs snappy.
pub fn list_runs(config: &Config, automation_id: &str, limit: usize) -> Result<Vec<AutomationRun>> {
    let dir = runs_dir(config, automation_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut runs = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AutomationRun>(&raw).ok())
        {
            Some(run) => runs.push(run),
            None => tracing::warn!("skip corrupt run file {}", path.display()),
        }
    }
    runs.sort_by_key(|b| std::cmp::Reverse(b.started_at));
    if runs.len() > limit {
        runs.truncate(limit);
    }
    Ok(runs)
}

/// All recent runs across automations (newest first).
pub fn list_all_runs(config: &Config, limit: usize) -> Result<Vec<AutomationRun>> {
    let mut runs = Vec::new();
    for a in list_automations(config)? {
        runs.extend(list_runs(config, &a.id, limit)?);
    }
    runs.sort_by_key(|b| std::cmp::Reverse(b.started_at));
    if runs.len() > limit {
        runs.truncate(limit);
    }
    Ok(runs)
}

pub fn unread_count(config: &Config) -> Result<usize> {
    let mut n = 0;
    for a in list_automations(config)? {
        for run in list_runs(config, &a.id, 50)? {
            if run.is_open_inbox() {
                n += 1;
            }
        }
    }
    Ok(n)
}

pub fn mark_run_read(config: &Config, automation_id: &str, run_id: &str) -> Result<()> {
    let mut run = load_run(config, automation_id, run_id)?;
    run.unread = false;
    save_run(config, &run)
}

pub fn mark_all_read(config: &Config) -> Result<usize> {
    let mut n = 0;
    for a in list_automations(config)? {
        for mut run in list_runs(config, &a.id, 200)? {
            if run.unread {
                run.unread = false;
                save_run(config, &run)?;
                n += 1;
            }
        }
    }
    Ok(n)
}

/// Expand `~`, bare names under `$HOME`, and relative paths to absolute for storage.
/// Shares tilde / bare-name rules with the sidebar path picker.
pub fn expand_cwd(path: &str) -> Result<String> {
    crate::bar::path_picker::expand_path_for_storage(path)
}

/// Isolated config for tests (temp home → state under tmp/.local/share/sessions).
#[cfg(test)]
pub fn isolated_test_config() -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.home = dir.path().to_path_buf();
    (dir, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::schema::{slugify_id, AutomationStatus};

    fn test_config() -> (tempfile::TempDir, Config) {
        isolated_test_config()
    }

    #[test]
    fn save_load_list_delete() {
        let (_tmp, config) = test_config();
        assert_isolated_automations_config(&config);
        ensure_root(&config).unwrap();
        let mut a = Automation::new(
            "daily-ci".into(),
            "Daily CI".into(),
            "check".into(),
            "grok".into(),
            "grok-build".into(),
            "FREQ=DAILY;BYHOUR=9;BYMINUTE=0".into(),
            "/tmp/proj".into(),
        );
        save_automation(&config, &a).unwrap();
        let loaded = load_automation(&config, "daily-ci").unwrap();
        assert_eq!(loaded.name, "Daily CI");
        assert_eq!(list_automations(&config).unwrap().len(), 1);

        a.status = AutomationStatus::Paused;
        a.touch();
        save_automation(&config, &a).unwrap();
        assert_eq!(
            load_automation(&config, "daily-ci").unwrap().status,
            AutomationStatus::Paused
        );

        delete_automation(&config, "daily-ci").unwrap();
        assert!(list_automations(&config).unwrap().is_empty());
    }

    #[test]
    fn run_persistence_and_unread() {
        let (_tmp, config) = test_config();
        assert_isolated_automations_config(&config);
        let a = Automation::new(
            "r1".into(),
            "R1".into(),
            "p".into(),
            "codex".into(),
            "gpt-5.4".into(),
            "FREQ=HOURLY;INTERVAL=1".into(),
            "/tmp".into(),
        );
        save_automation(&config, &a).unwrap();
        let mut run = AutomationRun::new(&a, "/tmp");
        run.status = crate::automation::schema::RunStatus::Done;
        run.unread = true;
        save_run(&config, &run).unwrap();
        assert_eq!(unread_count(&config).unwrap(), 1);
        mark_run_read(&config, "r1", &run.id).unwrap();
        assert_eq!(unread_count(&config).unwrap(), 0);
    }

    #[test]
    fn jitter_salt_stable() {
        let (_tmp, config) = test_config();
        let s1 = load_or_create_jitter_salt(&config).unwrap();
        let s2 = load_or_create_jitter_salt(&config).unwrap();
        assert_eq!(s1, s2);
    }

    #[test]
    fn slugify_used_for_ids() {
        assert_eq!(slugify_id("Hello World"), "hello-world");
    }
}
