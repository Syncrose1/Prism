//! VRAM sensing, and the reason it cannot reuse the memory sensor's shape.
//!
//! Every other resource Prism governs is fungible: a free byte of RAM is a free
//! byte, whoever asks for it next. VRAM is not, and treating it as if it were
//! produces a governor that fires constantly while nothing is wrong.
//!
//! The workload this exists for is a voice agent holding a recogniser, a
//! language model and a synthesiser resident — 11.8 GiB of a 12.29 GiB card,
//! measured. By the naive metric that is a permanent Black tier. It is in fact
//! the system working exactly as designed: the models are resident *because*
//! resident is the only way the latency budget closes. Shedding them to restore
//! "headroom" would destroy the thing being protected in order to protect it.
//!
//! ## So the governed quantity is foreign demand, not free memory
//!
//! What actually hurts is somebody else wanting VRAM and not getting it: a game
//! starting, ComfyUI loading a checkpoint, a browser compositing video. Those
//! processes are not Prism's, cannot be asked to wait, and will either fail
//! outright or fall back to host memory and crawl.
//!
//! So the sensor splits the card in two. `ours_mib` is VRAM held by processes
//! inside a Prism cgroup — workloads the governor can actually shed.
//! `foreign_mib` is everything else. The pressure signal is the second number,
//! and the tier it warrants says how much of the first must go.
//!
//! The split is deliberately computed as *card minus ours*, not as a sum over
//! the foreign processes, and the difference is not small. Measured on this
//! host at idle: `memory.used` reports 1149 MiB while the per-process query
//! attributes only 182 MiB — a compositor, a stream host and a tray icon. The
//! missing 967 MiB is real, occupied, and unavailable to anyone: scanout
//! buffers, driver context, allocations the query does not itemise. Summing
//! processes would undercount what the desktop is actually using by five times
//! and hand a governed workload a budget the card cannot honour.
//!
//! That inverts the usual reading in a way worth being explicit about: **rising
//! foreign use is what raises the tier, and our own use does not raise it at
//! all.** A card full of nothing but Prism's models sits at Green, because
//! every byte of it can be given back the moment something asks.
//!
//! ## Attribution is by cgroup, not by name
//!
//! A PID is ours if `/proc/<pid>/cgroup` names a `prism-` unit — the scope or
//! service the supervisor started it under. Matching on process names would be
//! both fragile and wrong: two `python3` processes, one a facet and one the
//! contractor's own notebook, are different in exactly the way that matters.
//!
//! ## Why `nvidia-smi` and not NVML
//!
//! NVML is on this machine and would sample in about a millisecond. `nvidia-smi`
//! costs 14 ms, measured, against a tick interval of seconds — so the speed is
//! not worth the cost of hand-rolled FFI against an interface whose symbols are
//! version-suffixed (`_v2`, `_v3`) and change with the driver. A sensor that
//! silently returns nothing after a driver update is worse than one that costs
//! 14 ms. If the tick ever gets fast enough for this to matter, NVML goes behind
//! this same `sample()`.
//!
//! A host with no NVIDIA GPU returns `None` throughout, and the governor treats
//! absent VRAM sensing the way it treats absent disk sensing: no pressure, never
//! manufactured.

use std::process::Command;

/// Where `nvidia-smi` lives, if it does. Resolved per call; cheap next to the
/// process spawn, and it means a driver installed after prismd started is
/// picked up without a restart.
const SMI: &str = "nvidia-smi";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuMemory {
    pub total_mib: u64,
    pub used_mib: u64,
    pub free_mib: u64,
}

#[derive(Debug, Clone)]
pub struct GpuProcess {
    pub pid: u32,
    pub used_mib: u64,
    pub name: String,
    /// The facet this process belongs to, or `None` if it is not Prism's.
    ///
    /// The id rather than a bare boolean, because "whose VRAM is this" is the
    /// question that decides whether a spill into RAM is coming: ComfyUI
    /// answers a VRAM shortage by moving weights to host memory, and llama.cpp
    /// answers it by failing. Knowing only that the VRAM is *ours* cannot tell
    /// those apart.
    pub owner: Option<String>,
}

impl GpuProcess {
    /// Whether the governor has any way to make this process let go.
    pub fn ours(&self) -> bool {
        self.owner.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct GpuSnapshot {
    pub memory: GpuMemory,
    pub processes: Vec<GpuProcess>,
}

impl GpuSnapshot {
    /// VRAM held by governed workloads: what can be shed.
    pub fn ours_mib(&self) -> u64 {
        self.processes.iter().filter(|p| p.ours()).map(|p| p.used_mib).sum()
    }

    /// VRAM held by one facet, across every process in its cgroup.
    ///
    /// Summed rather than taken from the largest process because a workload
    /// that forks — ComfyUI's workers, a server with a sampler subprocess —
    /// holds VRAM in several places and would otherwise be undercounted at
    /// exactly the moment the total matters.
    pub fn facet_mib(&self, facet_id: &str) -> u64 {
        self.processes
            .iter()
            .filter(|p| p.owner.as_deref() == Some(facet_id))
            .map(|p| p.used_mib)
            .sum()
    }

    /// VRAM held by everything else: what is driving the tier.
    ///
    /// The card's own occupancy less our share, rather than a sum over foreign
    /// processes, so that everything `nvidia-smi` declines to itemise still
    /// counts against us. It is occupied either way.
    pub fn foreign_mib(&self) -> u64 {
        self.memory.used_mib.saturating_sub(self.ours_mib())
    }

    /// What a governed workload may hold without crowding anyone.
    ///
    /// Total, less what foreigners already hold, less a reserve for the
    /// foreigner that has not started yet. The reserve is the whole reason this
    /// is a *budget* and not simply "whatever is free": by the time a game has
    /// failed to allocate, shedding is too late — the allocation already
    /// errored. Prism must be holding less than it could, in advance.
    pub fn budget_mib(&self, reserve_mib: u64) -> u64 {
        self.memory
            .total_mib
            .saturating_sub(self.foreign_mib())
            .saturating_sub(reserve_mib)
    }

    /// How much a governed workload is over its budget. Zero when within it.
    ///
    /// This is the number the degradation ladder acts on: not a tier, not a
    /// percentage, but "give back this many MiB".
    pub fn overdraft_mib(&self, reserve_mib: u64) -> u64 {
        self.ours_mib().saturating_sub(self.budget_mib(reserve_mib))
    }
}

/// Sample the first GPU, or `None` if there is no NVIDIA driver here.
///
/// Single-GPU by design. A multi-GPU host would want per-device budgets and
/// per-facet affinity, which is a different feature; pretending to support it by
/// summing the cards would produce a budget no single allocation can draw on.
pub fn sample() -> Option<GpuSnapshot> {
    let memory = sample_memory()?;
    let processes = sample_processes();
    Some(GpuSnapshot { memory, processes })
}

fn sample_memory() -> Option<GpuMemory> {
    let out = Command::new(SMI)
        .args([
            "--id=0",
            "--query-gpu=memory.total,memory.used,memory.free",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?;
    let mut fields = line.split(',').map(|f| f.trim().parse::<u64>().ok());
    Some(GpuMemory {
        total_mib: fields.next()??,
        used_mib: fields.next()??,
        free_mib: fields.next()??,
    })
}

fn sample_processes() -> Vec<GpuProcess> {
    let Ok(out) = Command::new(SMI)
        .args([
            "--id=0",
            "--query-compute-apps=pid,used_memory,process_name",
            "--format=csv,noheader,nounits",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_process)
        .collect()
}

fn parse_process(line: &str) -> Option<GpuProcess> {
    // `process_name` is a path and may itself contain commas, so split from the
    // left for the two numeric fields and keep the rest whole.
    let mut parts = line.splitn(3, ',');
    let pid: u32 = parts.next()?.trim().parse().ok()?;
    // A process that has not allocated yet reports "[N/A]" rather than a number.
    let used_mib: u64 = parts.next()?.trim().parse().unwrap_or(0);
    let name = parts.next().unwrap_or("").trim().to_string();
    let owner = owner_of(pid);
    Some(GpuProcess { pid, used_mib, name, owner })
}

/// Which facet, if any, a PID belongs to.
///
/// Both shapes the supervisor uses are covered — `prism-<id>.service` and
/// `prism-facet-<id>.scope`. A process that has exited between the two
/// `nvidia-smi` calls has no cgroup file and reads as foreign, which is the
/// safe direction: it makes the governor slightly more cautious rather than
/// slightly less.
fn owner_of(pid: u32) -> Option<String> {
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    facet_of_cgroup(&cgroup)
}

/// Extract a facet id from the contents of `/proc/<pid>/cgroup`.
///
/// Split out from the read so it can be tested against real cgroup lines
/// without a process to point at.
pub(crate) fn facet_of_cgroup(cgroup: &str) -> Option<String> {
    for line in cgroup.lines() {
        for segment in line.split('/') {
            // `prism-facet-<id>.scope` must be tried first: it also starts with
            // `prism-`, so the service pattern would otherwise claim it and
            // return `facet-<id>`.
            if let Some(rest) = segment.strip_prefix("prism-facet-") {
                if let Some(id) = rest.strip_suffix(".scope") {
                    return Some(id.to_string());
                }
            }
            if let Some(rest) = segment.strip_prefix("prism-") {
                if let Some(id) = rest.strip_suffix(".service") {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `used` is the card's own occupancy, which is always at least the sum of
    /// the processes named and usually more.
    fn snapshot(total: u64, used: u64, procs: &[(u64, bool)]) -> GpuSnapshot {
        GpuSnapshot {
            memory: GpuMemory {
                total_mib: total,
                used_mib: used,
                free_mib: total.saturating_sub(used),
            },
            processes: procs
                .iter()
                .enumerate()
                .map(|(i, (used, ours))| GpuProcess {
                    pid: i as u32 + 1,
                    used_mib: *used,
                    name: "t".into(),
                    owner: ours.then(|| "stargazer".to_string()),
                })
                .collect(),
        }
    }

    #[test]
    fn our_own_models_do_not_count_against_us() {
        // 6 GiB of a bare card, all of it Prism's. Nothing foreign, so the
        // whole card less the reserve is ours to use and there is no
        // overdraft — which is the point of governing foreign demand rather
        // than free memory.
        let s = snapshot(12288, 6144, &[(6144, true)]);
        assert_eq!(s.foreign_mib(), 0);
        assert_eq!(s.budget_mib(1024), 12288 - 1024);
        assert_eq!(s.overdraft_mib(1024), 0);
    }

    #[test]
    fn unattributed_vram_counts_as_foreign() {
        // Measured at idle on this host: the card reports 1149 MiB used while
        // the process query itemises 182. Summing processes would call the
        // other 967 free. It is not.
        let s = snapshot(12288, 1149, &[(154, false), (22, false), (6, false)]);
        assert_eq!(s.foreign_mib(), 1149);
    }

    #[test]
    fn a_foreign_process_creates_the_overdraft() {
        // We hold 6 GiB. A game arrives and takes the rest of the card, so
        // foreign use is now 6 GiB too. Our budget is what remains after the
        // foreigner and the reserve — 5 GiB — so a gigabyte has to go back.
        //
        // Note what the card reports while this is true: zero free. The
        // overdraft is not derived from that number and must not be, because
        // a full card says nothing about who can give.
        let s = snapshot(12288, 12288, &[(6144, true)]);
        assert_eq!(s.foreign_mib(), 6144);
        assert_eq!(s.budget_mib(1024), 5120);
        assert_eq!(s.overdraft_mib(1024), 1024);
    }

    #[test]
    fn the_reserve_is_what_makes_shedding_early_rather_than_late() {
        // Nothing foreign is running and we still may not fill the card: the
        // allocation that fails is the one nobody has made yet.
        let s = snapshot(12288, 12288, &[(12288, true)]);
        assert_eq!(s.overdraft_mib(1024), 1024);
        assert_eq!(s.overdraft_mib(0), 0);
    }

    #[test]
    fn the_full_resident_stack_does_not_fit_a_working_desktop() {
        // The measurement this whole ladder exists because of. All three
        // models resident peaked at 11.80 GiB of a 12.29 GiB card — which fit,
        // but only with the screen doing nothing. An idle KDE session on this
        // same card holds 1.12 GiB before anything else starts.
        const STACK_MIB: u64 = 11_800;
        const IDLE_DESKTOP_MIB: u64 = 1_149;
        const CARD_MIB: u64 = 12_288;
        assert!(STACK_MIB + IDLE_DESKTOP_MIB > CARD_MIB);

        // So the reachable state is: desktop up, and as much of the stack as
        // the budget allows. The governor is not an exception path here — it
        // is how the product runs at rest.
        let ours = CARD_MIB - IDLE_DESKTOP_MIB;
        let s = snapshot(CARD_MIB, CARD_MIB, &[(ours, true)]);
        assert_eq!(s.foreign_mib(), IDLE_DESKTOP_MIB);
        assert_eq!(s.overdraft_mib(1024), 1024);
    }

    #[test]
    fn a_process_name_containing_a_comma_survives_parsing() {
        let p = parse_process("1234, 512, /opt/my, app/bin/run").unwrap();
        assert_eq!(p.pid, 1234);
        assert_eq!(p.used_mib, 512);
        assert_eq!(p.name, "/opt/my, app/bin/run");
    }

    #[test]
    fn a_scope_is_not_mistaken_for_a_service_named_facet_something() {
        // `prism-facet-comfyui.scope` also starts with `prism-`, so trying the
        // service pattern first would attribute its VRAM to a facet called
        // `facet-comfyui` that does not exist — and the spill liability for the
        // real ComfyUI would read as zero.
        assert_eq!(
            facet_of_cgroup("0::/user.slice/user-1000.slice/prism-facet-comfyui.scope"),
            Some("comfyui".into())
        );
        assert_eq!(
            facet_of_cgroup("0::/user.slice/user-1000.slice/prism-comfyui.service"),
            Some("comfyui".into())
        );
        assert_eq!(facet_of_cgroup("0::/user.slice/app.slice/firefox.scope"), None);
    }

    #[test]
    fn vram_is_summed_across_every_process_a_facet_forked() {
        let s = GpuSnapshot {
            memory: GpuMemory { total_mib: 12288, used_mib: 5000, free_mib: 7288 },
            processes: vec![
                GpuProcess { pid: 1, used_mib: 3000, name: "a".into(), owner: Some("comfyui".into()) },
                GpuProcess { pid: 2, used_mib: 1500, name: "b".into(), owner: Some("comfyui".into()) },
                GpuProcess { pid: 3, used_mib: 500, name: "c".into(), owner: Some("llama".into()) },
            ],
        };
        assert_eq!(s.facet_mib("comfyui"), 4500);
        assert_eq!(s.facet_mib("llama"), 500);
        assert_eq!(s.facet_mib("nothing"), 0);
    }

    #[test]
    fn a_process_that_has_not_allocated_reads_as_zero_not_as_a_parse_failure() {
        let p = parse_process("99, [N/A], /usr/bin/x").unwrap();
        assert_eq!(p.used_mib, 0);
    }
}

#[cfg(test)]
mod live {
    /// Sample the real card. Ignored by default: it needs an NVIDIA driver and
    /// says nothing on a host without one. Run with `--ignored --nocapture` to
    /// check the parse against whatever `nvidia-smi` on this box actually emits.
    #[test]
    #[ignore]
    fn reads_this_machine() {
        let Some(s) = super::sample() else {
            println!("no NVIDIA GPU here");
            return;
        };
        println!(
            "total {} MiB · used {} MiB · ours {} · foreign {} · budget {} · overdraft {}",
            s.memory.total_mib,
            s.memory.used_mib,
            s.ours_mib(),
            s.foreign_mib(),
            s.budget_mib(1024),
            s.overdraft_mib(1024)
        );
        for p in &s.processes {
            println!(
                "  {} {:>6} MiB  owner={}  {}",
                p.pid,
                p.used_mib,
                p.owner.as_deref().unwrap_or("-"),
                p.name
            );
        }
        assert!(s.memory.total_mib > 0);
        assert!(s.ours_mib() <= s.memory.used_mib);
    }
}
