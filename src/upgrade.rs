use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use crate::config::Config;
use crate::telemetry::{self, FeatureId};
use crate::telemetry::config::SessionsConfig;
use crate::version::VERSION;

const DEFAULT_INSTALL_URL: &str =
    "https://raw.githubusercontent.com/sessions-cli/sessions-cli/main/install.sh";

pub fn run_upgrade(check_only: bool, channel: Option<&str>) -> Result<()> {
    telemetry::record_feature(
        if check_only {
            FeatureId::CliUpgradeCheck
        } else {
            FeatureId::CliUpgrade
        },
        telemetry::feature::Source::Cli,
    );

    if check_only {
        telemetry::heartbeat::maybe_heartbeat(true)?;
        print_update_status()?;
        return Ok(());
    }

    let home = crate::paths::home();
    let mut cfg = SessionsConfig::load(&home)?;
    if let Some(ch) = channel {
        cfg.telemetry.channel = ch.to_string();
        cfg.save(&home)?;
    }

    telemetry::heartbeat::maybe_heartbeat(true)?;
    let cfg = SessionsConfig::load(&home)?;
    let update = cfg.update_info();
    if update.is_none() {
        println!("sessions {VERSION} is up to date");
        return Ok(());
    }

    let method = cfg.telemetry.install_method.as_str();
    match method {
        "git" | "local" => upgrade_via_git(&cfg)?,
        _ => upgrade_via_curl(&cfg)?,
    }

    let config = Config::default();
    let binary = crate::paths::resolve_binary(&home);
    Command::new(&binary)
        .arg("hooks")
        .arg("setup")
        .status()
        .context("hooks setup after upgrade")?;

    restart_daemon_and_sidebar(&config)?;

    telemetry::record_lifecycle(FeatureId::LifecycleUpgradeCompleted);
    println!("Upgraded to sessions {}", crate::version::VERSION);
    Ok(())
}

fn print_update_status() -> Result<()> {
    let cfg = SessionsConfig::load(&crate::paths::home())?;
    if let Some(info) = cfg.update_info() {
        if let Some(version) = info.available_version {
            println!(
                "update available: {version} ({})",
                info.urgency.as_str()
            );
            if !info.message.is_empty() {
                println!("  {}", info.message);
            }
            if !info.changelog_url.is_empty() {
                println!("  {}", info.changelog_url);
            }
        }
    } else {
        println!("sessions {VERSION} is up to date");
    }
    Ok(())
}

fn upgrade_via_git(cfg: &SessionsConfig) -> Result<()> {
    let checkout = cfg.install.checkout_path.trim();
    if checkout.is_empty() {
        anyhow::bail!(
            "git install has no checkout_path in config — set [install] checkout_path or reinstall from git"
        );
    }
    let root = Path::new(checkout);
    if !root.join("install.sh").is_file() {
        anyhow::bail!("checkout_path {} has no install.sh", root.display());
    }
    let status = Command::new("git")
        .args(["-C", checkout, "pull", "--ff-only"])
        .status()
        .context("git pull")?;
    if !status.success() {
        anyhow::bail!("git pull failed");
    }
    let status = Command::new(root.join("install.sh"))
        .current_dir(root)
        .status()
        .context("install.sh after git pull")?;
    if !status.success() {
        anyhow::bail!("install.sh failed");
    }
    Ok(())
}

fn upgrade_via_curl(cfg: &SessionsConfig) -> Result<()> {
    let url = if cfg.update.install_url.is_empty() {
        DEFAULT_INSTALL_URL.to_string()
    } else {
        cfg.update.install_url.clone()
    };
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "curl -fsSL '{url}' | sh -s -- --skip-deps"
        ))
        .status()
        .context("curl install upgrade")?;
    if !status.success() {
        anyhow::bail!("upgrade install script failed");
    }
    Ok(())
}

fn restart_daemon_and_sidebar(config: &Config) -> Result<()> {
    let home = &config.home;
    let binary = crate::paths::resolve_binary(home);
    let lock_path = config.socket_path.with_extension("pid");
    if let Ok(pid_str) = std::fs::read_to_string(&lock_path) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            if pid > 0 {
                unsafe { libc::kill(pid, libc::SIGTERM) };
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    let _ = std::fs::remove_file(&config.socket_path);
    Command::new(&binary)
        .args(["daemon", "--foreground"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("restart daemon")?;
    for _ in 0..30 {
        if crate::daemon::server::socket_responds(&config.socket_path) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = Command::new("tmux")
        .args([
            "respawn-pane",
            "-k",
            "-t",
            &format!("{}:ui.0", config.tmux_ui_session),
            &binary.to_string_lossy(),
            "bar",
        ])
        .status();
    Ok(())
}