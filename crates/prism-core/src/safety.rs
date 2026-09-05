//! Blast-radius bounds for anything that terminates a process.
//!
//! Prism's first stated goal is that the machine never becomes unreachable. A
//! daemon whose job is killing processes is therefore one bad pid away from
//! being the cause of the outage it exists to prevent.
//!
//! On 2026-09-04 this was not hypothetical. A unit test passed `u32::MAX` as
//! "a pid that cannot exist"; it wrapped to `-1` through the `pid_t` cast, and
//! `kill(-1, SIGTERM)` terminated every process owned by the operator — the
//! compositor, the shell, the stream, the lot — four times, while the machine
//! was being operated remotely from another city.
//!
//! The lesson taken here is that a guard on the *caller* is not enough. Every
//! pid is validated at the point of signalling, against rules that do not depend
//! on any caller having been careful:
//!
//! 1. it must be a single, positive, real pid — never a process group;
//! 2. it must not be init, ourselves, or anything we descend from;
//! 3. it must not be a process the machine's reachability depends on.
//!
//! Rule 3 is deliberately conservative. Refusing to kill something Prism should
//! have killed costs a warning; killing `sshd` from 60 miles away costs the
//! weekend.

use std::collections::HashSet;

/// Process names that must never be signalled, whatever a caller asks.
///
/// These are the processes that keep the machine reachable and recoverable. The
/// list is intentionally about *reachability*, not importance: a database being
/// killed is a bad day, but `tailscaled` being killed means nobody can log in to
/// notice.
pub const DEFAULT_PROTECTED: &[&str] = &[
    // Init and service management — killing these ends everything.
    "systemd",
    "init",
    // Remote access. The whole point of the exercise.
    "sshd",
    "tailscaled",
    "tailscale",
    // Display stack. Losing this severs Sunshine/Moonlight and any GUI path.
    "Hyprland",
    "sway",
    "gnome-shell",
    "kwin_wayland",
    "Xorg",
    "sddm",
    "gdm",
    "greetd",
    // Streaming host.
    "sunshine",
    // Prism itself.
    "prismd",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Would not have targeted a single process.
    NotASinglePid,
    /// pid 1.
    Init,
    /// Prism's own process.
    SelfPid,
    /// A process Prism descends from — killing it takes Prism with it.
    Ancestor,
    /// Matched the protected-name list.
    Protected(String),
}

impl Refusal {
    pub fn reason(&self) -> String {
        match self {
            Refusal::NotASinglePid => {
                "not a single positive pid (would signal a process group)".into()
            }
            Refusal::Init => "pid 1".into(),
            Refusal::SelfPid => "prism's own process".into(),
            Refusal::Ancestor => "an ancestor of prism".into(),
            Refusal::Protected(name) => format!("`{name}` is protected"),
        }
    }
}

pub struct SafetyGuard {
    protected: HashSet<String>,
    self_pid: u32,
}

impl Default for SafetyGuard {
    fn default() -> Self {
        Self::new(DEFAULT_PROTECTED.iter().map(|s| s.to_string()))
    }
}

impl SafetyGuard {
    pub fn new(protected: impl IntoIterator<Item = String>) -> Self {
        Self {
            protected: protected.into_iter().collect(),
            self_pid: std::process::id(),
        }
    }

    /// Add extra protected names, e.g. from operator config.
    pub fn protect(&mut self, name: impl Into<String>) {
        self.protected.insert(name.into());
    }

    pub fn is_protected_name(&self, comm: &str) -> bool {
        self.protected.contains(comm)
    }

    /// `Ok(pid)` if this pid may be signalled, `Err(reason)` otherwise.
    ///
    /// Returns the validated `i32` so callers cannot re-derive it incorrectly:
    /// the only way to obtain a signalable pid is to pass this check.
    pub fn check(&self, pid: u32) -> Result<i32, Refusal> {
        // A pid that does not survive the cast as a positive value is not one
        // process. 0 targets our own group, -1 targets everything we own, and
        // anything below -1 targets a group. This is the check whose absence
        // caused the 2026-09-04 outages.
        let pid_i32 = match i32::try_from(pid) {
            Ok(p) if p > 0 => p,
            _ => return Err(Refusal::NotASinglePid),
        };

        if pid_i32 == 1 {
            return Err(Refusal::Init);
        }
        if pid == self.self_pid {
            return Err(Refusal::SelfPid);
        }
        if self.is_ancestor(pid) {
            return Err(Refusal::Ancestor);
        }
        if let Some(comm) = comm_of(pid)
            && self.is_protected_name(&comm)
        {
            return Err(Refusal::Protected(comm));
        }

        Ok(pid_i32)
    }

    /// Walk our own parent chain looking for `pid`.
    ///
    /// Killing an ancestor kills Prism, and on 2026-09-04 the ancestor chain ran
    /// through `cargo` to the terminal to the compositor — so signalling "upward"
    /// is exactly how a local mistake becomes a session-wide one.
    fn is_ancestor(&self, pid: u32) -> bool {
        let mut current = self.self_pid;
        // Bounded: pid chains are shallow, and a cycle would otherwise hang the
        // daemon inside a safety check.
        for _ in 0..64 {
            if current == pid {
                return true;
            }
            match parent_of(current) {
                Some(parent) if parent != current && parent != 0 => current = parent,
                _ => return false,
            }
        }
        false
    }
}

pub fn comm_of(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn parent_of(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("PPid:")?.trim().parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unused but structurally valid. Deliberately *not* `u32::MAX`, which is
    /// the value that caused the incident this module exists to prevent.
    const UNUSED_PID: u32 = 0x7FFF_FFF0;

    #[test]
    fn rejects_the_value_that_caused_the_outage() {
        let guard = SafetyGuard::default();
        assert_eq!(guard.check(u32::MAX), Err(Refusal::NotASinglePid));
    }

    #[test]
    fn rejects_every_non_single_pid_encoding() {
        let guard = SafetyGuard::default();
        // 0 -> our own process group.
        assert_eq!(guard.check(0), Err(Refusal::NotASinglePid));
        // Anything above i32::MAX wraps negative -> a process group.
        assert_eq!(guard.check(0x8000_0000), Err(Refusal::NotASinglePid));
        assert_eq!(guard.check(0xFFFF_FFFE), Err(Refusal::NotASinglePid));
    }

    #[test]
    fn no_input_can_ever_yield_a_non_positive_pid() {
        // Property: whatever a caller passes, an accepted result is always a
        // single real process. This is the invariant the incident violated.
        let guard = SafetyGuard::default();
        for candidate in [
            0u32,
            1,
            2,
            i32::MAX as u32,
            i32::MAX as u32 + 1,
            u32::MAX,
            u32::MAX - 1,
            0x8000_0000,
            UNUSED_PID,
        ] {
            if let Ok(pid) = guard.check(candidate) {
                assert!(pid > 0, "check() returned non-positive pid {pid}");
            }
        }
    }

    #[test]
    fn refuses_init() {
        assert_eq!(SafetyGuard::default().check(1), Err(Refusal::Init));
    }

    #[test]
    fn refuses_own_pid() {
        let guard = SafetyGuard::default();
        assert_eq!(guard.check(std::process::id()), Err(Refusal::SelfPid));
    }

    #[test]
    fn refuses_ancestors() {
        // The test runner's parent is an ancestor of this process.
        let parent = parent_of(std::process::id()).expect("have a parent");
        if parent > 1 {
            assert_eq!(SafetyGuard::default().check(parent), Err(Refusal::Ancestor));
        }
    }

    #[test]
    fn refuses_protected_names() {
        let mut guard = SafetyGuard::new(Vec::new());
        // Protect whatever this test process is actually called, then confirm a
        // live pid with that name is refused by name rather than by any other
        // rule. Uses a spawned child so it is not caught by the self/ancestor
        // rules first.
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn");
        let pid = child.id();
        guard.protect("sleep");

        // Between fork and exec the child still carries the parent's comm, so
        // asserting immediately is a race that only shows up under load.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while comm_of(pid).as_deref() != Some("sleep") && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(
            guard.check(pid),
            Err(Refusal::Protected("sleep".into())),
            "a protected name must be refused"
        );

        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn allows_an_ordinary_unrelated_process() {
        let guard = SafetyGuard::new(Vec::new());
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn");
        let pid = child.id();
        assert_eq!(guard.check(pid), Ok(pid as i32));

        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn default_guard_protects_reachability_critical_names() {
        let guard = SafetyGuard::default();
        for name in ["sshd", "tailscaled", "Hyprland", "systemd", "prismd"] {
            assert!(
                guard.is_protected_name(name),
                "{name} must be protected: losing it costs remote access"
            );
        }
    }

    #[test]
    fn unused_pid_is_permitted_and_positive() {
        let guard = SafetyGuard::default();
        // Not running, but structurally valid — must not be refused as a group.
        assert!(matches!(guard.check(UNUSED_PID), Ok(p) if p > 0));
    }
}
