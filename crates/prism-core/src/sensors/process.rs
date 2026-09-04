//! Lightweight `/proc` scanning.
//!
//! Deliberately hand-rolled rather than pulling in a procfs crate: the scan runs
//! at 1 Hz over every process on the system, and Prism's whole value proposition
//! is being cheap and dependency-light enough to stay responsive while the
//! machine is starved.
//!
//! Resident size is read from `VmRSS` in `/proc/<pid>/status` (already in kB)
//! only for processes that matched a pattern, which avoids both the cost of
//! parsing `status` for every process and any assumption about page size.

/// A process as seen by a single scan. Intentionally minimal.
#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub cmdline: String,
}

/// Visit every process, invoking `f` with its pid and full command line.
///
/// Processes that exit mid-scan are skipped rather than treated as errors —
/// that race is routine, not exceptional.
pub fn for_each<F: FnMut(u32, &str)>(mut f: F) {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
            continue; // exited between readdir and open
        };
        if raw.is_empty() {
            continue; // kernel thread
        }
        f(pid, &render_cmdline(&raw));
    }
}

/// Collect every process whose command line satisfies `pred`.
pub fn find<F: Fn(&str) -> bool>(pred: F) -> Vec<ProcInfo> {
    let mut out = Vec::new();
    for_each(|pid, cmdline| {
        if pred(cmdline) {
            out.push(ProcInfo {
                pid,
                cmdline: cmdline.to_string(),
            });
        }
    });
    out
}

/// `/proc/<pid>/cmdline` is NUL-separated and usually NUL-terminated.
fn render_cmdline(raw: &[u8]) -> String {
    let trimmed = raw.strip_suffix(&[0]).unwrap_or(raw);
    let mut s = String::with_capacity(trimmed.len());
    for byte in trimmed {
        s.push(if *byte == 0 { ' ' } else { *byte as char });
    }
    s
}

/// Resident set size in kB, or `None` if the process has exited.
pub fn rss_kb(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

/// Total resident kB across the given pids, ignoring any that have exited.
pub fn total_rss_kb(pids: impl IntoIterator<Item = u32>) -> u64 {
    pids.into_iter().filter_map(rss_kb).sum()
}

/// A process with its resident size, for "what is eating the machine?".
#[derive(Debug, Clone)]
pub struct RankedProc {
    pub pid: u32,
    pub comm: String,
    pub cmdline: String,
    pub rss_kb: u64,
}

/// The `n` largest processes by resident size, descending.
///
/// Reads `VmRSS` for every process, which is markedly more expensive than the
/// cmdline-only scan used by the storm detector. That is acceptable because this
/// runs on demand for the rescue page rather than every tick — but it is the
/// reason the two paths are separate rather than one shared scan.
pub fn top_by_rss(n: usize) -> Vec<RankedProc> {
    let mut all: Vec<RankedProc> = Vec::new();
    for_each(|pid, cmdline| {
        let Some(rss_kb) = rss_kb(pid) else { return };
        // Kernel threads and trivial processes are noise on a page whose job is
        // identifying what to kill.
        if rss_kb < 1024 {
            return;
        }
        all.push(RankedProc {
            pid,
            comm: comm(pid).unwrap_or_default(),
            cmdline: cmdline.to_string(),
            rss_kb,
        });
    });
    all.sort_unstable_by(|a, b| b.rss_kb.cmp(&a.rss_kb));
    all.truncate(n);
    all
}

pub fn comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_nul_separated_cmdline() {
        assert_eq!(render_cmdline(b"qs\0-p\0/path/killDialog.qml\0"), "qs -p /path/killDialog.qml");
    }

    #[test]
    fn renders_cmdline_without_trailing_nul() {
        assert_eq!(render_cmdline(b"sleep\010"), "sleep 10");
    }

    #[test]
    fn single_argument_survives() {
        assert_eq!(render_cmdline(b"init\0"), "init");
    }

    #[test]
    fn scan_sees_this_test_process() {
        let me = std::process::id();
        let mut found = false;
        for_each(|pid, _| {
            if pid == me {
                found = true;
            }
        });
        assert!(found, "scan should observe the running test process");
    }

    #[test]
    fn rss_of_self_is_nonzero() {
        assert!(rss_kb(std::process::id()).unwrap_or(0) > 0);
    }

    #[test]
    fn rss_of_impossible_pid_is_none() {
        assert!(rss_kb(u32::MAX).is_none());
    }
}
