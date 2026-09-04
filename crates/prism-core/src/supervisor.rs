//! Facet lifecycle via systemd transient units.
//!
//! Workloads are launched as transient *user* services (`systemd-run --user`),
//! which buys three things Prism would otherwise have to build badly:
//!
//! * **Atomic termination.** `cgroup.kill` ends the entire process tree in one
//!   write — no orphaned CUDA workers, no pid races, and critically no way for
//!   the kill to escape the cgroup. After the 2026-09-04 incident (see
//!   `architecture.md` §1.3) this is the preferred path; raw pids are the
//!   dangerous fallback.
//! * **Live limits.** `MemoryHigh`/`MemoryMax`/`MemorySwapMax` can be changed on
//!   a running workload without restarting it, which is what makes the "drag a
//!   slider from London" requirement possible at all.
//! * **Accounting.** Per-facet `memory.current` and `memory.pressure` give the
//!   governor real attribution instead of guesswork.
//!
//! No root is required: the memory controller is delegated to the user slice on
//! a modern systemd, so all of this works as an ordinary user.

use crate::config::{Facet, FacetLimits};
use std::process::Command;

/// Unit name for a facet. Stable across restarts so a crashed daemon can find
/// its own workloads again on startup.
pub fn unit_name(facet_id: &str) -> String {
    format!("prism-{facet_id}.service")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacetStatus {
    /// No transient unit exists.
    Stopped,
    Running,
    Failed(String),
}

/// Build the `systemd-run` argument vector for a facet.
///
/// Split out from execution so the command can be asserted in tests without
/// launching anything — the incident that motivated this module came from code
/// that could only be verified by running it.
pub fn launch_argv(facet: &Facet) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "systemd-run".into(),
        "--user".into(),
        format!("--unit={}", unit_name(&facet.id)),
        // Collect on failure so a crashed facet does not linger as a failed
        // unit blocking the next start with the same name.
        "--collect".into(),
        format!("--description=Prism facet: {}", facet.name),
    ];

    if let Some(cwd) = &facet.cwd {
        argv.push(format!("--working-directory={}", cwd.display()));
    }

    for property in limit_properties(&facet.limits) {
        argv.push(format!("--property={property}"));
    }

    // Accounting must be explicit; without it memory.current reads zero and the
    // governor silently attributes nothing.
    argv.push("--property=MemoryAccounting=yes".into());

    argv.push("--".into());
    argv.extend(facet.command.iter().cloned());
    argv
}

/// Render limits as systemd properties, omitting anything unset.
///
/// Unset means unlimited, deliberately. The operator routinely runs workloads
/// that consume nearly the whole machine, and a ceiling that truncates a
/// legitimate job is a worse outcome than one that runs slowly. `swap_max` is
/// the exception worth setting by default: the failure being defended against is
/// a swap runaway, so capping swap contains the spiral without constraining RAM.
pub fn limit_properties(limits: &FacetLimits) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(v) = &limits.memory_high {
        out.push(format!("MemoryHigh={v}"));
    }
    if let Some(v) = &limits.memory_max {
        out.push(format!("MemoryMax={v}"));
    }
    if let Some(v) = &limits.swap_max {
        out.push(format!("MemorySwapMax={v}"));
    }
    out
}

pub struct Supervisor;

impl Supervisor {
    pub fn new() -> Self {
        Self
    }

    pub fn start(&self, facet: &Facet) -> anyhow::Result<()> {
        let argv = launch_argv(facet);
        let (program, args) = argv.split_first().expect("argv is never empty");
        let output = Command::new(program).args(args).output()?;
        if !output.status.success() {
            anyhow::bail!(
                "systemd-run failed for `{}`: {}",
                facet.id,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Terminate a facet's entire process tree atomically.
    ///
    /// Writes `1` to the cgroup's `cgroup.kill`, which sends SIGKILL to every
    /// member in one operation. This cannot escape the cgroup, cannot race a
    /// forking child, and cannot be mis-aimed by an arithmetic error — which is
    /// exactly why it is preferred over signalling pids.
    pub fn kill(&self, facet_id: &str) -> anyhow::Result<()> {
        let Some(path) = self.cgroup_path(facet_id)? else {
            anyhow::bail!("facet `{facet_id}` has no cgroup (not running?)");
        };
        std::fs::write(path.join("cgroup.kill"), "1")
            .map_err(|e| anyhow::anyhow!("cgroup.kill for `{facet_id}`: {e}"))
    }

    /// Ask a facet to stop politely, letting it run its own shutdown.
    pub fn stop(&self, facet_id: &str) -> anyhow::Result<()> {
        run_systemctl(&["stop", &unit_name(facet_id)])
    }

    /// Change limits on a *running* facet, without restarting it.
    ///
    /// `--runtime` keeps the change transient, so a restart returns to the
    /// configured baseline rather than silently inheriting whatever was last
    /// dragged on a slider.
    pub fn set_limits(&self, facet_id: &str, limits: &FacetLimits) -> anyhow::Result<()> {
        let properties = limit_properties(limits);
        if properties.is_empty() {
            return Ok(());
        }
        let unit = unit_name(facet_id);
        let mut args = vec!["set-property", "--runtime", unit.as_str()];
        args.extend(properties.iter().map(|s| s.as_str()));
        run_systemctl(&args)
    }

    pub fn status(&self, facet_id: &str) -> FacetStatus {
        let unit = unit_name(facet_id);
        let Ok(output) = Command::new("systemctl")
            .args(["--user", "show", &unit, "-p", "ActiveState", "--value"])
            .output()
        else {
            return FacetStatus::Stopped;
        };
        match String::from_utf8_lossy(&output.stdout).trim() {
            "active" | "activating" => FacetStatus::Running,
            "failed" => FacetStatus::Failed("unit failed".into()),
            _ => FacetStatus::Stopped,
        }
    }

    /// Current memory charge for a facet, in kB.
    pub fn memory_current_kb(&self, facet_id: &str) -> Option<u64> {
        let path = self.cgroup_path(facet_id).ok()??;
        let raw = std::fs::read_to_string(path.join("memory.current")).ok()?;
        raw.trim().parse::<u64>().ok().map(|b| b / 1024)
    }

    /// Current swap charge for a facet, in kB.
    ///
    /// Tracked separately from RAM because swap is the axis the spiral runs
    /// along: a facet with high RAM and no swap is working, whereas one pushing
    /// steadily into swap is on its way to taking the machine down.
    pub fn memory_swap_kb(&self, facet_id: &str) -> Option<u64> {
        let path = self.cgroup_path(facet_id).ok()??;
        let raw = std::fs::read_to_string(path.join("memory.swap.current")).ok()?;
        raw.trim().parse::<u64>().ok().map(|b| b / 1024)
    }

    /// Resolve a facet's cgroup directory by asking systemd, rather than
    /// assuming a hierarchy layout that differs between distributions.
    fn cgroup_path(&self, facet_id: &str) -> anyhow::Result<Option<std::path::PathBuf>> {
        let output = Command::new("systemctl")
            .args([
                "--user",
                "show",
                &unit_name(facet_id),
                "-p",
                "ControlGroup",
                "--value",
            ])
            .output()?;
        let relative = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if relative.is_empty() || relative == "/" {
            return Ok(None);
        }
        let path = std::path::PathBuf::from("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
        Ok(path.is_dir().then_some(path))
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

fn run_systemctl(args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "systemctl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::Gate;
    use std::path::PathBuf;

    fn facet() -> Facet {
        Facet {
            id: "comfyui".into(),
            name: "ComfyUI".into(),
            command: vec!["/opt/comfy/run.sh".into(), "--listen".into()],
            cwd: Some(PathBuf::from("/opt/comfy")),
            limits: FacetLimits::default(),
            enabled_if: Gate::default(),
        }
    }

    #[test]
    fn unit_names_are_namespaced() {
        assert_eq!(unit_name("comfyui"), "prism-comfyui.service");
    }

    #[test]
    fn unset_limits_produce_no_properties() {
        assert!(limit_properties(&FacetLimits::default()).is_empty());
    }

    #[test]
    fn only_set_limits_are_rendered() {
        let limits = FacetLimits {
            swap_max: Some("6G".into()),
            ..Default::default()
        };
        assert_eq!(limit_properties(&limits), vec!["MemorySwapMax=6G"]);
    }

    #[test]
    fn all_limits_render_to_systemd_property_names() {
        let limits = FacetLimits {
            memory_high: Some("22G".into()),
            memory_max: Some("26G".into()),
            swap_max: Some("6G".into()),
        };
        assert_eq!(
            limit_properties(&limits),
            vec!["MemoryHigh=22G", "MemoryMax=26G", "MemorySwapMax=6G"]
        );
    }

    #[test]
    fn launch_argv_is_well_formed() {
        let argv = launch_argv(&facet());
        assert_eq!(argv[0], "systemd-run");
        assert!(argv.contains(&"--user".to_string()));
        assert!(argv.contains(&"--unit=prism-comfyui.service".to_string()));
        assert!(argv.contains(&"--property=MemoryAccounting=yes".to_string()));
        assert!(argv.contains(&"--working-directory=/opt/comfy".to_string()));
    }

    #[test]
    fn command_follows_the_double_dash_separator() {
        // Without `--`, a workload flag like `--listen` would be parsed by
        // systemd-run itself rather than passed through.
        let argv = launch_argv(&facet());
        let sep = argv.iter().position(|a| a == "--").expect("has separator");
        assert_eq!(&argv[sep + 1..], &["/opt/comfy/run.sh", "--listen"]);
    }

    #[test]
    fn facet_without_cwd_omits_working_directory() {
        let mut f = facet();
        f.cwd = None;
        let argv = launch_argv(&f);
        assert!(!argv.iter().any(|a| a.starts_with("--working-directory")));
    }

    #[test]
    fn limits_appear_before_the_separator() {
        let mut f = facet();
        f.limits.memory_max = Some("26G".into());
        let argv = launch_argv(&f);
        let sep = argv.iter().position(|a| a == "--").unwrap();
        let prop = argv
            .iter()
            .position(|a| a == "--property=MemoryMax=26G")
            .expect("limit present");
        assert!(prop < sep, "properties must precede the command");
    }

    #[test]
    fn status_of_nonexistent_facet_is_stopped() {
        assert_eq!(
            Supervisor::new().status("definitely-not-a-real-facet-xyzzy"),
            FacetStatus::Stopped
        );
    }

    #[test]
    fn cgroup_lookup_of_nonexistent_facet_is_none() {
        let s = Supervisor::new();
        assert!(s.memory_current_kb("definitely-not-a-real-facet-xyzzy").is_none());
    }

    #[test]
    fn killing_a_stopped_facet_errors_rather_than_targeting_nothing() {
        // Must not silently succeed: a kill that hits no cgroup is a failure to
        // report, not a job done.
        assert!(Supervisor::new().kill("definitely-not-a-real-facet-xyzzy").is_err());
    }
}
