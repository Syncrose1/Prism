//! Terminal sessions.
//!
//! A real PTY per session, wrapped in a systemd scope so an interactive shell
//! is bounded by the same cgroup machinery as every other workload. See
//! ADR 0003.

pub mod pty;
pub mod ring;
pub mod session;
