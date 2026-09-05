//! Prism core: sensing, policy, and portable configuration.
//!
//! Nothing in this crate is specific to the machine it was written on. Host
//! specifics live in [`config::HostConfig`]; everything intended to travel
//! between machines lives in [`config::Profile`] and is guarded by
//! [`gate::Gate`] so it degrades rather than breaks on an unfamiliar host.

pub mod auth;
pub mod config;
pub mod events;
pub mod files;
pub mod gate;
pub mod governor;
pub mod platform;
pub mod safety;
pub mod sensors;
pub mod supervisor;
pub mod term;
pub mod watchdog;

#[cfg(not(target_os = "linux"))]
compile_error!(
    "Prism currently supports Linux only. Its resilience layer rests on cgroups \
     v2, PSI and zram accounting, none of which have direct equivalents \
     elsewhere — see crates/prism-core/src/platform/mod.rs for the seams a port \
     would implement."
);

pub use gate::Gate;
