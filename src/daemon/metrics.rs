use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static TMUX_COMMANDS: AtomicU64 = AtomicU64::new(0);
static TMUX_DURATION_US: AtomicU64 = AtomicU64::new(0);
static LSOF_CALLS: AtomicU64 = AtomicU64::new(0);
static PS_CALLS: AtomicU64 = AtomicU64::new(0);
static AGENT_SCANS: AtomicU64 = AtomicU64::new(0);
static AGENT_SCAN_DURATION_US: AtomicU64 = AtomicU64::new(0);
static LOG_PARSES: AtomicU64 = AtomicU64::new(0);
static REFRESH_COUNT: AtomicU64 = AtomicU64::new(0);
static REFRESH_DURATION_US: AtomicU64 = AtomicU64::new(0);
static REFRESH_MAX_US: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT_BYTES: AtomicU64 = AtomicU64::new(0);
static HOOK_APPLY_COUNT: AtomicU64 = AtomicU64::new(0);
static HOOK_APPLY_DURATION_US: AtomicU64 = AtomicU64::new(0);
static HOOK_APPLY_MAX_US: AtomicU64 = AtomicU64::new(0);
static SPOOL_BACKLOG: AtomicU64 = AtomicU64::new(0);
static SUBSCRIBER_COUNT: AtomicU64 = AtomicU64::new(0);
static POLL_COUNT: AtomicU64 = AtomicU64::new(0);
static LAST_POLL_UNIX_MS: AtomicU64 = AtomicU64::new(0);

static NOTIFY_APPLIED: AtomicU64 = AtomicU64::new(0);
static NOTIFY_DUPLICATE: AtomicU64 = AtomicU64::new(0);
static NOTIFY_DEFERRED: AtomicU64 = AtomicU64::new(0);
static NOTIFY_UNKNOWN_SESSION: AtomicU64 = AtomicU64::new(0);
static NOTIFY_SOCKET_FAILED: AtomicU64 = AtomicU64::new(0);
static NOTIFY_SPOOL_FAILED: AtomicU64 = AtomicU64::new(0);

pub struct TmuxCommandTimer {
    start: Instant,
}

impl TmuxCommandTimer {
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Drop for TmuxCommandTimer {
    fn drop(&mut self) {
        record_tmux_command(self.start.elapsed().as_micros() as u64);
    }
}

pub fn record_tmux_command(duration_us: u64) {
    TMUX_COMMANDS.fetch_add(1, Ordering::Relaxed);
    TMUX_DURATION_US.fetch_add(duration_us, Ordering::Relaxed);
}

pub fn record_lsof_call() {
    LSOF_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_ps_call() {
    PS_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_agent_scan(duration_us: u64) {
    AGENT_SCANS.fetch_add(1, Ordering::Relaxed);
    AGENT_SCAN_DURATION_US.fetch_add(duration_us, Ordering::Relaxed);
}

pub fn record_log_parse() {
    LOG_PARSES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_refresh(duration_us: u64) {
    REFRESH_COUNT.fetch_add(1, Ordering::Relaxed);
    REFRESH_DURATION_US.fetch_add(duration_us, Ordering::Relaxed);
    update_max(&REFRESH_MAX_US, duration_us);
}

pub fn record_snapshot_bytes(bytes: u64) {
    SNAPSHOT_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

pub fn record_hook_apply(duration_us: u64) {
    HOOK_APPLY_COUNT.fetch_add(1, Ordering::Relaxed);
    HOOK_APPLY_DURATION_US.fetch_add(duration_us, Ordering::Relaxed);
    update_max(&HOOK_APPLY_MAX_US, duration_us);
}

pub fn set_spool_backlog(count: u64) {
    SPOOL_BACKLOG.store(count, Ordering::Relaxed);
}

pub fn set_subscriber_count(count: u64) {
    SUBSCRIBER_COUNT.store(count, Ordering::Relaxed);
}

pub fn record_poll() {
    POLL_COUNT.fetch_add(1, Ordering::Relaxed);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    LAST_POLL_UNIX_MS.store(ms, Ordering::Relaxed);
}

pub fn record_notify_applied() {
    NOTIFY_APPLIED.fetch_add(1, Ordering::Relaxed);
}

pub fn record_notify_duplicate() {
    NOTIFY_DUPLICATE.fetch_add(1, Ordering::Relaxed);
}

pub fn record_notify_deferred() {
    NOTIFY_DEFERRED.fetch_add(1, Ordering::Relaxed);
}

pub fn record_notify_unknown_session() {
    NOTIFY_UNKNOWN_SESSION.fetch_add(1, Ordering::Relaxed);
}

pub fn record_notify_socket_failed() {
    NOTIFY_SOCKET_FAILED.fetch_add(1, Ordering::Relaxed);
}

pub fn record_notify_spool_failed() {
    NOTIFY_SPOOL_FAILED.fetch_add(1, Ordering::Relaxed);
}

fn update_max(atom: &AtomicU64, value: u64) {
    let mut current = atom.load(Ordering::Relaxed);
    while value > current {
        match atom.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(v) => current = v,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookOutcomes {
    pub applied: u64,
    pub duplicate: u64,
    pub deferred: u64,
    pub unknown_session: u64,
    pub socket_failed: u64,
    pub spool_failed: u64,
}

pub fn hook_outcomes_snapshot() -> HookOutcomes {
    HookOutcomes {
        applied: NOTIFY_APPLIED.load(Ordering::Relaxed),
        duplicate: NOTIFY_DUPLICATE.load(Ordering::Relaxed),
        deferred: NOTIFY_DEFERRED.load(Ordering::Relaxed),
        unknown_session: NOTIFY_UNKNOWN_SESSION.load(Ordering::Relaxed),
        socket_failed: NOTIFY_SOCKET_FAILED.load(Ordering::Relaxed),
        spool_failed: NOTIFY_SPOOL_FAILED.load(Ordering::Relaxed),
    }
}

pub fn take_hook_outcomes() -> HookOutcomes {
    HookOutcomes {
        applied: NOTIFY_APPLIED.swap(0, Ordering::Relaxed),
        duplicate: NOTIFY_DUPLICATE.swap(0, Ordering::Relaxed),
        deferred: NOTIFY_DEFERRED.swap(0, Ordering::Relaxed),
        unknown_session: NOTIFY_UNKNOWN_SESSION.swap(0, Ordering::Relaxed),
        socket_failed: NOTIFY_SOCKET_FAILED.swap(0, Ordering::Relaxed),
        spool_failed: NOTIFY_SPOOL_FAILED.swap(0, Ordering::Relaxed),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeMetrics {
    pub tmux_commands: u64,
    pub tmux_duration_us: u64,
    pub lsof_calls: u64,
    pub ps_calls: u64,
    pub agent_scans: u64,
    pub agent_scan_duration_us: u64,
    pub log_parses: u64,
    pub refresh_count: u64,
    pub refresh_duration_us: u64,
    pub refresh_p95_us: u64,
    pub snapshot_bytes: u64,
    pub hook_apply_count: u64,
    pub hook_apply_duration_us: u64,
    pub hook_apply_p95_us: u64,
    pub spool_backlog: u64,
    pub subscriber_count: u64,
    pub poll_count: u64,
    pub last_poll_unix_ms: u64,
    pub hook_outcomes: HookOutcomes,
}

impl RuntimeMetrics {
    pub fn refresh_p95_us(&self) -> u64 {
        self.refresh_p95_us
    }

    pub fn hook_apply_p95_us(&self) -> u64 {
        self.hook_apply_p95_us
    }
}

pub fn snapshot() -> RuntimeMetrics {
    RuntimeMetrics {
        tmux_commands: TMUX_COMMANDS.load(Ordering::Relaxed),
        tmux_duration_us: TMUX_DURATION_US.load(Ordering::Relaxed),
        lsof_calls: LSOF_CALLS.load(Ordering::Relaxed),
        ps_calls: PS_CALLS.load(Ordering::Relaxed),
        agent_scans: AGENT_SCANS.load(Ordering::Relaxed),
        agent_scan_duration_us: AGENT_SCAN_DURATION_US.load(Ordering::Relaxed),
        log_parses: LOG_PARSES.load(Ordering::Relaxed),
        refresh_count: REFRESH_COUNT.load(Ordering::Relaxed),
        refresh_duration_us: REFRESH_DURATION_US.load(Ordering::Relaxed),
        refresh_p95_us: REFRESH_MAX_US.load(Ordering::Relaxed),
        snapshot_bytes: SNAPSHOT_BYTES.load(Ordering::Relaxed),
        hook_apply_count: HOOK_APPLY_COUNT.load(Ordering::Relaxed),
        hook_apply_duration_us: HOOK_APPLY_DURATION_US.load(Ordering::Relaxed),
        hook_apply_p95_us: HOOK_APPLY_MAX_US.load(Ordering::Relaxed),
        spool_backlog: SPOOL_BACKLOG.load(Ordering::Relaxed),
        subscriber_count: SUBSCRIBER_COUNT.load(Ordering::Relaxed),
        poll_count: POLL_COUNT.load(Ordering::Relaxed),
        last_poll_unix_ms: LAST_POLL_UNIX_MS.load(Ordering::Relaxed),
        hook_outcomes: hook_outcomes_snapshot(),
    }
}