//! Capability gates.
//!
//! Prism runs on machines that differ: a Hyprland desktop, a headless inference
//! box, a laptop with no GPU. Rather than detecting "which machine am I", every
//! watchdog and facet declares the capabilities it needs. Anything whose subject
//! is absent self-disables silently, so a profile authored on one host can be
//! dropped onto another and degrade gracefully instead of erroring.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Conditions that must *all* hold for the owning item to activate.
///
/// An empty gate is always satisfied, so `enabled_if` can be omitted entirely
/// for portable items.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    /// An executable of this name must exist on `PATH`.
    pub binary: Option<String>,
    /// This path must exist.
    pub path: Option<PathBuf>,
    /// A process with this exact name must be running.
    pub process: Option<String>,
    /// Explicit override; `false` disables regardless of everything else.
    pub enabled: Option<bool>,
}

impl Gate {
    pub fn satisfied(&self) -> bool {
        self.evaluate().is_satisfied()
    }

    /// Evaluate the gate, retaining *why* it failed so the daemon can log a
    /// useful reason rather than silently doing nothing.
    pub fn evaluate(&self) -> GateOutcome {
        if self.enabled == Some(false) {
            return GateOutcome::Blocked("explicitly disabled".into());
        }
        if let Some(bin) = &self.binary
            && !binary_on_path(bin)
        {
            return GateOutcome::Blocked(format!("binary `{bin}` not on PATH"));
        }
        if let Some(path) = &self.path
            && !path.exists()
        {
            return GateOutcome::Blocked(format!("path `{}` absent", path.display()));
        }
        if let Some(proc_name) = &self.process
            && !process_running(proc_name)
        {
            return GateOutcome::Blocked(format!("process `{proc_name}` not running"));
        }
        GateOutcome::Satisfied
    }
}

#[derive(Debug, Clone)]
pub enum GateOutcome {
    Satisfied,
    Blocked(String),
}

impl GateOutcome {
    pub fn is_satisfied(&self) -> bool {
        matches!(self, GateOutcome::Satisfied)
    }
}

fn binary_on_path(name: &str) -> bool {
    // An absolute or relative path is used as-is rather than searched for.
    if name.contains('/') {
        return Path::new(name).is_file();
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .any(|dir| Path::new(dir).join(name).is_file())
}

/// True if any process has exactly this `comm` name.
///
/// Reads `/proc/*/comm` rather than shelling out to `pidof`, both to avoid the
/// fork cost on every gate evaluation and to keep Prism dependency-free.
fn process_running(name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(pid) = file_name.to_str() else {
            continue;
        };
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm"))
            && comm.trim() == name
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_gate_is_satisfied() {
        assert!(Gate::default().satisfied());
    }

    #[test]
    fn explicit_disable_wins() {
        let gate = Gate {
            enabled: Some(false),
            ..Default::default()
        };
        assert!(!gate.satisfied());
    }

    #[test]
    fn missing_binary_blocks() {
        let gate = Gate {
            binary: Some("definitely-not-a-real-binary-xyzzy".into()),
            ..Default::default()
        };
        assert!(!gate.satisfied());
    }

    #[test]
    fn present_path_passes() {
        let gate = Gate {
            path: Some("/proc/meminfo".into()),
            ..Default::default()
        };
        assert!(gate.satisfied());
    }

    #[test]
    fn absent_path_blocks() {
        let gate = Gate {
            path: Some("/nonexistent/xyzzy".into()),
            ..Default::default()
        };
        assert!(!gate.satisfied());
    }
}
