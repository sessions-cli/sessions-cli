use crate::config::Config;
use crate::daemon::events::EventSender;
use crate::daemon::manifest_sync::ManifestSyncQueue;
use crate::daemon::persist::{load_state, load_state_or_empty, save_state};
use crate::daemon::spool::drain_config_spool;
use crate::daemon::state::DaemonState;
use crate::model::{ClientCommand, NotifyMessage, ServerEvent};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

const PERSIST_DEBOUNCE_MS: u64 = 500;
const PERSIST_POLL_MS: u64 = 50;
const MANIFEST_PERSIST_DEBOUNCE_MS: u64 = 500;
const MANIFEST_PERSIST_POLL_MS: u64 = 50;
const SUBSCRIBE_READY_WAIT_MS: u64 = 15_000;
const AUTO_RECONCILE_DELAY_MS: u64 = 200;

pub async fn run_daemon(config: Config, foreground: bool) -> Result<()> {
    setup_logging(&config, foreground)?;

    if socket_responds(&config.socket_path) {
        info!(
            "sessionsd already running on {}",
            config.socket_path.display()
        );
        return Ok(());
    }

    // PID lockfile guard: if another daemon process is alive but its socket
    // file was deleted (socket-stomp race), kill the orphan so only one
    // daemon runs at a time.
    let lock_path = config.socket_path.with_extension("pid");
    if let Ok(pid_str) = std::fs::read_to_string(&lock_path) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            if pid > 0 {
                let orphan = unsafe { libc::kill(pid, 0) == 0 };
                if orphan {
                    warn!("orphan daemon pid {pid} detected (socket stomp) — sending SIGTERM");
                    let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
                    // Give it a moment to release resources
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
            }
        }
    }
    // Remove stale lockfile before writing our own
    let _ = std::fs::remove_file(&lock_path);
    std::fs::write(&lock_path, format!("{}\n", std::process::id()))?;

    if let Some(parent) = config.socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if config.socket_path.exists() {
        std::fs::remove_file(&config.socket_path)?;
    }

    let persisted = match load_state(&config) {
        Ok(state) => state,
        Err(err) => {
            warn!("state restore failed: {err}");
            load_state_or_empty(&config)
        }
    };
    let manifest_sync = Arc::new(ManifestSyncQueue::new());
    let state = Arc::new(DaemonState::new(
        config.clone(),
        persisted.sessions,
        manifest_sync.clone(),
    ));
    let (event_tx, _) = crate::daemon::events::new_event_channel(1024);
    let event_tx = Arc::new(event_tx);

    let listener = UnixListener::bind(&config.socket_path)
        .with_context(|| format!("bind {}", config.socket_path.display()))?;
    info!("sessionsd listening on {}", config.socket_path.display());

    drain_spool_into_state(&state, &config).await?;

    let state_poll = state.clone();
    let config_poll = config.clone();
    let event_tx_poll = event_tx.clone();
    tokio::spawn(async move {
        poll_loop(state_poll, config_poll, event_tx_poll).await;
    });

    let state_reconcile = state.clone();
    let config_reconcile = config.clone();
    let event_tx_reconcile = event_tx.clone();
    tokio::spawn(async move {
        try_auto_reconcile(state_reconcile, config_reconcile, event_tx_reconcile).await;
    });

    let state_persist = state.clone();
    let config_persist = config.clone();
    tokio::spawn(async move {
        persist_loop(state_persist, config_persist).await;
    });

    let manifest_sync_loop = manifest_sync.clone();
    let config_manifest = config.clone();
    tokio::spawn(async move {
        manifest_persist_loop(manifest_sync_loop, config_manifest).await;
    });

    let state_spool = state.clone();
    let config_spool = config.clone();
    let event_tx_spool = event_tx.clone();
    tokio::spawn(async move {
        spool_loop(state_spool, config_spool, event_tx_spool).await;
    });

    crate::daemon::telemetry_worker::spawn_heartbeat_loop(state.clone(), config.clone());

    let mut shutdown =
        signal(SignalKind::terminate()).context("register SIGTERM handler for sessionsd")?;

    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                info!("SIGTERM received — flushing sessionsd.json and session manifest");
                flush_persist_if_dirty(&state, &config).await;
                if let Err(err) = manifest_sync.flush_all(&config) {
                    error!("manifest flush failed: {err}");
                }
                break;
            }
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        let state = state.clone();
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, state, event_tx).await {
                                warn!("client error: {e}");
                            }
                        });
                    }
                    Err(e) => error!("accept error: {e}"),
                }
            }
        }
    }
    Ok(())
}

fn setup_logging(config: &Config, foreground: bool) -> Result<()> {
    if let Some(parent) = config.log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    if foreground {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&config.log_path)?;
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(file)
            .init();
    }
    Ok(())
}

async fn drain_spool_into_state(state: &Arc<DaemonState>, config: &Config) -> Result<()> {
    for spooled in drain_config_spool(config)? {
        if state.handle_notify(&spooled.msg).await.is_some() {
            info!("drained spool event: {}", spooled.msg.event);
        }
        // Ack only after the event has been applied to state, so a crash
        // mid-apply leaves the file for the next drain (at-least-once).
        spooled.ack();
    }
    Ok(())
}

async fn finish_reconcile(state: &DaemonState, event_tx: &EventSender) -> ServerEvent {
    state.restore_complete().await;
    match state.refresh_from_tmux().await {
        Some(event) => {
            let _ = event_tx.send(event.clone());
            event
        }
        None => {
            let event = state.snapshot_event().await;
            let _ = event_tx.send(event.clone());
            event
        }
    }
}

async fn try_auto_reconcile(
    state: Arc<DaemonState>,
    config: Config,
    event_tx: Arc<EventSender>,
) {
    tokio::time::sleep(std::time::Duration::from_millis(AUTO_RECONCILE_DELAY_MS)).await;
    if !state.is_booting().await {
        return;
    }
    if !crate::daemon::tmux::session_exists(&config.tmux_session) {
        return;
    }
    let manifest = match crate::session::manifest::load_manifest(&config) {
        Ok(manifest) => manifest,
        Err(_) => return,
    };
    let live_set: HashSet<String> = crate::daemon::tmux::list_live_sessions_session_ids(
        &config.tmux_session,
    )
    .unwrap_or_default()
    .into_keys()
    .collect();
    if crate::session::needs_cold_boot_restore(&live_set, &manifest) {
        return;
    }
    let _ = finish_reconcile(state.as_ref(), event_tx.as_ref()).await;
}

async fn wait_until_reconciled(state: &DaemonState) {
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(SUBSCRIBE_READY_WAIT_MS);
    while state.is_booting().await && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    if state.is_booting().await {
        warn!("sidebar subscribe: reconcile wait timed out — sending best-effort snapshot");
    }
}

async fn poll_loop(state: Arc<DaemonState>, config: Config, event_tx: Arc<EventSender>) {
    let interval = std::time::Duration::from_millis(config.poll_interval_ms);
    loop {
        if !crate::daemon::tmux::session_exists(&config.tmux_session) {
            state.set_booting(true).await;
        }
        if let Some(event) = state.refresh_from_tmux().await {
            // During cold-boot restore, windows come up one-by-one; suppress
            // partial snapshots so the sidebar sees the full list at once.
            if !state.is_booting().await {
                let _ = event_tx.send(event);
            }
        }
        tokio::time::sleep(interval).await;
    }
}

async fn persist_loop(state: Arc<DaemonState>, config: Config) {
    let debounce = std::time::Duration::from_millis(PERSIST_DEBOUNCE_MS);
    let poll = std::time::Duration::from_millis(PERSIST_POLL_MS);
    loop {
        while !state.is_dirty().await {
            tokio::time::sleep(poll).await;
        }

        let mut stable_version = state.version().await;
        loop {
            tokio::time::sleep(debounce).await;
            if !state.is_dirty().await {
                break;
            }
            let current_version = state.version().await;
            if current_version == stable_version {
                flush_persist_if_dirty(&state, &config).await;
                break;
            }
            stable_version = current_version;
        }
    }
}

async fn manifest_persist_loop(manifest_sync: Arc<ManifestSyncQueue>, config: Config) {
    let debounce = std::time::Duration::from_millis(MANIFEST_PERSIST_DEBOUNCE_MS);
    let poll = std::time::Duration::from_millis(MANIFEST_PERSIST_POLL_MS);
    loop {
        while !manifest_sync.is_dirty() {
            tokio::time::sleep(poll).await;
        }

        let mut stable_generation = manifest_sync.generation();
        loop {
            tokio::time::sleep(debounce).await;
            if !manifest_sync.is_dirty() {
                break;
            }
            let current_generation = manifest_sync.generation();
            if crate::daemon::manifest_sync::flush_after_debounce(
                stable_generation,
                current_generation,
                true,
            ) {
                if let Err(err) = manifest_sync.flush_all(&config) {
                    error!("manifest persist failed: {err}");
                }
                break;
            }
            stable_generation = current_generation;
        }
    }
}

async fn flush_persist_if_dirty(state: &Arc<DaemonState>, config: &Config) {
    if !state.is_dirty().await {
        return;
    }
    let sessions = state.session_list().await;
    let version = state.version().await;
    match save_state(config, &sessions, version) {
        Ok(()) => {
            state.clear_dirty().await;
        }
        Err(err) => error!("persist failed: {err}"),
    }
}

async fn spool_loop(state: Arc<DaemonState>, config: Config, event_tx: Arc<EventSender>) {
    let interval = std::time::Duration::from_secs(2);
    loop {
        tokio::time::sleep(interval).await;
        if let Ok(messages) = drain_config_spool(&config) {
            for spooled in messages {
                if let Some(patch) = state.handle_notify(&spooled.msg).await {
                    let _ = event_tx.send(patch);
                }
                // Ack only after applying, so a crash mid-apply retries.
                spooled.ack();
            }
        }
    }
}

async fn handle_client(
    stream: UnixStream,
    state: Arc<DaemonState>,
    event_tx: Arc<EventSender>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    if let Some(first) = lines.next_line().await? {
        if let Ok(notify) = serde_json::from_str::<NotifyMessage>(first.trim()) {
            if notify.is_sessions_notify() {
                if let Some(patch) = state.handle_notify(&notify).await {
                    let _ = event_tx.send(patch);
                }
                return Ok(());
            }
        }

        if let Ok(cmd) = serde_json::from_str::<ClientCommand>(first.trim()) {
            if matches!(cmd, ClientCommand::Subscribe) {
                return handle_subscriber(lines, writer, state, event_tx).await;
            }
            handle_command(cmd, &mut writer, &state, &event_tx).await?;
            return Ok(());
        }
    }

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(notify) = serde_json::from_str::<NotifyMessage>(line) {
            if notify.is_sessions_notify() {
                if let Some(patch) = state.handle_notify(&notify).await {
                    let _ = event_tx.send(patch);
                }
                continue;
            }
        }
        let cmd: ClientCommand = serde_json::from_str(line)?;
        if matches!(cmd, ClientCommand::Subscribe) {
            return handle_subscriber(lines, writer, state, event_tx).await;
        }
        handle_command(cmd, &mut writer, &state, &event_tx).await?;
        return Ok(());
    }
    Ok(())
}

async fn handle_subscriber(
    mut lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    mut writer: tokio::net::unix::OwnedWriteHalf,
    state: Arc<DaemonState>,
    event_tx: Arc<EventSender>,
) -> Result<()> {
    wait_until_reconciled(state.as_ref()).await;
    let snapshot = match state.refresh_from_tmux().await {
        Some(event) => event,
        None => state.snapshot_event().await,
    };
    writer
        .write_all((serde_json::to_string(&snapshot)? + "\n").as_bytes())
        .await?;
    let mut rx = event_tx.subscribe();

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line? {
                    Some(line) => {
                        let line = line.trim();
                        if line.is_empty() { continue; }
                        if let Ok(cmd) = serde_json::from_str::<ClientCommand>(line) {
                            match cmd {
                                ClientCommand::Focus {
                                    window_index,
                                    tab_index,
                                } => {
                                    if let Err(e) = state.focus_exec(window_index, tab_index).await {
                                        warn!("focus failed: {e}");
                                    }
                                    let snapshot = state.snapshot_event().await;
                                    writer
                                        .write_all(
                                            (serde_json::to_string(&snapshot)? + "\n").as_bytes(),
                                        )
                                        .await?;
                                }
                                ClientCommand::Refresh => {
                                    let event = match state.refresh_from_tmux().await {
                                        Some(event) => event,
                                        None => state.snapshot_event().await,
                                    };
                                    let _ = event_tx.send(event.clone());
                                    writer
                                        .write_all(
                                            (serde_json::to_string(&event)? + "\n").as_bytes(),
                                        )
                                        .await?;
                                }
                                ClientCommand::Rename { session_id, title } => {
                                    match state.rename_session(&session_id, title).await {
                                        Ok(event) => {
                                            let _ = event_tx.send(event.clone());
                                            writer
                                                .write_all(
                                                    (serde_json::to_string(&event)? + "\n")
                                                        .as_bytes(),
                                                )
                                                .await?;
                                        }
                                        Err(e) => warn!("rename failed: {e}"),
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    None => break,
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(ev) => {
                        writer.write_all((serde_json::to_string(&ev)? + "\n").as_bytes()).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let snapshot = state.snapshot_event().await;
                        writer
                            .write_all((serde_json::to_string(&snapshot)? + "\n").as_bytes())
                            .await?;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    Ok(())
}

async fn handle_command(
    cmd: ClientCommand,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    state: &Arc<DaemonState>,
    event_tx: &Arc<EventSender>,
) -> Result<()> {
    match cmd {
        ClientCommand::Subscribe => {
            let snapshot = state.snapshot_event().await;
            writer
                .write_all((serde_json::to_string(&snapshot)? + "\n").as_bytes())
                .await?;
        }
        ClientCommand::Focus {
            window_index,
            tab_index,
        } => match state.focus(window_index, tab_index).await {
            Ok(event) => {
                let _ = event_tx.send(event.clone());
                writer
                    .write_all((serde_json::to_string(&event)? + "\n").as_bytes())
                    .await?;
            }
            Err(e) => {
                warn!("focus failed: {e}");
                let ack = serde_json::json!({ "type": "focus", "ok": false });
                writer
                    .write_all((ack.to_string() + "\n").as_bytes())
                    .await?;
            }
        },
        ClientCommand::List => {
            let sessions = state.session_list().await;
            writer
                .write_all((serde_json::to_string(&sessions)? + "\n").as_bytes())
                .await?;
        }
        ClientCommand::Status { verbose } => {
            let metrics = if verbose {
                Some(serde_json::to_value(crate::daemon::metrics::snapshot())?)
            } else {
                None
            };
            let update = crate::telemetry::config::SessionsConfig::load(&state.config().home)
                .ok()
                .and_then(|cfg| cfg.update_info())
                .map(|info| {
                    serde_json::json!({
                        "available_version": info.available_version,
                        "urgency": info.urgency.as_str(),
                        "message": info.message,
                        "changelog_url": info.changelog_url,
                    })
                });
            let status = ServerEvent::Status {
                healthy: true,
                session_count: state.session_count().await,
                version: state.version().await,
                last_poll_at: state.last_poll_at().await,
                booting: state.is_booting().await,
                metrics,
                app_version: Some(crate::version::VERSION.to_string()),
                update,
            };
            writer
                .write_all((serde_json::to_string(&status)? + "\n").as_bytes())
                .await?;
        }
        ClientCommand::Refresh => {
            let event = match state.refresh_from_tmux().await {
                Some(event) => event,
                None => state.snapshot_event().await,
            };
            let _ = event_tx.send(event.clone());
            writer
                .write_all((serde_json::to_string(&event)? + "\n").as_bytes())
                .await?;
        }
        ClientCommand::Rename { session_id, title } => {
            match state.rename_session(&session_id, title).await {
                Ok(event) => {
                    let _ = event_tx.send(event.clone());
                    writer
                        .write_all((serde_json::to_string(&event)? + "\n").as_bytes())
                        .await?;
                }
                Err(e) => {
                    warn!("rename failed: {e}");
                    let ack = serde_json::json!({ "type": "rename", "ok": false });
                    writer
                        .write_all((ack.to_string() + "\n").as_bytes())
                        .await?;
                }
            }
        }
        ClientCommand::CloseSession { session_id } => {
            match state.close_session(&session_id).await {
                Ok(event) => {
                    let _ = event_tx.send(event.clone());
                    writer
                        .write_all((serde_json::to_string(&event)? + "\n").as_bytes())
                        .await?;
                }
                Err(e) => {
                    warn!("close session failed: {e}");
                    let ack = serde_json::json!({ "type": "close_session", "ok": false });
                    writer
                        .write_all((ack.to_string() + "\n").as_bytes())
                        .await?;
                }
            }
        }
        ClientCommand::AcknowledgeCompletion { session_id } => {
            match state.acknowledge_completion(&session_id).await {
                Ok(Some(event)) => {
                    let _ = event_tx.send(event.clone());
                    writer
                        .write_all((serde_json::to_string(&event)? + "\n").as_bytes())
                        .await?;
                }
                Ok(None) => {
                    let ack = serde_json::json!({ "type": "acknowledge_completion", "ok": true });
                    writer
                        .write_all((ack.to_string() + "\n").as_bytes())
                        .await?;
                }
                Err(e) => {
                    warn!("acknowledge completion failed: {e}");
                    let ack = serde_json::json!({ "type": "acknowledge_completion", "ok": false });
                    writer
                        .write_all((ack.to_string() + "\n").as_bytes())
                        .await?;
                }
            }
        }
        ClientCommand::TelemetryFlush => {
            crate::daemon::telemetry_worker::handle_telemetry_flush(state, state.config()).await;
            let ack = serde_json::json!({ "type": "telemetry_flush", "ok": true });
            writer
                .write_all((ack.to_string() + "\n").as_bytes())
                .await?;
        }
        ClientCommand::RestoreComplete => {
            let _ = finish_reconcile(state.as_ref(), &event_tx).await;
            let ack = serde_json::json!({ "type": "restore_complete", "ok": true });
            writer
                .write_all((ack.to_string() + "\n").as_bytes())
                .await?;
        }
        ClientCommand::PrepareRestore => {
            state.set_booting(true).await;
            let ack = serde_json::json!({ "type": "prepare_restore", "ok": true });
            writer
                .write_all((ack.to_string() + "\n").as_bytes())
                .await?;
        }
        ClientCommand::FlushManifest => {
            if let Err(err) = state.manifest_sync().flush_all(state.config()) {
                warn!("flush manifest failed: {err}");
                let ack = serde_json::json!({ "type": "flush_manifest", "ok": false });
                writer
                    .write_all((ack.to_string() + "\n").as_bytes())
                    .await?;
            } else {
                let ack = serde_json::json!({ "type": "flush_manifest", "ok": true });
                writer
                    .write_all((ack.to_string() + "\n").as_bytes())
                    .await?;
            }
        }
    }
    Ok(())
}

/// Start sessionsd when the socket is missing or stale.
pub fn ensure_daemon_running(config: &Config) -> Result<()> {
    if socket_responds(&config.socket_path) {
        return Ok(());
    }
    if config.socket_path.exists() {
        let _ = std::fs::remove_file(&config.socket_path);
    }
    if let Some(parent) = config.socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let sessions = crate::daemon::tmux::sessions_binary();
    std::process::Command::new(&sessions)
        .args(["daemon", "--foreground"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {}", sessions.display()))?;
    for _ in 0..30 {
        if socket_responds(&config.socket_path) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::bail!("sessionsd did not become ready");
}

pub fn socket_responds(socket_path: &Path) -> bool {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let Ok(mut stream) = UnixStream::connect(socket_path) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let cmd = r#"{"cmd":"status"}"#;
    if stream.write_all(format!("{cmd}\n").as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 256];
    stream.read(&mut buf).is_ok()
}
