//! The Prism daemon.
//!
//! Phase 1 scope: sense, detect, and intervene. No network surface yet.
//!
//! The loop is deliberately synchronous and allocation-light. Prism has to keep
//! running during the exact memory exhaustion it exists to resolve, so the
//! design constraint throughout is that nothing here should need to allocate,
//! page in, or wait on a runtime at the moment it matters most.

mod action;
mod monitor;

use anyhow::Context as _;
use prism_core::config::{self, HostConfig, Profile};
use tracing::{info, warn};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PRISM_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let dir = config::config_dir();
    let host: HostConfig = config::load_or_default(&dir.join("prism.toml"))
        .context("loading host config")?;
    let profile: Profile = config::load_or_default(&dir.join("profile.toml"))
        .context("loading profile")?;

    info!(
        profile = %profile.name,
        config = %dir.display(),
        port = host.server.port,
        "prism starting"
    );

    if host.server.bind.is_wildcard() {
        warn!(
            "server.bind is a wildcard address: Prism will be reachable beyond \
             the tailnet. This is not the intended deployment."
        );
    }

    lock_memory();

    let mut monitor = monitor::Monitor::new(profile);
    monitor.run()
}

/// Pin the daemon's pages so it cannot be swapped out.
///
/// Without this, Prism is one of the first things evicted under the pressure it
/// is supposed to resolve — it would be paged out precisely when it needs to
/// act, which is how a watchdog becomes another casualty rather than a rescue.
///
/// Failure is not fatal: the daemon is still useful unlocked, and on a machine
/// with a restrictive `RLIMIT_MEMLOCK` refusing to start would be worse than
/// running slightly less reliably.
fn lock_memory() {
    // SAFETY: `mlockall` takes only a flags bitmask and touches no caller
    // memory. The call is total; it either succeeds or reports errno.
    let rc = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
    if rc == 0 {
        info!("memory locked (mlockall): daemon is unswappable");
    } else {
        let err = std::io::Error::last_os_error();
        warn!(
            %err,
            "could not lock memory; prism may be paged out under pressure. \
             Raise LimitMEMLOCK in the service unit to fix."
        );
    }
}
