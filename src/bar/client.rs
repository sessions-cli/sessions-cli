use crate::config::Config;
use crate::model::{ClientCommand, ServerEvent};
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub struct DaemonClient {
    socket_path: std::path::PathBuf,
}

impl DaemonClient {
    pub fn new(socket_path: std::path::PathBuf) -> Self {
        Self { socket_path }
    }

    pub fn from_config(config: &Config) -> Self {
        Self::new(config.socket_path.clone())
    }

    pub fn subscribe(&self) -> Result<EventReceiver> {
        let (tx, rx) = mpsc::channel();
        let socket_path = self.socket_path.clone();
        thread::spawn(move || {
            let mut backoff = Duration::from_millis(200);
            loop {
                match subscribe_loop(&socket_path, &tx) {
                    Ok(()) => backoff = Duration::from_millis(200),
                    Err(e) => {
                        let _ = tx.send(ClientEvent::Disconnected(format!("{e}")));
                        thread::sleep(backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(8));
                    }
                }
            }
        });
        Ok(EventReceiver { rx })
    }

    pub fn focus(&self, window_index: u32, tab_index: Option<u32>) -> Result<()> {
        send_command_with_timeout(
            &self.socket_path,
            &ClientCommand::Focus {
                window_index,
                tab_index,
            },
            Duration::from_millis(500),
        )
    }

    pub fn refresh(&self) -> Result<()> {
        send_command(&self.socket_path, &ClientCommand::Refresh)
    }

    pub fn refresh_async(&self) {
        let socket_path = self.socket_path.clone();
        thread::spawn(move || {
            let _ = send_command(&socket_path, &ClientCommand::Refresh);
        });
    }

    pub fn refresh_snapshot(&self) -> Result<Option<ServerEvent>> {
        send_command_for_event(&self.socket_path, &ClientCommand::Refresh)
    }

    pub fn rename(&self, session_id: &str, title: String) -> Result<Option<ServerEvent>> {
        send_command_for_event(
            &self.socket_path,
            &ClientCommand::Rename {
                session_id: session_id.to_string(),
                title,
            },
        )
    }

    pub fn close_session(&self, session_id: &str) -> Result<Option<ServerEvent>> {
        send_command_for_event(
            &self.socket_path,
            &ClientCommand::CloseSession {
                session_id: session_id.to_string(),
            },
        )
    }

    pub fn acknowledge_completion(&self, session_id: &str) -> Result<()> {
        send_command_with_timeout(
            &self.socket_path,
            &ClientCommand::AcknowledgeCompletion {
                session_id: session_id.to_string(),
            },
            Duration::from_millis(500),
        )
    }

    pub fn telemetry_flush_async(&self) {
        let socket_path = self.socket_path.clone();
        thread::spawn(move || {
            let _ = send_command(&socket_path, &ClientCommand::TelemetryFlush);
        });
    }
}

pub enum ClientEvent {
    Snapshot {
        sessions: Vec<crate::model::Session>,
        version: u64,
    },
    Patch(ServerEvent),
    Disconnected(String),
}

pub struct EventReceiver {
    rx: mpsc::Receiver<ClientEvent>,
}

impl EventReceiver {
    pub fn recv(&self) -> Option<ClientEvent> {
        self.rx.recv().ok()
    }

    pub fn try_recv(&self) -> Option<ClientEvent> {
        self.rx.try_recv().ok()
    }
}

fn send_command(socket_path: &Path, cmd: &ClientCommand) -> Result<()> {
    send_command_with_timeout(socket_path, cmd, Duration::from_secs(5))
}

fn send_command_for_event(socket_path: &Path, cmd: &ClientCommand) -> Result<Option<ServerEvent>> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connect {}", socket_path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let line = serde_json::to_string(cmd)? + "\n";
    stream.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    if reader.read_line(&mut buf)? == 0 {
        return Ok(None);
    }
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .with_context(|| format!("parse daemon response: {trimmed}"))
}

fn send_command_with_timeout(
    socket_path: &Path,
    cmd: &ClientCommand,
    timeout: Duration,
) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connect {}", socket_path.display()))?;
    stream.set_read_timeout(Some(timeout))?;
    let line = serde_json::to_string(cmd)? + "\n";
    stream.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    let _ = reader.read_line(&mut buf);
    Ok(())
}

fn subscribe_loop(socket_path: &Path, tx: &mpsc::Sender<ClientEvent>) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connect {}", socket_path.display()))?;
    // Block until the daemon pushes — idle sidebars are normal; a read timeout
    // was surfacing as spurious "reconnecting: Resource temporarily unavailable".
    let cmd = serde_json::to_string(&ClientCommand::Subscribe)? + "\n";
    stream.write_all(cmd.as_bytes())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                if line.trim().is_empty() {
                    continue;
                }
                let event: ServerEvent = serde_json::from_str(line.trim())?;
                match event {
                    ServerEvent::Snapshot { sessions, version } => {
                        let _ = tx.send(ClientEvent::Snapshot { sessions, version });
                    }
                    patch @ ServerEvent::Patch { .. } => {
                        let _ = tx.send(ClientEvent::Patch(patch));
                    }
                    _ => {}
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
}
