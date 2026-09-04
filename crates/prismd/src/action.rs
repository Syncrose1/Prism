//! Carrying out interventions.
//!
//! Every signal goes through [`prism_core::safety::SafetyGuard`], which is the
//! only way to obtain a signalable pid. See that module for why: on 2026-09-04
//! an unguarded cast in this file terminated the operator's entire graphical
//! session four times.

use prism_core::safety::SafetyGuard;
use std::sync::LazyLock;
use std::time::Duration;
use tracing::{info, warn};

/// One process-wide guard. Constructed once so `self_pid` is captured before
/// any caller can influence it.
static GUARD: LazyLock<SafetyGuard> = LazyLock::new(SafetyGuard::default);

/// Terminate a set of processes: SIGTERM, a short grace period, then SIGKILL
/// for whatever remains.
///
/// Returns the pids confirmed gone. Processes that exit on their own between
/// the signal and the check count as successes — the goal is that they are no
/// longer running, not that Prism personally killed them.
pub fn terminate(pids: &[u32], grace: Duration) -> Vec<u32> {
    for &pid in pids {
        signal(pid, libc::SIGTERM);
    }

    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if pids.iter().all(|&p| !alive(p)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let stubborn: Vec<u32> = pids.iter().copied().filter(|&p| alive(p)).collect();
    if !stubborn.is_empty() {
        warn!(?stubborn, "ignored SIGTERM, escalating to SIGKILL");
        for &pid in &stubborn {
            signal(pid, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    pids.iter().copied().filter(|&p| !alive(p)).collect()
}

/// Send a signal, ignoring ESRCH — a process that has already exited is the
/// outcome we wanted.
fn signal(pid: u32, sig: i32) {
    // The guard is the only source of a signalable pid. It rejects anything that
    // would not target exactly one ordinary process — process groups, init,
    // ourselves, our ancestors, and anything the machine's reachability depends
    // on. A caller cannot opt out, because a caller being careful is precisely
    // the assumption that failed on 2026-09-04.
    let pid = match GUARD.check(pid) {
        Ok(pid) => pid,
        Err(refusal) => {
            warn!(pid, sig, reason = %refusal.reason(), "refusing to signal");
            return;
        }
    };

    // SAFETY: `kill` takes scalars and touches no caller memory. An invalid or
    // exited pid returns ESRCH rather than causing undefined behaviour.
    let rc = unsafe { libc::kill(pid, sig) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            warn!(pid, sig, %err, "signal failed");
        }
    }
}

/// Liveness by `/proc` presence rather than `kill(pid, 0)`.
///
/// `kill(0)` cannot distinguish a live process from a zombie, and a zombie has
/// already released its memory — treating one as still running would make Prism
/// escalate to SIGKILL against a process that is, for our purposes, gone.
pub fn alive(pid: u32) -> bool {
    let Ok(state) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // The comm field may contain spaces and parentheses, so parse state from
    // after the final ')' rather than by splitting the whole line.
    match state.rsplit_once(')') {
        Some((_, rest)) => !matches!(rest.split_whitespace().next(), Some("Z") | None),
        None => false,
    }
}

/// Run a command detached, without waiting for it.
pub fn spawn_detached(argv: &[String]) {
    let Some((program, args)) = argv.split_first() else {
        warn!("empty command, nothing to run");
        return;
    };
    match std::process::Command::new(program).args(args).spawn() {
        Ok(child) => info!(pid = child.id(), cmd = %argv.join(" "), "ran remedy command"),
        Err(e) => warn!(cmd = %argv.join(" "), error = %e, "remedy command failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_is_alive() {
        assert!(alive(std::process::id()));
    }

    #[test]
    fn impossible_pid_is_not_alive() {
        assert!(!alive(u32::MAX));
    }

    /// A pid that is unused but still a *valid* pid. Deliberately not
    /// `u32::MAX`: that wraps to -1 through the `pid_t` cast in `signal` and
    /// means "every process we own". `alive()` tolerates u32::MAX because it
    /// only stats /proc, which is why the sibling test below can still use it —
    /// the asymmetry is the whole trap.
    const UNUSED_PID: u32 = 0x7FFF_FFF0;

    #[test]
    fn terminate_reports_already_dead_pids_as_gone() {
        let gone = terminate(&[UNUSED_PID], Duration::from_millis(50));
        assert_eq!(gone, vec![UNUSED_PID]);
    }

    #[test]
    fn signal_refuses_pids_that_would_wrap_to_a_group() {
        // Must not panic and must not signal anything. u32::MAX -> -1,
        // 0x8000_0000 -> i32::MIN, 0 -> our own process group.
        signal(u32::MAX, libc::SIGTERM);
        signal(0x8000_0000, libc::SIGTERM);
        signal(0, libc::SIGTERM);
        // Still alive: none of the above reached kill().
        assert!(alive(std::process::id()));
    }

    #[test]
    fn terminates_a_real_child() {
        let child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        assert!(alive(pid));

        let gone = terminate(&[pid], Duration::from_secs(2));
        assert_eq!(gone, vec![pid]);

        // Reap so the zombie does not linger for other tests.
        let mut child = child;
        let _ = child.wait();
    }
}
