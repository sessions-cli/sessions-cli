use std::path::Path;
use std::process::Command;

use crate::agents::codex::disk::rollout_path_for_thread;
use crate::agents::common::notify_binary::hook_binary;
use crate::agents::detect_agent_id_for_session;
use crate::agents::grok::hooks::present as grok_present;
use crate::agents::launcher::agent_session_exists;
use crate::agents::looks_like_shell_command;
use crate::config::Config;
use crate::daemon::persist::load_state_or_empty;
use crate::hooks;
use crate::session::{
    launch_command_needs_shell, load_manifest, manifest_entry_for_ssn, repair_manifest,
    save_manifest, ManifestEntry, ManifestRepairReport, SessionManifest,
};
use crate::telemetry::config::SessionsConfig;
use crate::version::VERSION;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub label: String,
    pub ok: bool,
    pub detail: String,
    pub fix: Option<String>,
}

pub fn install_checks(home: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    let binary = hook_binary(home);
    let path_link = home.join(".local/bin/sessions");
    let install_dir = crate::paths::install_dir(home);
    let data_root = crate::paths::data_root(home);

    checks.push(binary_installed(&binary));
    checks.push(binary_runs(&binary));
    if cfg!(target_os = "macos") {
        checks.push(codesign_valid(&binary));
    }
    checks.push(path_entry(&path_link, &binary));
    checks.push(grok_legacy_binary_link(home, &binary));
    checks.push(data_layout(home, &data_root, &install_dir));
    checks.extend(agent_hook_checks(home));
    if cfg!(target_os = "macos") {
        checks.push(ghostty_keybind_check(home));
    }
    checks.push(cursor_keybind_check(home));
    checks.push(ide_terminal_color_check(home));
    checks.push(version_check());
    checks.push(terminal_host_check());
    checks.push(telemetry_config_check(home));
    checks.push(version_below_min_check(home));
    checks.extend(manifest_checks(home));
    checks.push(stale_managed_files_check(home));
    checks.extend(tmux_runtime_checks(home));
    checks.extend(companion_checks(home));
    checks
}

/// Focused install/runtime readiness for Settings + doctor (skillshare, Obot, daemon).
pub fn readiness_checks(home: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    let binary = hook_binary(home);
    checks.push(binary_installed(&binary));
    checks.push(binary_runs(&binary));
    checks.push(sessionsd_ready_check(home));
    checks.push(tmux_present_check());
    checks.extend(companion_checks(home));
    checks.extend(agent_hook_checks(home));
    checks
}

fn sessionsd_ready_check(home: &Path) -> Check {
    let config = config_from_home(home);
    let ok = crate::daemon::server::socket_responds(&config.socket_path);
    Check {
        label: "sessionsd".into(),
        ok,
        detail: if ok {
            "daemon responding".into()
        } else {
            format!("not responding ({})", config.socket_path.display())
        },
        fix: (!ok).then(|| "run sessions up  or  bin/start-sessionsd.sh".into()),
    }
}

fn tmux_present_check() -> Check {
    let ok = Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    Check {
        label: "tmux".into(),
        ok,
        detail: if ok {
            "installed".into()
        } else {
            "not found on PATH".into()
        },
        fix: (!ok).then(|| "install tmux (brew install tmux)".into()),
    }
}

fn companion_checks(home: &Path) -> Vec<Check> {
    vec![skillshare_check(home), obot_check(home), docker_check()]
}

fn skillshare_check(home: &Path) -> Check {
    let status = crate::skills::status(home);
    if !status.installed {
        return Check {
            label: "skillshare".into(),
            ok: false,
            detail: "binary not found".into(),
            fix: Some("open Skills panel (⌘K) and press Enter to set up automatically".into()),
        };
    }
    let version = status.version.as_deref().unwrap_or("ok");
    if status.store_exists {
        Check {
            label: "skillshare".into(),
            ok: true,
            detail: format!("{version} · store ready"),
            fix: None,
        }
    } else {
        Check {
            label: "skillshare".into(),
            ok: false,
            detail: format!("{version} · store missing"),
            fix: Some("open Skills (⌘K) → set up, or run skillshare init".into()),
        }
    }
}

fn obot_check(home: &Path) -> Check {
    match crate::mcp::health(home) {
        Ok(h) => match h.status {
            crate::mcp::ObotHealthStatus::Up => Check {
                label: "Obot".into(),
                ok: true,
                detail: format!("up · {}", h.base_url),
                fix: None,
            },
            crate::mcp::ObotHealthStatus::Disabled => Check {
                label: "Obot".into(),
                ok: true,
                detail: "disabled in obot.toml".into(),
                fix: None,
            },
            crate::mcp::ObotHealthStatus::Down => Check {
                label: "Obot".into(),
                ok: false,
                detail: format!("down · {} ({})", h.base_url, h.detail),
                fix: Some(
                    "open MCPs panel (⌘M) and press Enter to set up Obot (needs Docker)".into(),
                ),
            },
        },
        Err(err) => Check {
            label: "Obot".into(),
            ok: false,
            detail: err.to_string(),
            fix: Some("open MCPs (⌘M) → set up automatically".into()),
        },
    }
}

fn docker_check() -> Check {
    let output = Command::new("docker").arg("info").output();
    match output {
        Ok(out) if out.status.success() => Check {
            label: "Docker".into(),
            ok: true,
            detail: "daemon ready (for optional Obot MCP)".into(),
            fix: None,
        },
        Ok(_) => Check {
            label: "Docker".into(),
            ok: false,
            detail: "cli found but daemon not ready (optional — for Obot)".into(),
            fix: Some("start Docker Desktop, then open MCPs (⌘M) to set up Obot".into()),
        },
        Err(_) => Check {
            label: "Docker".into(),
            ok: false,
            detail: "not installed (optional — only needed for Obot MCP gateway)".into(),
            fix: Some("install Docker Desktop if you want hosted MCP via Obot".into()),
        },
    }
}

pub fn run_repair(home: &Path) -> anyhow::Result<ManifestRepairReport> {
    let config = config_from_home(home);
    let report = repair_manifest(&config)?;
    repair_manifest_agents(&config)?;
    // Drop bare `tmux attach -t agents` clients that fight the sidebar for focus.
    // Never detaches the nested workspace attach inside sessions-ui.
    match crate::daemon::tmux::detach_stray_agents_clients(
        &config.tmux_ui_session,
        &config.tmux_session,
    ) {
        Ok(detached) if !detached.is_empty() => {
            eprintln!(
                "detached {} stray client(s) from {}: {}",
                detached.len(),
                config.tmux_session,
                detached.join(", ")
            );
        }
        Ok(_) => {}
        Err(err) => {
            eprintln!("warning: could not detach stray agents clients: {err}");
        }
    }
    Ok(report)
}

fn config_from_home(home: &Path) -> Config {
    Config {
        home: home.to_path_buf(),
        state_path: crate::paths::state_dir(home).join("sessionsd.json"),
        ..Default::default()
    }
}

fn manifest_checks(home: &Path) -> Vec<Check> {
    let config = config_from_home(home);
    let mut checks = Vec::new();
    let path = crate::session::manifest_path(home);
    if !path.exists() {
        checks.push(Check {
            label: "Session manifest".into(),
            ok: true,
            detail: "not created yet (fresh install)".into(),
            fix: None,
        });
        return checks;
    }

    let manifest = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|data| serde_json::from_str::<SessionManifest>(&data).ok())
    {
        Some(manifest) => manifest,
        None => {
            return vec![Check {
                label: "Session manifest".into(),
                ok: false,
                detail: format!("invalid JSON at {}", path.display()),
                fix: Some("inspect or remove session-manifest.json and re-run sessions up".into()),
            }];
        }
    };

    if let Some(ref from) = manifest.migrated_from {
        checks.push(Check {
            label: "Manifest migration".into(),
            ok: true,
            detail: format!("manifest_migrated_from: {from}"),
            fix: None,
        });
    }

    let open_entries = manifest
        .entries
        .iter()
        .filter(|entry| !entry.closed)
        .count();
    let tmux_detail = match crate::daemon::tmux::list_windows(&config.tmux_session) {
        Ok(windows) => {
            let tmux_count = windows.len();
            let ok = open_entries == tmux_count;
            checks.push(Check {
                label: "Manifest vs tmux".into(),
                ok,
                detail: format!("open manifest entries={open_entries}, tmux windows={tmux_count}"),
                fix: (!ok).then(|| {
                    "run sessions up to restore missing windows or sessions doctor --repair for stale rows".into()
                }),
            });
            format!("{open_entries} open entries")
        }
        Err(_) => {
            checks.push(Check {
                label: "Manifest vs tmux".into(),
                ok: true,
                detail: format!("open manifest entries={open_entries}, tmux n/a"),
                fix: None,
            });
            format!("{open_entries} open entries")
        }
    };

    checks.push(corrupted_launch_command_check(&manifest));
    checks.push(agent_session_id_drift_check(&config, &manifest));
    checks.push(manifest_agent_mismatch_check(home, &manifest));
    checks.push(restore_disk_stale_check(home, &manifest));
    checks.push(codex_home_drift_check(home, &manifest));

    checks.insert(
        0,
        Check {
            label: "Session manifest".into(),
            ok: true,
            detail: tmux_detail,
            fix: None,
        },
    );
    checks
}

fn corrupted_launch_command_check(manifest: &SessionManifest) -> Check {
    let corrupted: Vec<_> = manifest
        .entries
        .iter()
        .filter(|entry| !entry.closed)
        .filter(|entry| {
            launch_command_needs_shell(entry) && !looks_like_shell_command(&entry.launch_command)
        })
        .map(|entry| entry.sessions_session_id.as_str())
        .collect();
    let count = corrupted.len();
    Check {
        label: "Manifest launch commands".into(),
        ok: count == 0,
        detail: if count == 0 {
            "all open launch_command values look like shell commands".into()
        } else {
            format!(
                "{count} open entries have corrupted launch_command ({})",
                corrupted.join(", ")
            )
        },
        fix: (count > 0).then(|| {
            "run sessions doctor --repair to rewrite quick-launch or resume commands".into()
        }),
    }
}

fn is_generic_manifest_agent(agent: &str) -> bool {
    agent.is_empty() || agent == "console" || agent == "session"
}

fn inferred_manifest_agent(home: &Path, entry: &ManifestEntry) -> Option<String> {
    if let Some(agent_session_id) = entry.agent_session_id.as_deref() {
        if let Some(agent) = detect_agent_id_for_session(home, agent_session_id) {
            return Some(agent.to_string());
        }
    }
    entry.title.as_deref().and_then(|title| {
        crate::pty::parse_app(title)
            .filter(|app| crate::pty::is_agent_app(app))
            .map(|app| app.to_ascii_lowercase())
    })
}

fn resolve_agent_for_disk_probe(home: &Path, entry: &ManifestEntry) -> Option<String> {
    if !is_generic_manifest_agent(&entry.agent) {
        return Some(entry.agent.clone());
    }
    inferred_manifest_agent(home, entry)
}

fn manifest_agent_mismatch_check(home: &Path, manifest: &SessionManifest) -> Check {
    let mismatched: Vec<_> = manifest
        .entries
        .iter()
        .filter(|entry| !entry.closed)
        .filter(|entry| is_generic_manifest_agent(&entry.agent))
        .filter(|entry| inferred_manifest_agent(home, entry).is_some())
        .map(|entry| entry.sessions_session_id.as_str())
        .collect();
    let count = mismatched.len();
    Check {
        label: "Manifest agent mismatch".into(),
        ok: count == 0,
        detail: if count == 0 {
            "manifest agent matches title or disk for open entries".into()
        } else {
            format!(
                "{count} open entries have console/session agent but title or disk implies a provider ({})",
                mismatched.join(", ")
            )
        },
        fix: (count > 0).then(|| {
            "run sessions doctor --repair to rewrite agent from title prefix or disk detect, or wait for daemon back-sync".into()
        }),
    }
}

fn restore_disk_stale_check(home: &Path, manifest: &SessionManifest) -> Check {
    let stale: Vec<_> = manifest
        .entries
        .iter()
        .filter(|entry| !entry.closed)
        .filter_map(|entry| {
            let agent_session_id = entry.agent_session_id.as_deref()?;
            let agent = resolve_agent_for_disk_probe(home, entry)?;
            if agent_session_exists(&agent, agent_session_id) {
                return None;
            }
            Some(entry.sessions_session_id.as_str())
        })
        .collect();
    let count = stale.len();
    Check {
        label: "Restore disk stale".into(),
        ok: true,
        detail: if count == 0 {
            "all bound agent_session_id values exist on disk".into()
        } else {
            format!(
                "warn: {count} open entries have agent_session_id missing on disk ({}) — restore will quick-launch",
                stale.join(", ")
            )
        },
        fix: (count > 0).then(|| {
            "close ended sessions, run sessions doctor --repair to tombstone orphans, or send a new prompt to rebind".into()
        }),
    }
}

fn codex_home_drift_check(home: &Path, manifest: &SessionManifest) -> Check {
    let Some(custom_root) = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
    else {
        return Check {
            label: "Codex home drift".into(),
            ok: true,
            detail: "CODEX_HOME unset".into(),
            fix: None,
        };
    };

    let default_root = home.join(".codex");
    let drifted: Vec<_> = manifest
        .entries
        .iter()
        .filter(|entry| !entry.closed)
        .filter_map(|entry| {
            let agent_session_id = entry.agent_session_id.as_deref()?;
            let agent = resolve_agent_for_disk_probe(home, entry)?;
            if agent != "codex" {
                return None;
            }
            let on_default = rollout_path_for_thread(&default_root, agent_session_id).is_some();
            let on_custom = rollout_path_for_thread(&custom_root, agent_session_id).is_some();
            if !on_default && on_custom {
                Some(entry.sessions_session_id.as_str())
            } else {
                None
            }
        })
        .collect();
    let count = drifted.len();
    Check {
        label: "Codex home drift".into(),
        ok: true,
        detail: if count == 0 {
            format!(
                "CODEX_HOME={} — codex sessions resolve under custom home",
                custom_root.display()
            )
        } else {
            format!(
                "warn: CODEX_HOME={} — {count} codex entries only exist under custom home ({})",
                custom_root.display(),
                drifted.join(", ")
            )
        },
        fix: (count > 0).then(|| {
            "ensure daemon inherits CODEX_HOME from your shell profile, or symlink default ~/.codex to CODEX_HOME".into()
        }),
    }
}

fn repair_manifest_agents(config: &Config) -> anyhow::Result<Vec<String>> {
    if !config.session_manifest_path().exists() {
        return Ok(Vec::new());
    }

    let mut manifest = load_manifest(config)?;
    let mut rewritten = Vec::new();
    let mut changed = false;

    for entry in &mut manifest.entries {
        if entry.closed || !is_generic_manifest_agent(&entry.agent) {
            continue;
        }
        let Some(agent) = inferred_manifest_agent(&config.home, entry) else {
            continue;
        };
        if entry.agent != agent {
            entry.agent = agent;
            rewritten.push(entry.sessions_session_id.clone());
            changed = true;
        }
    }

    if changed {
        save_manifest(config, &manifest)?;
    }
    Ok(rewritten)
}

fn agent_session_id_drift_check(config: &Config, manifest: &SessionManifest) -> Check {
    let state = load_state_or_empty(config);
    let mut drifted = Vec::new();
    for session in &state.sessions {
        let Some(ref sessions_session_id) = session.sessions_session_id else {
            continue;
        };
        if session.agent_session_id.is_none() {
            continue;
        }
        let Some(entry) = manifest_entry_for_ssn(manifest, sessions_session_id) else {
            continue;
        };
        if entry.agent_session_id.is_none() {
            drifted.push(sessions_session_id.clone());
        }
    }
    let count = drifted.len();
    Check {
        label: "Manifest agent_session_id".into(),
        ok: count == 0,
        detail: if count == 0 {
            "manifest matches sessionsd agent_session_id for open entries".into()
        } else {
            format!(
                "{count} open entries missing agent_session_id in manifest ({})",
                drifted.join(", ")
            )
        },
        fix: (count > 0).then(|| {
            "run sessions doctor --repair to backfill from sessionsd, or wait for daemon back-sync"
                .into()
        }),
    }
}

fn stale_managed_files_check(home: &Path) -> Check {
    let managed_dir = crate::session::managed::managed_state_dir(home);
    if !managed_dir.is_dir() {
        return Check {
            label: "Stale managed files".into(),
            ok: true,
            detail: "no managed records".into(),
            fix: None,
        };
    }

    let open_ssns = crate::session::manifest_path(home)
        .exists()
        .then(|| {
            std::fs::read_to_string(crate::session::manifest_path(home))
                .ok()
                .and_then(|data| {
                    serde_json::from_str::<crate::session::SessionManifest>(&data).ok()
                })
        })
        .flatten()
        .map(|manifest| {
            manifest
                .entries
                .iter()
                .filter(|entry| !entry.closed)
                .map(|entry| entry.sessions_session_id.clone())
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let tmux_session = config_from_home(home).tmux_session;
    let live_windows = crate::daemon::tmux::list_windows(&tmux_session)
        .ok()
        .map(|windows| {
            windows
                .into_iter()
                .map(|window| window.index)
                .collect::<std::collections::HashSet<_>>()
        });

    let mut total = 0usize;
    let mut stale = 0usize;
    let entries = match std::fs::read_dir(&managed_dir) {
        Ok(entries) => entries,
        Err(_) => {
            return Check {
                label: "Stale managed files".into(),
                ok: true,
                detail: "managed directory unreadable".into(),
                fix: None,
            };
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        total += 1;
        let Ok(data) = std::fs::read_to_string(&path) else {
            stale += 1;
            continue;
        };
        let Ok(record) =
            serde_json::from_str::<crate::session::managed::ManagedLaunchRecord>(&data)
        else {
            stale += 1;
            continue;
        };
        let not_open = !open_ssns.contains(&record.sessions_session_id);
        let window_gone = live_windows
            .as_ref()
            .is_some_and(|live| !live.is_empty() && !live.contains(&record.window_index));
        if not_open || window_gone {
            stale += 1;
        }
    }

    Check {
        label: "Stale managed files".into(),
        ok: stale == 0,
        detail: if total == 0 {
            "no managed records".into()
        } else if stale == 0 {
            format!("{total} managed records")
        } else {
            format!("{stale} stale of {total} managed records")
        },
        fix: (stale > 0).then(|| {
            "close ended sessions or run sessions up to reconcile managed launch records".into()
        }),
    }
}

fn version_check() -> Check {
    Check {
        label: "Version".into(),
        ok: true,
        detail: VERSION.to_string(),
        fix: None,
    }
}

fn terminal_host_check() -> Check {
    let ui_session = Config::default().tmux_ui_session;
    let host = crate::bar::host_terminal::detect_for_ui_session(Some(&ui_session));
    Check {
        label: "Terminal host".into(),
        ok: true,
        detail: host.detail(),
        fix: None,
    }
}

fn telemetry_config_check(home: &Path) -> Check {
    let path = crate::paths::config_path(home);
    let ok = !path.exists() || std::fs::OpenOptions::new().write(true).open(&path).is_ok();
    let level = SessionsConfig::load(home)
        .map(|c| c.telemetry.level.as_str().to_string())
        .unwrap_or_else(|_| "unknown".into());
    Check {
        label: "Telemetry config".into(),
        ok,
        detail: if path.exists() {
            format!("level={level}")
        } else {
            "config will be created on install".into()
        },
        fix: (!ok).then(|| "ensure ~/.config/sessions is writable".into()),
    }
}

fn version_below_min_check(home: &Path) -> Check {
    let cfg = SessionsConfig::load(home).unwrap_or_default();
    if let Some(info) = cfg.update_info() {
        if info.urgency == crate::telemetry::config::UpdateUrgency::Critical {
            return Check {
                label: "Minimum supported version".into(),
                ok: false,
                detail: info
                    .available_version
                    .map(|v| format!("upgrade required — {v} available"))
                    .unwrap_or_else(|| "upgrade required".into()),
                fix: Some("sessions upgrade".into()),
            };
        }
    }
    Check {
        label: "Minimum supported version".into(),
        ok: true,
        detail: VERSION.to_string(),
        fix: None,
    }
}

pub fn all_ok(checks: &[Check]) -> bool {
    checks.iter().all(|check| check.ok)
}

pub fn print_report(checks: &[Check]) -> bool {
    for check in checks {
        let mark = if check.ok { "ok" } else { "FAIL" };
        println!("  [{mark}] {}{}", check.label, detail_suffix(&check.detail));
        if !check.ok {
            if let Some(fix) = &check.fix {
                println!("        fix: {fix}");
            }
        }
    }
    all_ok(checks)
}

fn detail_suffix(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        format!(" — {detail}")
    }
}

fn binary_installed(binary: &Path) -> Check {
    let ok = binary.is_file();
    Check {
        label: "Binary installed".into(),
        ok,
        detail: if ok {
            binary.display().to_string()
        } else {
            "sessions binary not found".into()
        },
        fix: (!ok).then(|| "re-run ./install.sh from the repo checkout".into()),
    }
}

fn binary_runs(binary: &Path) -> Check {
    let ok = binary.is_file()
        && Command::new(binary)
            .arg("--help")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
    Check {
        label: "Binary executes".into(),
        ok,
        detail: if ok {
            String::new()
        } else {
            "sessions --help failed (check code signature on macOS)".into()
        },
        fix: (!ok).then(|| "re-run ./install.sh (builds, signs, and deploys)".into()),
    }
}

fn codesign_valid(binary: &Path) -> Check {
    let ok = binary.is_file()
        && Command::new("codesign")
            .args(["--verify", "--verbose"])
            .arg(binary)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
    Check {
        label: "Code signature".into(),
        ok,
        detail: if ok {
            String::new()
        } else {
            "codesign verification failed".into()
        },
        fix: (!ok).then(|| "re-run ./install.sh to re-sign the deployed binary".into()),
    }
}

fn path_entry(link: &Path, binary: &Path) -> Check {
    let mut ok = false;
    let mut detail = String::new();
    if link.is_symlink() {
        if let Ok(target) = std::fs::read_link(link) {
            let canonical = binary
                .canonicalize()
                .unwrap_or_else(|_| binary.to_path_buf());
            let link_target = if target.is_absolute() {
                target
            } else {
                link.parent().unwrap_or_else(|| Path::new(".")).join(target)
            };
            let link_canonical = link_target.canonicalize().unwrap_or(link_target);
            ok = link_canonical == canonical;
            detail = format!("{} -> {}", link.display(), link_canonical.display());
        }
    } else if link.is_file() {
        detail = format!("{} is a regular file, expected symlink", link.display());
    } else {
        detail = format!("{} not on PATH", link.display());
    }

    Check {
        label: "PATH entry".into(),
        ok,
        detail,
        fix: (!ok).then(|| {
            "add export PATH=\"${HOME}/.local/bin:${PATH}\" to your shell profile, then re-run ./install.sh".into()
        }),
    }
}

fn grok_legacy_binary_link(home: &Path, binary: &Path) -> Check {
    if !grok_present(home) {
        return Check {
            label: "Grok legacy binary".into(),
            ok: true,
            detail: "n/a".into(),
            fix: None,
        };
    }

    let legacy = crate::paths::grok_legacy_binary_path(home);
    let canonical = binary
        .canonicalize()
        .unwrap_or_else(|_| binary.to_path_buf());
    let mut ok = false;
    let mut detail = String::new();

    if legacy.is_symlink() {
        if let Ok(target) = std::fs::read_link(&legacy) {
            let resolved = if target.is_absolute() {
                target
            } else {
                legacy
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(target)
            };
            let resolved = resolved.canonicalize().unwrap_or(resolved);
            ok = resolved == canonical;
            detail = format!("{} -> {}", legacy.display(), resolved.display());
        }
    } else if legacy.is_file() {
        detail = format!("{} is a regular file, expected symlink", legacy.display());
    } else {
        detail = format!("{} missing", legacy.display());
    }

    Check {
        label: "Grok legacy binary".into(),
        ok,
        detail,
        fix: (!ok).then(|| "re-run ./install.sh".into()),
    }
}

fn data_layout(home: &Path, data_root: &Path, install_dir: &Path) -> Check {
    let state = crate::paths::state_dir(home);
    let logs = crate::paths::logs_dir(home);
    let ok = install_dir.is_dir() && data_root.is_dir() && state.is_dir() && logs.is_dir();
    Check {
        label: "Data directories".into(),
        ok,
        detail: data_root.display().to_string(),
        fix: (!ok).then(|| "re-run ./install.sh".into()),
    }
}

fn tmux_runtime_checks(home: &Path) -> Vec<Check> {
    let config = config_from_home(home);
    if !hooks::command_on_path("tmux") {
        return vec![Check {
            label: "Tmux UI runtime".into(),
            ok: true,
            detail: "tmux not installed".into(),
            fix: None,
        }];
    }
    vec![
        ui_runtime_check(&config),
        tmux_client_attachment_check(&config),
    ]
}

fn ui_runtime_check(config: &Config) -> Check {
    if !crate::daemon::tmux::session_exists(&config.tmux_ui_session) {
        return Check {
            label: "Tmux UI runtime".into(),
            ok: true,
            detail: "sessions-ui not running (run sessions up)".into(),
            fix: None,
        };
    }
    match crate::daemon::tmux::verify_ui_runtime(&config.tmux_ui_session, &config.tmux_session) {
        Ok(()) => Check {
            label: "Tmux UI runtime".into(),
            ok: true,
            detail: "mouse on, bindings installed, sidebar bar running".into(),
            fix: None,
        },
        Err(err) => Check {
            label: "Tmux UI runtime".into(),
            ok: false,
            detail: err.to_string(),
            fix: Some("run make reload, or sessions down && sessions up".into()),
        },
    }
}

fn tmux_client_attachment_check(config: &Config) -> Check {
    let agents = config.tmux_session.as_str();
    let ui = config.tmux_ui_session.as_str();

    // Prefer the shared tmux helper (correctly ignores the nested workspace
    // attach inside sessions-ui:ui.1). Fall back to raw list-clients if the
    // helper cannot talk to tmux.
    let clients = match crate::daemon::tmux::list_tmux_clients() {
        Ok(clients) => clients,
        Err(_) => {
            return Check {
                label: "Tmux client attachment".into(),
                ok: true,
                detail: "no tmux server".into(),
                fix: None,
            };
        }
    };

    let ui_client_count = clients.iter().filter(|c| c.session == ui).count();
    let nested_tty = crate::daemon::tmux::ui_workspace_pane_tty(ui);
    let strays =
        crate::daemon::tmux::filter_stray_agents_clients(&clients, agents, nested_tty.as_deref());

    if strays.is_empty() {
        let detail = if nested_tty.is_some() {
            format!("nested workspace attach only (sidebar uses {ui})")
        } else {
            format!("no bare {agents} clients (sidebar uses {ui})")
        };
        return Check {
            label: "Tmux client attachment".into(),
            ok: true,
            detail,
            fix: None,
        };
    }

    let count = strays.len();
    let tty_list = strays
        .iter()
        .map(|c| c.tty.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let detail = if ui_client_count == 0 {
        format!("{count} bare client(s) on {agents} with none on {ui}: {tty_list}")
    } else {
        format!("{count} bare client(s) on {agents} (excluding nested workspace): {tty_list}")
    };

    let misconfig = if ui_client_count == 0 {
        format!(" Nobody is attached to {ui} — you are on bare {agents} instead of the sidebar UI.")
    } else {
        String::new()
    };
    let first_tty = strays.first().map(|c| c.tty.as_str()).unwrap_or("<tty>");

    // Bare host clients on agents fight sessions-ui for focus / active-window.
    // The nested attach from sessions-ui:ui.1 is intentional and is not a stray.
    let fix = format!(
        "Bare clients on '{agents}' (not the nested sidebar workspace) fight the UI for \
         tmux focus/active-window and can make the sidebar feel laggy or unresponsive.{misconfig}\n        \
         auto-fix:  sessions doctor --repair   (or: sessions up)\n        \
         list:       tmux list-clients -t {agents}\n        \
         detach one: tmux detach-client -t {first_tty}\n        \
         attach UI:  sessions up   or   sessions tmux ui attach   (session: {ui})"
    );

    Check {
        label: "Tmux client attachment".into(),
        ok: false,
        detail,
        fix: Some(fix),
    }
}

fn ghostty_config_path(home: &Path) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/com.mitchellh.ghostty/config")
    } else {
        home.join(".config/ghostty/config")
    }
}

fn ghostty_keybind_check(home: &Path) -> Check {
    let ghostty = crate::hooks::command_on_path("ghostty");
    let config = ghostty_config_path(home);
    if !ghostty && !config.exists() {
        return Check {
            label: "Ghostty keybinds".into(),
            ok: true,
            detail: "n/a".into(),
            fix: None,
        };
    }

    let body = config
        .exists()
        .then(|| std::fs::read_to_string(&config).ok())
        .flatten()
        .unwrap_or_default();
    let ok = body.contains("# >>> sessions-cli ghostty keybinds >>>")
        && body.contains("keybind = super+n=text:\\x1bn")
        && body.contains("keybind = super+f=text:\\x1bf")
        && body.contains("keybind = super+b=text:\\x1bb")
        && body.contains("keybind = super+1=text:\\x1b1")
        && body.contains("keybind = super+0=text:\\x1b0");
    Check {
        label: "Ghostty keybinds".into(),
        ok,
        detail: if ok {
            "⌘+N/⌘+F/⌘+B/⌘1–0 routed to sessions".into()
        } else if config.exists() {
            format!("missing sessions block in {}", config.display())
        } else {
            "ghostty config not found".into()
        },
        fix: (!ok).then(|| "re-run ./install.sh (merges Ghostty ⌘+N/⌘+F/⌘+B/⌘1–0 keybinds)".into()),
    }
}

fn cursor_user_dir(home: &Path) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Cursor/User")
    } else {
        home.join(".config/Cursor/User")
    }
}

fn cursor_keybindings_path(home: &Path) -> std::path::PathBuf {
    cursor_user_dir(home).join("keybindings.json")
}

fn cursor_tasks_path(home: &Path) -> std::path::PathBuf {
    cursor_user_dir(home).join("tasks.json")
}

fn cursor_keybind_check(home: &Path) -> Check {
    let cursor_bin = crate::hooks::command_on_path("cursor");
    let config = cursor_keybindings_path(home);
    let tasks = cursor_tasks_path(home);
    let cursor_dir = cursor_user_dir(home).exists();
    if !cursor_bin && !cursor_dir && !config.exists() {
        return Check {
            label: "Cursor keybinds".into(),
            ok: true,
            detail: "n/a".into(),
            fix: None,
        };
    }

    let body = config
        .exists()
        .then(|| std::fs::read_to_string(&config).ok())
        .flatten()
        .unwrap_or_default();
    let tasks_body = tasks
        .exists()
        .then(|| std::fs::read_to_string(&tasks).ok())
        .flatten()
        .unwrap_or_default();
    let ok = body.contains("// >>> sessions-cli cursor keybinds >>>")
        && body.contains("workbench.action.tasks.runTask")
        && body.contains("sessions: focus 1")
        && body.contains("terminalFocusInAny")
        && body.contains("-workbench.action.focusFirstEditorGroup")
        && (body.contains("\"cmd+1\"") || body.contains("\"ctrl+1\""))
        && tasks_body.contains("// >>> sessions-cli cursor tasks >>>")
        && tasks_body.contains("sessions: focus 1")
        && tasks_body.contains("\"focus\"");
    Check {
        label: "Cursor keybinds".into(),
        ok,
        detail: if ok {
            "⌘1–0 → sessions focus (terminal pane / agent terminal)".into()
        } else if config.exists() {
            format!("missing sessions block in {}", config.display())
        } else {
            "cursor keybindings.json not found".into()
        },
        fix: (!ok).then(|| {
            "re-run ./install.sh or bin/setup-cursor.sh (merges Cursor ⌘1–0 keybinds + tasks)"
                .into()
        }),
    }
}

fn vscode_user_dir(home: &Path) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Code/User")
    } else {
        home.join(".config/Code/User")
    }
}

/// IDE integrated terminal settings for sessions (Cursor + VS Code).
///
/// - Default `terminal.integrated.minimumContrastRatio` is **4.5**, which rewrites
///   cell colors and desaturates sessions truecolor greys/accents. Setup sets `1`.
/// - Default right-click shows the IDE context menu and steals events from the
///   sessions sidebar. Setup sets `rightClickBehavior` to `nothing`.
/// - Hybrid right-click JS (`sessions-terminal.js` in `vscode_custom_css.imports`)
///   restores the IDE menu over the workspace; optional but reported when missing.
fn ide_terminal_color_check(home: &Path) -> Check {
    let mut details = Vec::new();
    let mut missing = Vec::new();
    let mut any_present = false;

    for (label, settings) in [
        ("Cursor", cursor_user_dir(home).join("settings.json")),
        ("VS Code", vscode_user_dir(home).join("settings.json")),
    ] {
        if !settings.exists() && !settings.parent().is_some_and(|p| p.exists()) {
            continue;
        }
        any_present = true;
        let body = std::fs::read_to_string(&settings).unwrap_or_default();
        // Accept bare 1 or 1.0 (json number forms).
        let contrast_ok = body.contains("\"terminal.integrated.minimumContrastRatio\"")
            && (body.contains("minimumContrastRatio\": 1")
                || body.contains("minimumContrastRatio\":1")
                || body.contains("minimumContrastRatio\": 1.0")
                || body.contains("minimumContrastRatio\":1.0"));
        let right_click_ok = body.contains("\"terminal.integrated.rightClickBehavior\"")
            && (body.contains("rightClickBehavior\": \"nothing\"")
                || body.contains("rightClickBehavior\":\"nothing\""));
        let hybrid_js = body.contains("sessions-terminal.js");
        let mut parts = Vec::new();
        if contrast_ok {
            parts.push("contrast=1");
        } else {
            parts.push("contrast not disabled");
        }
        if right_click_ok {
            parts.push("rightClick=nothing");
        } else {
            parts.push("rightClick not pass-through");
        }
        if hybrid_js {
            parts.push("hybridRightClick=js");
        } else {
            parts.push("hybridRightClick missing (Shift+right-click still works)");
        }
        details.push(format!("{label}: {}", parts.join(", ")));
        // Hybrid JS is advisory — pass-through + contrast are required for sessions UX.
        if !(contrast_ok && right_click_ok) {
            missing.push(label);
        }
    }

    if !any_present {
        return Check {
            label: "IDE terminal color".into(),
            ok: true,
            detail: "n/a (no Cursor/VS Code user dir)".into(),
            fix: None,
        };
    }

    let ok = missing.is_empty();
    Check {
        label: "IDE terminal color".into(),
        ok,
        detail: details.join("; "),
        fix: (!ok).then(|| {
            "re-run bin/setup-cursor.sh and/or bin/setup-vscode.sh \
(sets minimumContrastRatio=1, rightClickBehavior=nothing, and hybrid right-click JS)"
                .into()
        }),
    }
}

fn agent_hook_checks(home: &Path) -> Vec<Check> {
    let detected = hooks::detect_agents(home);
    if detected.is_empty() {
        return vec![Check {
            label: "Agent hooks".into(),
            ok: true,
            detail: "no agents detected".into(),
            fix: None,
        }];
    }

    detected
        .into_iter()
        .map(|report| {
            let ok = !report.needs_setup;
            Check {
                label: format!("{} hooks", report.id),
                ok,
                detail: report.detail,
                fix: (!ok).then(|| {
                    format!(
                        "sessions hooks setup {}  (or Settings → Integrations → Configure all hooks)",
                        report.id
                    )
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::managed::{save_managed_record, ManagedLaunchRecord};
    use crate::session::manifest::{append_entry, ManifestEntry, ManifestSource};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn stale_managed_files_detects_orphan_record() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = crate::config::Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_open".into(),
                source: ManifestSource::Cli,
                workspace_index: None,
                cwd: "/tmp".into(),
                cwd_label: "/tmp".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: None,
                title: None,
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        save_managed_record(
            home,
            &ManagedLaunchRecord {
                sessions_session_id: "ssn_open".into(),
                launch_id: "lch_open".into(),
                agent: "grok".into(),
                tmux_session: "agents-nonexistent".into(),
                window_index: 1,
                pane_id: Some("%1".into()),
                initial_cwd: "/tmp".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                agent_session_id: None,
                pool: false,
            },
        )
        .unwrap();
        save_managed_record(
            home,
            &ManagedLaunchRecord {
                sessions_session_id: "ssn_stale".into(),
                launch_id: "lch_stale".into(),
                agent: "grok".into(),
                tmux_session: "agents-nonexistent".into(),
                window_index: 99,
                pane_id: Some("%99".into()),
                initial_cwd: "/tmp".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                agent_session_id: None,
                pool: false,
            },
        )
        .unwrap();

        let check = stale_managed_files_check(home);
        assert!(
            !check.ok,
            "expected stale managed file to fail: {}",
            check.detail
        );
        assert!(check.detail.contains("1 stale"));
        assert!(check.detail.contains("2 managed"));
    }

    #[test]
    fn manifest_checks_flag_corrupted_launch_command() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = crate::config::Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        let mut entry = ManifestEntry {
            sessions_session_id: "ssn_corrupt".into(),
            source: ManifestSource::Cli,
            workspace_index: None,
            cwd: "/tmp".into(),
            cwd_label: "/tmp".into(),
            agent: "grok".into(),
            launch_command: "grok · sticky title".into(),
            agent_session_id: None,
            title: None,
            messaged_at: None,
            closed: false,
        };
        append_entry(&config, entry.clone()).unwrap();

        let checks = manifest_checks(home);
        let check = checks
            .iter()
            .find(|check| check.label == "Manifest launch commands")
            .expect("launch command check");
        assert!(!check.ok, "expected corrupted launch_command to fail");
        assert!(check.detail.contains("ssn_corrupt"));
        assert!(check.fix.as_deref().unwrap().contains("--repair"));
        assert!(check.fix.as_deref().unwrap().contains("quick-launch"));

        entry.launch_command = "grok".into();
        crate::session::save_manifest(
            &config,
            &crate::session::SessionManifest {
                version: 1,
                last_active_sessions_session_id: None,
                migrated_from: None,
                entries: vec![entry],
            },
        )
        .unwrap();
        let checks = manifest_checks(home);
        let check = checks
            .iter()
            .find(|check| check.label == "Manifest launch commands")
            .expect("launch command check");
        assert!(check.ok);
    }

    #[test]
    fn manifest_checks_flag_agent_session_id_drift() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = crate::config::Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();
        fs::create_dir_all(crate::paths::state_dir(home)).unwrap();

        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_drift".into(),
                source: ManifestSource::Cli,
                workspace_index: None,
                cwd: "/tmp".into(),
                cwd_label: "/tmp".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: None,
                title: None,
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let session = crate::model::Session {
            id: "tmux:win:1".into(),
            kitty_window_id: 1,
            kitty_tab_id: 1,
            kitty_os_window_id: 1,
            tab_index: 1,
            tmux_session: "agents-nonexistent".into(),
            tmux_pane_id: "%1".into(),
            pane_pid: 0,
            agent_session_id: Some("agent-drift".into()),
            title: "grok · drift".into(),
            description: "grok".into(),
            cwd: "/tmp".into(),
            cwd_label: "/tmp".into(),
            project: "grok".into(),
            state: crate::model::AgentState::Idle,
            completed_thread: None,
            completed_at: None,
            messaged_at: None,
            prompt_submitted: false,
            title_manual: false,
            is_active: true,
            last_event_at: chrono::Utc::now(),
            managed: true,
            sessions_session_id: Some("ssn_drift".into()),
            managed_agent: Some("grok".into()),
        };
        crate::daemon::persist::save_state(&config, &[session], 1).unwrap();

        let checks = manifest_checks(home);
        let check = checks
            .iter()
            .find(|check| check.label == "Manifest agent_session_id")
            .expect("agent_session_id check");
        assert!(!check.ok);
        assert!(check.detail.contains("ssn_drift"));
        assert!(check.fix.as_deref().unwrap().contains("--repair"));
        assert!(check.fix.as_deref().unwrap().contains("backfill"));
    }

    #[test]
    fn install_checks_include_core_labels() {
        let checks = install_checks(Path::new("/home/testuser"));
        let labels: Vec<_> = checks.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"Binary installed"));
        assert!(labels.contains(&"Grok legacy binary"));
        assert!(labels.contains(&"IDE terminal color"));
        assert!(labels.contains(&"skillshare"));
        assert!(labels.contains(&"Obot"));
        assert!(
            labels.iter().any(|label| label.ends_with(" hooks")),
            "expected agent hook checks, got {labels:?}"
        );
    }

    #[test]
    fn readiness_checks_include_companions() {
        let checks = readiness_checks(Path::new("/home/testuser"));
        let labels: Vec<_> = checks.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"sessionsd"));
        assert!(labels.contains(&"skillshare"));
        assert!(labels.contains(&"Obot"));
        assert!(labels.contains(&"Docker"));
    }

    #[test]
    fn ide_terminal_color_ok_when_contrast_disabled() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let cursor = cursor_user_dir(home);
        fs::create_dir_all(&cursor).unwrap();
        fs::write(
            cursor.join("settings.json"),
            r#"{
                "terminal.integrated.minimumContrastRatio": 1,
                "terminal.integrated.rightClickBehavior": "nothing"
            }"#,
        )
        .unwrap();
        let check = ide_terminal_color_check(home);
        assert!(check.ok, "{check:?}");
        assert!(check.detail.contains("contrast=1"));
        assert!(check.detail.contains("rightClick=nothing"));
    }

    #[test]
    fn ide_terminal_color_fails_when_right_click_not_pass_through() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let cursor = cursor_user_dir(home);
        fs::create_dir_all(&cursor).unwrap();
        fs::write(
            cursor.join("settings.json"),
            r#"{ "terminal.integrated.minimumContrastRatio": 1 }"#,
        )
        .unwrap();
        let check = ide_terminal_color_check(home);
        assert!(!check.ok, "{check:?}");
        assert!(check.detail.contains("rightClick not pass-through"));
    }

    #[test]
    fn manifest_checks_flag_agent_mismatch_from_title_prefix() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = crate::config::Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_mismatch".into(),
                source: ManifestSource::Cli,
                workspace_index: None,
                cwd: "/tmp".into(),
                cwd_label: "/tmp".into(),
                agent: "console".into(),
                launch_command: String::new(),
                agent_session_id: Some("019ef8fd-0000-7000-8000-000000000099".into()),
                title: Some("grok · resume after down-sync".into()),
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let checks = manifest_checks(home);
        let check = checks
            .iter()
            .find(|check| check.label == "Manifest agent mismatch")
            .expect("manifest agent mismatch check");
        assert!(!check.ok, "expected mismatch: {}", check.detail);
        assert!(check.detail.contains("ssn_mismatch"));
        assert!(check.fix.as_deref().unwrap().contains("--repair"));
    }

    #[test]
    fn manifest_checks_flag_agent_mismatch_from_disk_detect() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let cwd = env!("CARGO_MANIFEST_DIR");
        let agent_session_id = "019ef8fd-1111-7000-8000-000000000088";
        let mut config = crate::config::Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        let session_dir = crate::agents::grok::session_dir(home, cwd, agent_session_id);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("summary.json"),
            r#"{"generated_title":"test"}"#,
        )
        .unwrap();

        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_disk_mismatch".into(),
                source: ManifestSource::Cli,
                workspace_index: None,
                cwd: cwd.into(),
                cwd_label: cwd.into(),
                agent: "session".into(),
                launch_command: String::new(),
                agent_session_id: Some(agent_session_id.into()),
                title: None,
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let checks = manifest_checks(home);
        let check = checks
            .iter()
            .find(|check| check.label == "Manifest agent mismatch")
            .expect("manifest agent mismatch check");
        assert!(
            !check.ok,
            "expected disk-detected mismatch: {}",
            check.detail
        );
        assert!(check.detail.contains("ssn_disk_mismatch"));
    }

    #[test]
    fn manifest_checks_warn_restore_disk_stale_without_failing() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = crate::config::Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_stale_disk".into(),
                source: ManifestSource::Cli,
                workspace_index: None,
                cwd: "/tmp".into(),
                cwd_label: "/tmp".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: Some("019ef8fd-2222-7000-8000-000000000077".into()),
                title: Some("grok · stale thread".into()),
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let checks = manifest_checks(home);
        let check = checks
            .iter()
            .find(|check| check.label == "Restore disk stale")
            .expect("restore disk stale check");
        assert!(check.ok, "warn-only check should stay ok: {}", check.detail);
        assert!(check.detail.contains("warn"));
        assert!(check.detail.contains("ssn_stale_disk"));
        assert!(check.detail.contains("quick-launch"));
    }

    #[test]
    fn repair_manifest_agents_rewrites_console_row_from_title_prefix() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = crate::config::Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_repair_agent".into(),
                source: ManifestSource::Cli,
                workspace_index: None,
                cwd: "/tmp".into(),
                cwd_label: "/tmp".into(),
                agent: "console".into(),
                launch_command: String::new(),
                agent_session_id: Some("019ef8fd-3333-7000-8000-000000000066".into()),
                title: Some("codex · repair agent field".into()),
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let rewritten = repair_manifest_agents(&config).unwrap();
        assert_eq!(rewritten, vec!["ssn_repair_agent".to_string()]);

        let manifest = load_manifest(&config).unwrap();
        let entry = manifest
            .entries
            .iter()
            .find(|entry| entry.sessions_session_id == "ssn_repair_agent")
            .expect("repaired entry");
        assert_eq!(entry.agent, "codex");

        let checks = manifest_checks(home);
        let check = checks
            .iter()
            .find(|check| check.label == "Manifest agent mismatch")
            .expect("manifest agent mismatch check");
        assert!(check.ok, "repair should clear mismatch: {}", check.detail);
    }

    #[test]
    fn manifest_checks_pass_agent_mismatch_when_agent_is_concrete() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let mut config = crate::config::Config::default();
        config.home = home.to_path_buf();
        config.state_path = crate::paths::state_dir(home).join("sessionsd.json");
        config.tmux_session = "agents-nonexistent".into();

        append_entry(
            &config,
            ManifestEntry {
                sessions_session_id: "ssn_ok".into(),
                source: ManifestSource::Cli,
                workspace_index: None,
                cwd: "/tmp".into(),
                cwd_label: "/tmp".into(),
                agent: "grok".into(),
                launch_command: "grok".into(),
                agent_session_id: Some("019ef8fd-4444-7000-8000-000000000055".into()),
                title: Some("grok · healthy".into()),
                messaged_at: None,
                closed: false,
            },
        )
        .unwrap();

        let checks = manifest_checks(home);
        let check = checks
            .iter()
            .find(|check| check.label == "Manifest agent mismatch")
            .expect("manifest agent mismatch check");
        assert!(check.ok, "concrete agent should pass: {}", check.detail);
    }
}
