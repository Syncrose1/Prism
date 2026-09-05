//! The platform boundary.
//!
//! Prism is Linux-only today, and honestly so: its resilience story rests on
//! kernel interfaces that have no direct equivalent elsewhere.
//!
//! | Prism relies on | Purpose | Elsewhere |
//! |---|---|---|
//! | cgroups v2 | containment, live limits, atomic whole-tree kill | Windows Job Objects — similar idea, different semantics |
//! | PSI (`/proc/pressure/memory`) | stall as a *fraction of time*, not a counter | no equivalent; only coarse counters |
//! | zram `mm_stat` | the honest-headroom correction | no equivalent |
//! | `forkpty` | real terminals | ConPTY |
//! | systemd user scopes | supervision without root | Windows Services |
//!
//! This module exists so that a second platform would be *additive* rather than
//! a rewrite. The traits below are the seams: everything above them —
//! the API, auth, shell, Files, Gallery, Timeline, the governor's policy — is
//! already portable and does not name a Linux interface anywhere.
//!
//! The traits are deliberately narrow. A wider abstraction would be guesswork
//! about an implementation nobody has written, and the wrong shape is worse
//! than none: it would make the port harder while looking like preparation.
//!
//! What a port would *not* get for free is stated plainly rather than glossed:
//! **honest headroom has no Windows equivalent**, because it corrects for
//! compressed swap held in RAM. A Windows backend would have a weaker view of
//! memory pressure, and that limitation belongs in the design rather than being
//! discovered later.

use crate::config::FacetLimits;

/// Starting, stopping and constraining a workload.
///
/// Implemented on Linux by [`crate::supervisor::Supervisor`] over systemd
/// transient units. A Windows backend would sit on Job Objects, which offer
/// memory limits and whole-tree termination but no equivalent of
/// `MemorySwapMax` — the one limit that matters most here, since the failure
/// being defended against is a swap runaway.
pub trait ProcessSupervisor {
    type Error;

    fn start(&self, facet: &crate::config::Facet) -> Result<(), Self::Error>;

    /// Ask politely, allowing the workload to shut itself down.
    fn stop(&self, id: &str) -> Result<(), Self::Error>;

    /// Terminate the whole tree at once.
    ///
    /// Atomicity is the requirement, not speed: a shell that launched a
    /// workload that forked workers must die in one operation, with no pid
    /// races and nothing orphaned.
    fn kill(&self, id: &str) -> Result<(), Self::Error>;

    /// Change limits on a *running* workload, without restarting it.
    fn set_limits(&self, id: &str, limits: &FacetLimits) -> Result<(), Self::Error>;

    fn status(&self, id: &str) -> crate::supervisor::FacetStatus;

    /// Current charge in kB, or `None` when not running.
    fn memory_current_kb(&self, id: &str) -> Option<u64>;
    fn memory_swap_kb(&self, id: &str) -> Option<u64>;
}

/// Whether the machine is actually struggling.
///
/// The distinction that matters: PSI reports *stall as a fraction of wall
/// time*, which is a measure of harm. Free-memory counters report a level,
/// which is not — a machine can be nearly out of memory and perfectly happy, or
/// have gigabytes free and be thrashing. Any port must answer the harm
/// question, however it does so.
pub trait PressureSensor {
    /// Fraction of wall time fully stalled on memory, 0.0..=1.0.
    fn stall_full(&mut self) -> f64;

    /// Memory genuinely available, corrected for anything that only appears to
    /// be free. On Linux this subtracts the RAM cost of compressed swap; see
    /// [`crate::sensors::memory`].
    fn honest_headroom_kb(&self) -> u64;
}

/// A real terminal, not captured output.
///
/// The requirement is that an interactive program cannot tell the difference —
/// prompts must be answerable, and full-screen programs must work. Implemented
/// on Linux by [`crate::term::pty::Pty`]; a Windows backend would use ConPTY.
pub trait TerminalBackend: Send + Sync {
    fn read(&self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write(&self, buf: &[u8]) -> std::io::Result<usize>;
    fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()>;
    /// The exit status, once the child has been reaped.
    fn try_wait(&self) -> Option<i32>;
}

// ── Linux implementations ────────────────────────────────────────────────

impl ProcessSupervisor for crate::supervisor::Supervisor {
    type Error = anyhow::Error;

    fn start(&self, facet: &crate::config::Facet) -> anyhow::Result<()> {
        crate::supervisor::Supervisor::start(self, facet)
    }
    fn stop(&self, id: &str) -> anyhow::Result<()> {
        crate::supervisor::Supervisor::stop(self, id)
    }
    fn kill(&self, id: &str) -> anyhow::Result<()> {
        crate::supervisor::Supervisor::kill(self, id)
    }
    fn set_limits(&self, id: &str, limits: &FacetLimits) -> anyhow::Result<()> {
        crate::supervisor::Supervisor::set_limits(self, id, limits)
    }
    fn status(&self, id: &str) -> crate::supervisor::FacetStatus {
        crate::supervisor::Supervisor::status(self, id)
    }
    fn memory_current_kb(&self, id: &str) -> Option<u64> {
        crate::supervisor::Supervisor::memory_current_kb(self, id)
    }
    fn memory_swap_kb(&self, id: &str) -> Option<u64> {
        crate::supervisor::Supervisor::memory_swap_kb(self, id)
    }
}

impl TerminalBackend for crate::term::pty::Pty {
    fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        crate::term::pty::Pty::read(self, buf)
    }
    fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        crate::term::pty::Pty::write(self, buf)
    }
    fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()> {
        crate::term::pty::Pty::resize(self, crate::term::pty::WinSize { rows, cols })
    }
    fn try_wait(&self) -> Option<i32> {
        crate::term::pty::Pty::try_wait(self)
    }
}

/// PSI plus the honest-headroom correction.
#[derive(Default)]
pub struct LinuxPressure {
    psi: crate::sensors::memory::PsiTracker,
}

impl LinuxPressure {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PressureSensor for LinuxPressure {
    fn stall_full(&mut self) -> f64 {
        crate::sensors::memory::sample_psi()
            .and_then(|raw| self.psi.update(raw))
            .map(|s| s.full)
            .unwrap_or(0.0)
    }

    fn honest_headroom_kb(&self) -> u64 {
        crate::sensors::memory::sample()
            .map(|m| m.honest_headroom_kb)
            .unwrap_or(0)
    }
}

/// What this platform can actually do, for the UI and for diagnostics.
///
/// Reported rather than assumed so a future port degrades visibly instead of
/// silently pretending to contain workloads it cannot.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Capabilities {
    pub name: &'static str,
    /// Whole-tree containment with live, adjustable limits.
    pub cgroups: bool,
    /// Stall-based pressure rather than a free-memory level.
    pub psi: bool,
    /// The honest-headroom correction for compressed swap.
    pub compressed_swap_accounting: bool,
    pub pty: bool,
    /// Supervision without root.
    pub user_services: bool,
}

pub fn capabilities() -> Capabilities {
    Capabilities {
        name: "linux",
        cgroups: std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists(),
        psi: std::path::Path::new("/proc/pressure/memory").exists(),
        compressed_swap_accounting: std::path::Path::new("/sys/block/zram0/mm_stat").exists(),
        pty: true,
        user_services: std::env::var_os("XDG_RUNTIME_DIR").is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_host_reports_its_capabilities() {
        let c = capabilities();
        assert_eq!(c.name, "linux");
        assert!(c.pty);
    }

    #[test]
    fn capabilities_are_detected_rather_than_assumed() {
        // A container without cgroup delegation, or a kernel without PSI, must
        // report so rather than have Prism act as if containment worked.
        let c = capabilities();
        assert_eq!(
            c.cgroups,
            std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
        );
        assert_eq!(c.psi, std::path::Path::new("/proc/pressure/memory").exists());
    }

    #[test]
    fn the_supervisor_satisfies_the_trait() {
        // The point of the seam: a second implementation is additive.
        fn assert_impl<T: ProcessSupervisor>(_: &T) {}
        assert_impl(&crate::supervisor::Supervisor::new());
    }

    #[test]
    fn the_pressure_sensor_reads_this_machine() {
        let mut p = LinuxPressure::new();
        let _ = p.stall_full(); // first sample establishes a baseline
        assert!(
            p.honest_headroom_kb() > 0,
            "a running machine has some headroom"
        );
    }

    #[test]
    fn stall_is_a_fraction_not_a_counter() {
        let mut p = LinuxPressure::new();
        let s = p.stall_full();
        assert!(
            (0.0..=1.0).contains(&s),
            "stall must be a fraction of wall time, got {s}"
        );
    }
}
