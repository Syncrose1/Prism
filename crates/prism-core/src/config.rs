//! Configuration, split by portability.
//!
//! [`HostConfig`] describes *this* machine: where to bind, where state lives,
//! which directories may be browsed. It never travels.
//!
//! [`Profile`] describes intent: what to watch for, what to run, what the
//! thresholds are. It is designed to be exported from one machine and imported
//! on another, so it must contain no secrets and no host-specific absolutes it
//! cannot justify. Items that cannot apply on the destination are gated rather
//! than erroring.

use crate::gate::Gate;
use crate::watchdog::storm::{StormRule, builtin_rules};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 9000;

// ---------------------------------------------------------------------------
// Host configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostConfig {
    pub server: ServerConfig,
    pub files: FilesConfig,
    pub terminal: TerminalConfig,
}

/// Terminal sessions.
///
/// Present as host config rather than profile config because whether remote
/// shell access is acceptable is a property of the machine and its exposure,
/// not of the workload profile running on it — and it should not travel between
/// hosts in an exported profile. See ADR 0003 §4.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    /// An operator deploying to a machine where remote shell access is
    /// unacceptable turns the feature off here, rather than relying on nobody
    /// navigating to it.
    pub enabled: bool,
    /// Defaults to `$SHELL`.
    pub shell: Option<String>,
    pub scrollback_bytes: usize,
    pub max_sessions: usize,
    /// Wrap each session in a transient systemd scope. Only disable on hosts
    /// with no user manager: without it a terminal is the one workload Prism
    /// cannot contain or attribute.
    pub use_scope: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shell: None,
            scrollback_bytes: 256 * 1024,
            max_sessions: 16,
            use_scope: true,
        }
    }
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            files: FilesConfig::default(),
            terminal: TerminalConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub port: u16,
    /// Which interface to listen on.
    pub bind: BindMode,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind: BindMode::Tailscale,
        }
    }
}

/// Where the API listens.
///
/// [`BindMode::Tailscale`] is the default and resolves the host's tailnet
/// address at startup. Binding the tailnet interface rather than `0.0.0.0` means
/// the service is not reachable from the local network or the internet even if
/// authentication were misconfigured — the network boundary and the auth
/// boundary fail independently.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BindMode {
    #[default]
    Tailscale,
    Localhost,
    /// An explicit address. Prism warns loudly if this is a wildcard.
    Address(String),
}

impl BindMode {
    /// True for addresses that expose the service beyond the local host and
    /// tailnet. Used to emit a warning, not to refuse.
    pub fn is_wildcard(&self) -> bool {
        matches!(self, BindMode::Address(a) if a.starts_with("0.0.0.0") || a.starts_with("[::]"))
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilesConfig {
    /// Directories exposed to the file browser. Empty means the feature is off.
    ///
    /// An allowlist rather than a denylist: the browser can reach nothing that
    /// is not named here, so a path-traversal slip cannot escape into the wider
    /// filesystem.
    pub roots: Vec<FileRoot>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileRoot {
    pub name: String,
    pub path: PathBuf,
    /// Writes and deletes are refused unless this is explicitly true.
    #[serde(default)]
    pub writable: bool,
}

// ---------------------------------------------------------------------------
// Portable profile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub governor: GovernorConfig,
    /// Process-storm rules. Defaults to [`builtin_rules`].
    pub storm: Vec<StormRule>,
    pub facet: Vec<Facet>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "default".into(),
            governor: GovernorConfig::default(),
            storm: builtin_rules(),
            facet: Vec::new(),
        }
    }
}

/// Governor thresholds.
///
/// Expressed against honest headroom and PSI stall rather than raw free memory,
/// because both of those remain meaningful on a machine with a different amount
/// of RAM — which is what makes a profile portable at all.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GovernorConfig {
    /// Fraction of wall time stalled, 0.0..=1.0.
    pub amber_stall: f64,
    pub red_stall: f64,
    pub black_stall: f64,
    /// Honest headroom floors, in MiB.
    pub amber_headroom_mib: u64,
    pub red_headroom_mib: u64,
    pub black_headroom_mib: u64,
    /// Free-space floors for the tightest watched mount, in MiB.
    ///
    /// Absolute rather than percentage: a host writing multi-gigabyte model
    /// files can sit at a comfortable-sounding 90% of a large disk and still
    /// fail the next download. Defaults are sized for that workload.
    pub amber_disk_free_mib: u64,
    pub red_disk_free_mib: u64,
    pub black_disk_free_mib: u64,
    /// Filesystems to watch. Empty means use `sensors::disk::default_paths()`.
    pub disk_paths: Vec<PathBuf>,
    /// How long a threshold must hold before the tier changes, in seconds.
    /// Prevents a single sampling artefact from killing a workload.
    pub sustain_secs: u64,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            amber_stall: 0.05,
            red_stall: 0.20,
            black_stall: 0.50,
            amber_headroom_mib: 4096,
            red_headroom_mib: 1536,
            black_headroom_mib: 512,
            // 20 GiB / 8 GiB / 2 GiB. Generous because a single language model
            // download is routinely 10-40 GiB: warning at 2 GiB free would fire
            // long after the write that fills the disk had already begun.
            amber_disk_free_mib: 20480,
            red_disk_free_mib: 8192,
            black_disk_free_mib: 2048,
            disk_paths: Vec::new(),
            sustain_secs: 10,
        }
    }
}

/// A workload Prism owns: it can start it, stop it, and constrain it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Facet {
    pub id: String,
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub limits: FacetLimits,
    #[serde(default)]
    pub enabled_if: Gate,
}

/// Resource ceilings, all optional.
///
/// Deliberately unset by default. The operator here routinely runs workloads
/// that consume nearly the whole machine, and a ceiling that truncates a
/// legitimate job is a worse outcome than the job running slowly. `swap_max` is
/// the exception worth setting: the failure mode being defended against is a
/// swap runaway, so capping swap contains the spiral without constraining RAM
/// use at all.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FacetLimits {
    pub memory_high: Option<String>,
    pub memory_max: Option<String>,
    pub swap_max: Option<String>,
}

// ---------------------------------------------------------------------------
// Paths and loading
// ---------------------------------------------------------------------------

pub fn config_dir() -> PathBuf {
    std::env::var_os("PRISM_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(|d| PathBuf::from(d).join("prism")))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/prism")))
        .unwrap_or_else(|| PathBuf::from("/etc/prism"))
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("PRISM_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_STATE_HOME").map(|d| PathBuf::from(d).join("prism")))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state/prism")))
        .unwrap_or_else(|| PathBuf::from("/var/lib/prism"))
}

/// Load a TOML file, returning the default value if it does not exist.
///
/// A missing config is a first run, not an error. A *malformed* config is an
/// error and is reported rather than silently replaced with defaults — quietly
/// ignoring a typo'd threshold would mean running unprotected while appearing
/// configured.
pub fn load_or_default<T>(path: &Path) -> anyhow::Result<T>
where
    T: Default + for<'de> Deserialize<'de>,
{
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(anyhow::anyhow!("{}: {e}", path.display())),
    }
}

pub fn save<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(value)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_9000() {
        assert_eq!(ServerConfig::default().port, 9000);
    }

    #[test]
    fn default_bind_is_tailnet_not_wildcard() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.bind, BindMode::Tailscale);
        assert!(!cfg.bind.is_wildcard());
    }

    #[test]
    fn wildcard_addresses_are_flagged() {
        assert!(BindMode::Address("0.0.0.0:9000".into()).is_wildcard());
        assert!(BindMode::Address("[::]:9000".into()).is_wildcard());
        assert!(!BindMode::Address("127.0.0.1:9000".into()).is_wildcard());
    }

    #[test]
    fn file_roots_are_read_only_by_default() {
        let root: FileRoot = toml::from_str(r#"name = "home"
path = "/home/x""#)
            .unwrap();
        assert!(!root.writable);
    }

    #[test]
    fn files_disabled_when_no_roots_configured() {
        assert!(FilesConfig::default().roots.is_empty());
    }

    #[test]
    fn profile_ships_with_builtin_storm_rules() {
        assert!(!Profile::default().storm.is_empty());
    }

    #[test]
    fn governor_thresholds_are_ordered() {
        let g = GovernorConfig::default();
        assert!(g.amber_stall < g.red_stall && g.red_stall < g.black_stall);
        assert!(g.amber_headroom_mib > g.red_headroom_mib);
        assert!(g.red_headroom_mib > g.black_headroom_mib);
    }

    #[test]
    fn facet_limits_are_unset_by_default() {
        let l = FacetLimits::default();
        assert!(l.memory_high.is_none() && l.memory_max.is_none() && l.swap_max.is_none());
    }

    #[test]
    fn profile_survives_a_round_trip() {
        let original = Profile::default();
        let text = toml::to_string_pretty(&original).unwrap();
        let parsed: Profile = toml::from_str(&text).unwrap();
        assert_eq!(parsed.storm.len(), original.storm.len());
        assert_eq!(parsed.name, original.name);
    }

    #[test]
    fn missing_file_yields_defaults() {
        let cfg: HostConfig = load_or_default(Path::new("/nonexistent/prism.toml")).unwrap();
        assert_eq!(cfg.server.port, DEFAULT_PORT);
    }

    #[test]
    fn malformed_config_is_an_error_not_a_silent_default() {
        let dir = std::env::temp_dir().join(format!("prism-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, "server = { port = \"not a number\" }").unwrap();
        assert!(load_or_default::<HostConfig>(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
