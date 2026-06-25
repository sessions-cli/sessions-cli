//! Platform-native process metadata — avoids `lsof`/`ps` subprocesses on hot paths.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessFacts {
    pub pid: u32,
    pub cwd: Option<String>,
    pub started_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CwdCacheEntry {
    tmux_cwd: String,
    cwd: Option<String>,
}

static CWD_CACHE: LazyLock<Mutex<HashMap<u32, CwdCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Resolve cwd for a pane pid, using cache keyed by `(pid, tmux_cwd)`.
pub fn cwd_for_pane_pid(tmux_cwd: &str, pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let mut cache = CWD_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = cache.get(&pid) {
        if entry.tmux_cwd == tmux_cwd {
            return entry.cwd.clone();
        }
    }
    let cwd = inspect_process_facts(pid).and_then(|facts| facts.cwd);
    cache.insert(
        pid,
        CwdCacheEntry {
            tmux_cwd: tmux_cwd.to_string(),
            cwd: cwd.clone(),
        },
    );
    cwd
}

/// Drop cached cwd entries at the start of each tmux poll.
pub fn clear_cwd_cache() {
    if let Ok(mut cache) = CWD_CACHE.lock() {
        cache.clear();
    }
}

pub fn process_start_time(pid: u32) -> Option<SystemTime> {
    if pid == 0 {
        return None;
    }
    inspect_process_facts(pid).and_then(|facts| facts.started_at)
}

fn inspect_process_facts(pid: u32) -> Option<ProcessFacts> {
    let mut facts = ProcessFacts {
        pid,
        cwd: None,
        started_at: None,
    };
    #[cfg(target_os = "macos")]
    {
        if let Some(native) = macos::facts(pid) {
            facts = native;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(native) = linux::facts(pid) {
            facts = native;
        }
    }
    if facts.cwd.is_none() {
        facts.cwd = fallback::cwd_via_lsof(pid);
    }
    if facts.started_at.is_none() {
        facts.started_at = fallback::start_time_via_ps(pid);
    }
    if facts.cwd.is_some() || facts.started_at.is_some() {
        Some(facts)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::ProcessFacts;
    use std::ffi::c_void;
    use std::time::{Duration, SystemTime};

    const PROC_PIDTBSDINFO: i32 = 3;
    const PROC_PIDVNODEPATHINFO: i32 = 9;

    // Layout from macOS SDK `sys/proc_info.h` (sizeof proc_bsdinfo = 136).
    #[repr(C)]
    struct ProcBsdInfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_xstatus: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: u32,
        pbi_gid: u32,
        pbi_ruid: u32,
        pbi_rgid: u32,
        pbi_svuid: u32,
        pbi_svgid: u32,
        rfu_1: u32,
        pbi_comm: [u8; 16],
        pbi_name: [u8; 32],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }

    // sizeof vnode_info_path = 1176; vip_path offset = 152.
    #[repr(C)]
    struct VnodeInfoPath {
        _vi: [u8; 152],
        path: [i8; 1024],
    }

    #[repr(C)]
    struct ProcVnodePathInfo {
        cdir: VnodeInfoPath,
        _rdir: VnodeInfoPath,
    }

    #[link(name = "proc", kind = "dylib")]
    extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut c_void,
            buffersize: i32,
        ) -> i32;
    }

    pub fn facts(pid: u32) -> Option<ProcessFacts> {
        let cwd = cwd_via_libproc(pid);
        let started_at = start_time_via_libproc(pid);
        if cwd.is_none() && started_at.is_none() {
            return None;
        }
        Some(ProcessFacts {
            pid,
            cwd,
            started_at,
        })
    }

    fn cwd_via_libproc(pid: u32) -> Option<String> {
        let mut info = ProcVnodePathInfo {
            cdir: VnodeInfoPath {
                _vi: [0; 152],
                path: [0; 1024],
            },
            _rdir: VnodeInfoPath {
                _vi: [0; 152],
                path: [0; 1024],
            },
        };
        let size = std::mem::size_of::<ProcVnodePathInfo>() as i32;
        let wrote = unsafe {
            proc_pidinfo(
                pid as i32,
                PROC_PIDVNODEPATHINFO,
                0,
                (&mut info as *mut ProcVnodePathInfo).cast(),
                size,
            )
        };
        if wrote <= 0 {
            return None;
        }
        let path = unsafe {
            std::ffi::CStr::from_ptr(info.cdir.path.as_ptr())
                .to_string_lossy()
                .into_owned()
        };
        if path.is_empty() {
            None
        } else {
            Some(path)
        }
    }

    fn start_time_via_libproc(pid: u32) -> Option<SystemTime> {
        let mut info = ProcBsdInfo {
            pbi_flags: 0,
            pbi_status: 0,
            pbi_xstatus: 0,
            pbi_pid: 0,
            pbi_ppid: 0,
            pbi_uid: 0,
            pbi_gid: 0,
            pbi_ruid: 0,
            pbi_rgid: 0,
            pbi_svuid: 0,
            pbi_svgid: 0,
            rfu_1: 0,
            pbi_comm: [0; 16],
            pbi_name: [0; 32],
            pbi_nfiles: 0,
            pbi_pgid: 0,
            pbi_pjobc: 0,
            e_tdev: 0,
            e_tpgid: 0,
            pbi_nice: 0,
            pbi_start_tvsec: 0,
            pbi_start_tvusec: 0,
        };
        let size = std::mem::size_of::<ProcBsdInfo>() as i32;
        let wrote = unsafe {
            proc_pidinfo(
                pid as i32,
                PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut ProcBsdInfo).cast(),
                size,
            )
        };
        if wrote <= 0 || info.pbi_start_tvsec == 0 {
            return None;
        }
        let usec_nanos = info
            .pbi_start_tvusec
            .checked_mul(1000)?
            .min(u64::from(u32::MAX)) as u32;
        SystemTime::UNIX_EPOCH.checked_add(Duration::new(info.pbi_start_tvsec, usec_nanos))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::ProcessFacts;
    use std::time::{Duration, SystemTime};

    pub fn facts(pid: u32) -> Option<ProcessFacts> {
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
            .ok()
            .map(|path| path.display().to_string());
        let started_at = start_time_from_proc(pid);
        Some(ProcessFacts {
            pid,
            cwd,
            started_at,
        })
    }

    fn start_time_from_proc(pid: u32) -> Option<SystemTime> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // field 22 is starttime (clock ticks since boot)
        let close = stat.rfind(')')?;
        let rest = stat.get(close + 2..)?;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let start_ticks: u64 = fields.get(19)?.parse().ok()?;
        let clk_tck = std::fs::read_to_string("/proc/self/stat")
            .ok()
            .and_then(|_| {
                // CLK_TCK is usually 100 on Linux
                Some(100u64)
            })
            .unwrap_or(100);
        let boot_offset_secs = start_ticks / clk_tck;
        // Approximate: use uptime for boot time
        let uptime = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())?;
        let now = SystemTime::now();
        let elapsed = Duration::from_secs_f64(uptime);
        now.checked_sub(elapsed)
            .and_then(|boot| boot.checked_add(Duration::from_secs(boot_offset_secs)))
    }
}

mod fallback {
    use chrono::{Local, NaiveDateTime};
    use std::process::Command;
    use std::time::{Duration, SystemTime};

    pub(super) fn cwd_via_lsof(pid: u32) -> Option<String> {
        crate::daemon::metrics::record_lsof_call();
        let output = Command::new("lsof")
            .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some(path) = line.strip_prefix('n') {
                let path = path.trim();
                if !path.is_empty() {
                    return Some(path.to_string());
                }
            }
        }
        None
    }

    pub(super) fn start_time_via_ps(pid: u32) -> Option<SystemTime> {
        crate::daemon::metrics::record_ps_call();
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stamp = stdout.trim();
        // macOS/BSD `ps lstart`: "Fri 12 Jun 08:25:30 2026" (day before month, local time).
        let naive = NaiveDateTime::parse_from_str(stamp, "%a %e %b %T %Y")
            .or_else(|_| NaiveDateTime::parse_from_str(stamp, "%a %d %b %T %Y"))
            .or_else(|_| NaiveDateTime::parse_from_str(stamp, "%a %b %e %T %Y"))
            .or_else(|_| NaiveDateTime::parse_from_str(stamp, "%a %b %d %T %Y"))
            .ok()?;
        let local = naive.and_local_timezone(Local).single()?;
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(local.timestamp() as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_cache_reuses_entry_for_same_pid_and_tmux_cwd() {
        clear_cwd_cache();
        let pid = std::process::id();
        let tmux_cwd = "/tmp";
        let first = cwd_for_pane_pid(tmux_cwd, pid);
        let second = cwd_for_pane_pid(tmux_cwd, pid);
        assert_eq!(first, second);
    }

    #[test]
    fn cwd_cache_invalidates_when_tmux_cwd_changes() {
        clear_cwd_cache();
        let pid = std::process::id();
        let _ = cwd_for_pane_pid("/tmp", pid);
        {
            let cache = CWD_CACHE.lock().unwrap();
            assert_eq!(cache.get(&pid).map(|e| e.tmux_cwd.as_str()), Some("/tmp"));
        }
        let _ = cwd_for_pane_pid("/var", pid);
        {
            let cache = CWD_CACHE.lock().unwrap();
            assert_eq!(cache.get(&pid).map(|e| e.tmux_cwd.as_str()), Some("/var"));
        }
    }

    #[test]
    fn current_process_start_time_is_available() {
        let pid = std::process::id();
        assert!(process_start_time(pid).is_some());
    }
}
