use crate::cli::config_with_socket;
use crate::config::Config;
use crate::daemon;
use crate::paths;
use anyhow::Result;
use std::path::Path;
use std::path::PathBuf;

pub fn run(
    socket: Option<PathBuf>,
    poll_interval: u64,
    foreground: bool,
) -> Result<()> {
    let mut config = config_with_socket(socket);
    config.poll_interval_ms = poll_interval;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(daemon::run_daemon(config, foreground))
}

pub fn ensure_daemon(config: &Config) -> Result<()> {
    if daemon::server::socket_responds(&config.socket_path) {
        return Ok(());
    }
    let sessions = paths::resolve_binary(&config.home)
        .to_string_lossy()
        .into_owned();
    std::process::Command::new(&sessions)
        .args(["daemon", "--foreground"])
        .stdout(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&config.log_path)?,
        )
        .stderr(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&config.log_path)?,
        )
        .spawn()?;
    for _ in 0..20 {
        if daemon::server::socket_responds(&config.socket_path) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::bail!("sessionsd failed to start")
}

pub fn send_sync(socket_path: &Path, cmd: &crate::model::ClientCommand) -> Result<String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket_path)?;
    let line = serde_json::to_string(cmd)? + "\n";
    stream.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    Ok(response.trim().to_string())
}