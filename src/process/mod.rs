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

type CwdCache = HashMap<u32, CwdCacheEntry>;
type CmdlineCache = HashMap<u32, Option<String>>;
type PaneCmdCache = HashMap<(u32, String), Option<String>>;

static CWD_CACHE: LazyLock<Mutex<CwdCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static CMDLINE_CACHE: LazyLock<Mutex<CmdlineCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static PANE_CMD_CACHE: LazyLock<Mutex<PaneCmdCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

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

/// Drop cached cwd/command entries at the start of each tmux poll.
pub fn clear_cwd_cache() {
    if let Ok(mut cache) = CWD_CACHE.lock() {
        cache.clear();
    }
    if let Ok(mut cache) = CMDLINE_CACHE.lock() {
        cache.clear();
    }
    if let Ok(mut cache) = PANE_CMD_CACHE.lock() {
        cache.clear();
    }
}

pub fn process_start_time(pid: u32) -> Option<SystemTime> {
    if pid == 0 {
        return None;
    }
    inspect_process_facts(pid).and_then(|facts| facts.started_at)
}

/// Full argv string for a process (space-joined), when available.
pub fn command_line_for_pid(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let mut cache = CMDLINE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = cache.get(&pid) {
        return entry.clone();
    }
    let cmdline = read_command_line(pid);
    cache.insert(pid, cmdline.clone());
    cmdline
}

/// Resolve the live command running in a tmux pane.
///
/// `pane_current_command` is only a process name (e.g. `Python` / `python3.13`).
/// This walks the pane process tree and returns a full argv when it can match
/// that name — so `./train.py` shows up instead of bare `python3.13`.
pub fn foreground_command_for_pane(pane_pid: u32, current_command: &str) -> Option<String> {
    if pane_pid == 0 {
        return None;
    }
    let current = current_command.trim();
    if current.is_empty() || is_shell_comm(current) {
        return None;
    }

    let cache_key = (pane_pid, current.to_string());
    {
        let cache = PANE_CMD_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(&cache_key) {
            return entry.clone();
        }
    }

    let resolved = resolve_foreground_command(pane_pid, current);
    let mut cache = PANE_CMD_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.insert(cache_key, resolved.clone());
    resolved
}

fn resolve_foreground_command(pane_pid: u32, current: &str) -> Option<String> {
    let target = normalize_comm(current);
    // Only language launchers (Python/Node/…) should pick an unrelated child argv.
    // Agent binaries (grok-0.2.106-ma) spawn MCP helpers; those must not win by
    // having a longer cmdline than the agent process itself.
    let current_is_launcher = is_language_launcher_comm(&target);
    // score, depth (shallower wins), cmdline
    let mut best: Option<(u8, u32, String)> = None;

    for (depth, pid) in pane_process_candidates(pane_pid) {
        let Some(cmdline) = command_line_for_pid(pid) else {
            continue;
        };
        let cmdline = cmdline.trim();
        if cmdline.is_empty() {
            continue;
        }
        let first = cmdline.split_whitespace().next().unwrap_or("");
        let first_base = basename(first);
        let score = if comm_matches(&normalize_comm(first_base), &target)
            || comm_matches(&normalize_comm(first), &target)
        {
            3u8
        } else if cmdline_matches_current(cmdline, &target) {
            2
        } else if current_is_launcher && pid != pane_pid && !is_shell_comm(first_base) {
            // Prefer non-shell descendants when tmux name is an interpreter alias
            // (e.g. current=Python, child argv0=/usr/bin/env … script.py).
            1
        } else {
            0
        };
        if score == 0 {
            continue;
        }
        let better = match &best {
            None => true,
            Some((best_score, best_depth, best_cmd)) => {
                if score != *best_score {
                    score > *best_score
                } else if depth != *best_depth {
                    // Prefer the process closer to the pane (agent over MCP child).
                    depth < *best_depth
                } else {
                    // Same depth: longer cmdline is richer (full python argv vs bare name).
                    cmdline.len() > best_cmd.len()
                }
            }
        };
        if better {
            best = Some((score, depth, cmdline.to_string()));
        }
    }

    best.map(|(_, _, cmd)| cmd)
}

/// BFS over the pane process tree as `(depth, pid)` pairs. Depth 0 is the pane shell.
fn pane_process_candidates(pane_pid: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut frontier = vec![pane_pid];
    let mut depth = 0u32;
    while !frontier.is_empty() && depth < 5 {
        let mut next = Vec::new();
        for pid in frontier {
            if !seen.insert(pid) {
                continue;
            }
            out.push((depth, pid));
            for child in child_pids(pid) {
                if !seen.contains(&child) {
                    next.push(child);
                }
            }
        }
        frontier = next;
        depth += 1;
    }
    out
}

fn cmdline_matches_current(cmdline: &str, target: &str) -> bool {
    cmdline
        .split_whitespace()
        .any(|token| comm_matches(&normalize_comm(basename(token)), target))
}

fn comm_matches(candidate: &str, target: &str) -> bool {
    if candidate.is_empty() || target.is_empty() {
        return false;
    }
    if candidate == target {
        return true;
    }
    // python3.13 ↔ python / Python (short version suffix)
    if candidate.starts_with(target) || target.starts_with(candidate) {
        if candidate.len().abs_diff(target.len()) <= 6 {
            return true;
        }
        // Versioned / platform agent binaries: argv0 is often bare `grok` while
        // tmux `pane_current_command` is the truncated on-disk name
        // `grok-0.2.106-ma` (from grok-0.2.106-macos-aarch64).
        let (shorter, longer) = if candidate.len() < target.len() {
            (candidate, target)
        } else {
            (target, candidate)
        };
        if versioned_binary_suffix_match(shorter, longer) {
            return true;
        }
    }
    false
}

/// True when `longer` is `shorter-<version|platform>` (not an unrelated compound name).
///
/// Accepts:
/// - version digit: `grok-0.2.106-ma`, `node-20`
/// - arch/platform packaging: `codex-aarch64-a`, `claude-macos-arm64`
///
/// Rejects: `code-helper`, `go-module-runner` (no version/arch hint after `-`).
fn versioned_binary_suffix_match(shorter: &str, longer: &str) -> bool {
    if shorter.is_empty() || !longer.starts_with(shorter) {
        return false;
    }
    let Some(suffix) = longer[shorter.len()..].strip_prefix('-') else {
        return false;
    };
    if suffix.is_empty() {
        return false;
    }
    if suffix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return true;
    }
    const ARCH_HINTS: &[&str] = &[
        "aarch64", "x86_64", "amd64", "arm64", "macos", "linux", "darwin", "windows", "mac",
    ];
    let lower = suffix.to_ascii_lowercase();
    ARCH_HINTS.iter().any(|hint| {
        lower == *hint
            || lower.starts_with(&format!("{hint}-"))
            || lower.starts_with(&format!("{hint}."))
            || lower.contains(&format!("-{hint}"))
            || lower.contains(&format!("-{hint}-"))
    })
}

/// Language launchers where score-1 child expansion is appropriate.
fn is_language_launcher_comm(name: &str) -> bool {
    let base = normalize_comm(name);
    matches!(
        base.as_str(),
        "env"
            | "python"
            | "python2"
            | "python3"
            | "node"
            | "nodejs"
            | "deno"
            | "bun"
            | "ruby"
            | "perl"
            | "php"
            | "lua"
            | "luajit"
            | "r"
            | "rscript"
            | "julia"
            | "dotnet"
            | "java"
    ) || base.starts_with("python")
        || base.starts_with("ruby")
        || base.starts_with("perl")
        || base.starts_with("php")
        || base.starts_with("node")
}

fn normalize_comm(name: &str) -> String {
    let base = basename(name.trim());
    base.to_ascii_lowercase()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_shell_comm(name: &str) -> bool {
    matches!(
        normalize_comm(name).as_str(),
        "zsh" | "bash" | "sh" | "fish" | "nu" | "-zsh" | "-bash" | "-sh" | "-fish"
    )
}

fn read_command_line(pid: u32) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        if let Some(cmd) = macos::command_line(pid) {
            return Some(cmd);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(cmd) = linux::command_line(pid) {
            return Some(cmd);
        }
    }
    fallback::command_line_via_ps(pid)
}

fn child_pids(pid: u32) -> Vec<u32> {
    #[cfg(target_os = "macos")]
    {
        let kids = macos::list_child_pids(pid);
        if !kids.is_empty() {
            return kids;
        }
    }
    #[cfg(target_os = "linux")]
    {
        let kids = linux::list_child_pids(pid);
        if !kids.is_empty() {
            return kids;
        }
    }
    fallback::child_pids_via_pgrep(pid)
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
        fn proc_listchildpids(pid: i32, buffer: *mut c_void, buffersize: i32) -> i32;
    }

    extern "C" {
        fn sysctl(
            name: *mut i32,
            namelen: u32,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
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

    pub fn list_child_pids(pid: u32) -> Vec<u32> {
        // proc_listchildpids returns the number of pids written.
        let mut buf = [0i32; 256];
        let n = unsafe {
            proc_listchildpids(
                pid as i32,
                buf.as_mut_ptr().cast(),
                std::mem::size_of_val(&buf) as i32,
            )
        };
        if n <= 0 {
            return Vec::new();
        }
        buf[..n as usize]
            .iter()
            .filter_map(|p| u32::try_from(*p).ok())
            .filter(|p| *p > 0)
            .collect()
    }

    pub fn command_line(pid: u32) -> Option<String> {
        // KERN_PROCARGS2: argc (i32) + exec path + argv[] + env (NUL-separated).
        const CTL_KERN: i32 = 1;
        const KERN_PROCARGS2: i32 = 49;
        let mut mib = [CTL_KERN, KERN_PROCARGS2, pid as i32];
        let mut size: usize = 0;
        let rc = unsafe {
            sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        // Size probe often fails on macOS; use a fixed buffer.
        let mut buf = vec![0u8; if rc == 0 && size > 0 { size } else { 64 * 1024 }];
        let mut size = buf.len();
        let rc = unsafe {
            sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 || size < 4 {
            return None;
        }
        buf.truncate(size);
        let argc = i32::from_ne_bytes(buf[0..4].try_into().ok()?);
        if argc <= 0 || argc > 4096 {
            return None;
        }
        let mut rest = &buf[4..];
        // exec path
        let exe_end = rest.iter().position(|&b| b == 0)?;
        rest = rest.get(exe_end + 1..)?;
        while rest.first() == Some(&0) {
            rest = &rest[1..];
        }
        let mut args = Vec::with_capacity(argc as usize);
        for _ in 0..argc {
            let end = rest.iter().position(|&b| b == 0)?;
            let arg = std::str::from_utf8(&rest[..end]).ok()?.to_string();
            args.push(arg);
            rest = rest.get(end + 1..)?;
        }
        if args.is_empty() {
            None
        } else {
            Some(args.join(" "))
        }
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

    pub fn command_line(pid: u32) -> Option<String> {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        if raw.is_empty() {
            return None;
        }
        let parts: Vec<&str> = raw
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .filter_map(|p| std::str::from_utf8(p).ok())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }

    pub fn list_child_pids(pid: u32) -> Vec<u32> {
        let path = format!("/proc/{pid}/task/{pid}/children");
        if let Ok(data) = std::fs::read_to_string(&path) {
            return data
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
        }
        // Fallback: scan /proc for matching ppid (slower, rare).
        let mut kids = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return kids;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(child) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{child}/stat")) else {
                continue;
            };
            let Some(close) = stat.rfind(')') else {
                continue;
            };
            let rest = &stat[close + 2..];
            let mut fields = rest.split_whitespace();
            let _state = fields.next();
            if fields.next().and_then(|s| s.parse::<u32>().ok()) == Some(pid) {
                kids.push(child);
            }
        }
        kids
    }

    fn start_time_from_proc(pid: u32) -> Option<SystemTime> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // field 22 is starttime (clock ticks since boot)
        let close = stat.rfind(')')?;
        let rest = stat.get(close + 2..)?;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let start_ticks: u64 = fields.get(19)?.parse().ok()?;
        // CLK_TCK is usually 100 on Linux; probe /proc to confirm we are on a Linux host.
        let clk_tck = std::fs::read_to_string("/proc/self/stat")
            .ok()
            .map(|_| 100u64)
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

    pub(super) fn command_line_via_ps(pid: u32) -> Option<String> {
        crate::daemon::metrics::record_ps_call();
        let output = Command::new("/bin/ps")
            .args(["-www", "-p", &pid.to_string(), "-o", "command="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let cmd = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if cmd.is_empty() {
            None
        } else {
            Some(cmd)
        }
    }

    pub(super) fn child_pids_via_pgrep(pid: u32) -> Vec<u32> {
        let output = Command::new("pgrep")
            .args(["-P", &pid.to_string()])
            .output();
        let Ok(output) = output else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect()
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

    #[test]
    fn command_line_for_current_process_is_available() {
        let pid = std::process::id();
        let cmd = command_line_for_pid(pid);
        assert!(cmd.is_some(), "expected argv for pid {pid}");
        let cmd = cmd.unwrap();
        assert!(!cmd.trim().is_empty());
    }

    #[test]
    fn foreground_command_resolves_python_child_script() {
        clear_cwd_cache();
        let script =
            std::env::temp_dir().join(format!("sessions-fg-cmd-{}.py", std::process::id()));
        std::fs::write(&script, "import time\ntime.sleep(30)\n").unwrap();
        let mut child = std::process::Command::new("python3")
            .arg(&script)
            .spawn()
            .expect("spawn python3");
        let child_pid = child.id();
        // Parent is this test process — treat as pane shell.
        let pane_pid = std::process::id();
        // Give the child a moment to exec.
        std::thread::sleep(std::time::Duration::from_millis(150));
        let resolved = foreground_command_for_pane(pane_pid, "python3")
            .or_else(|| foreground_command_for_pane(pane_pid, "Python"))
            .or_else(|| command_line_for_pid(child_pid));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&script);
        let resolved = resolved.expect("resolved python command line");
        assert!(
            resolved.contains(script.file_name().unwrap().to_str().unwrap())
                || resolved.contains(script.to_str().unwrap()),
            "expected script path in `{resolved}`"
        );
    }

    #[test]
    fn comm_matches_versioned_agent_binaries() {
        // tmux truncates long process names; argv0 is often the bare agent name.
        assert!(comm_matches("grok", "grok-0.2.106-ma"));
        assert!(comm_matches("grok-0.2.106-ma", "grok"));
        assert!(comm_matches("codex", "codex-aarch64-a"));
        assert!(comm_matches("python", "python3"));
        assert!(comm_matches("python3", "python3.13"));
        // Unrelated binaries must not match via a shared short prefix.
        assert!(!comm_matches("go", "google-calendar-mcp"));
        assert!(!comm_matches("code", "code-helper-tool-xyz"));
    }

    #[test]
    fn language_launcher_comm_detects_interpreters_only() {
        assert!(is_language_launcher_comm("python3"));
        assert!(is_language_launcher_comm("Python"));
        assert!(is_language_launcher_comm("node"));
        assert!(!is_language_launcher_comm("grok"));
        assert!(!is_language_launcher_comm("grok-0.2.106-ma"));
        assert!(!is_language_launcher_comm("htop"));
    }
}
