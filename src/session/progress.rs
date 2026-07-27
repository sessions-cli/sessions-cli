use std::cell::Cell;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static IS_TTY: AtomicBool = AtomicBool::new(false);

pub fn detect_tty() {
    IS_TTY.store(
        unsafe { libc::isatty(libc::STDERR_FILENO) != 0 },
        Ordering::Relaxed,
    );
}

fn is_tty() -> bool {
    IS_TTY.load(Ordering::Relaxed)
}

fn flush_stderr() {
    let _ = std::io::stderr().flush();
}

/// Quiet multi-phase status for `sessions up` / cold-boot restore.
///
/// On a TTY, the current phase rewrites a single dim line; completed steps
/// print green with elapsed ms. Non-TTY (CI/logs) gets plain line-per-event.
pub struct Progress {
    start: Instant,
    phase_start: Cell<Instant>,
}

impl Progress {
    pub fn new() -> Self {
        detect_tty();
        let now = Instant::now();
        Self {
            start: now,
            phase_start: Cell::new(now),
        }
    }

    pub fn banner(&self, msg: &str) {
        if is_tty() {
            eprintln!("\r\x1b[K{msg}");
        } else {
            eprintln!("{msg}");
        }
    }

    pub fn phase(&self, msg: &str) {
        self.phase_start.set(Instant::now());
        if is_tty() {
            eprint!("\r\x1b[K\x1b[2m  {msg}…\x1b[0m");
            flush_stderr();
        } else {
            eprintln!("  {msg}…");
        }
    }

    pub fn phase_done(&self, msg: &str) {
        let ms = self.phase_start.get().elapsed().as_millis();
        if is_tty() {
            eprintln!("\r\x1b[K\x1b[32m  {msg}\x1b[0m \x1b[2m({ms}ms)\x1b[0m");
        } else {
            eprintln!("  {msg} ({ms}ms)");
        }
    }

    pub fn item(&self, index: usize, total: usize, msg: &str) {
        if is_tty() {
            eprint!("\r\x1b[K\x1b[2m  {index}/{total} {msg}\x1b[0m");
            flush_stderr();
        } else {
            eprintln!("  {index}/{total} {msg}");
        }
    }

    pub fn item_done(&self, index: usize, total: usize, msg: &str) {
        if is_tty() {
            eprintln!("\r\x1b[K  {index}/{total} \x1b[32m{msg}\x1b[0m");
        } else {
            eprintln!("  {index}/{total} {msg}");
        }
    }

    pub fn final_line(&self, msg: &str) {
        let ms = self.start.elapsed().as_millis();
        if is_tty() {
            eprintln!("\r\x1b[K{msg} \x1b[2m({ms}ms)\x1b[0m");
        } else {
            eprintln!("{msg} ({ms}ms)");
        }
    }

    /// Non-fatal notice (e.g. skipped workspace with missing cwd). Clears any
    /// in-progress phase line on TTY so the warning is not overwritten.
    pub fn warn(&self, msg: &str) {
        if is_tty() {
            eprintln!("\r\x1b[K\x1b[33m  ! {msg}\x1b[0m");
        } else {
            eprintln!("  ! {msg}");
        }
    }
}

/// Standalone warning when no [`Progress`] is available (e.g. bootstrap helpers).
pub fn warn_line(msg: &str) {
    detect_tty();
    if is_tty() {
        eprintln!("\r\x1b[K\x1b[33m  ! {msg}\x1b[0m");
    } else {
        eprintln!("  ! {msg}");
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}
