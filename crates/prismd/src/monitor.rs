//! The sensing loop.
//!
//! Two independent detection paths run at the same 1 Hz cadence:
//!
//! * the **governor**, which reacts to system-wide pressure, and
//! * the **storm detector**, which reacts to process count and spawn rate.
//!
//! The storm path deliberately does not wait on the governor's tier escalation.
//! A recursion producing ~300 MB every three seconds consumes gigabytes before
//! PSI registers anything, so a detector chained behind pressure tiers would
//! always arrive after the damage. Prism catches shape before it catches size.

use crate::action;
use crate::api::{SharedVitals, Vitals, VramVitals};
use prism_core::config::Profile;
use prism_core::governor::{Governor, Reading};
use prism_core::graceful::{self, Deficit};
use prism_core::sensors::{disk, gpu, memory, process};
use prism_core::supervisor::{FacetStatus, Supervisor};
use prism_core::watchdog::storm::{StormAction, StormDetector, StormVerdict};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

const TICK: Duration = Duration::from_secs(1);

/// Sample the GPU every other tick.
///
/// `nvidia-smi` costs 14 ms per invocation and the sensor makes two, so at 1 Hz
/// it would be a standing tax of several percent of a core forever — an
/// unreasonable price on a daemon whose argument for existing is that it stays
/// responsive when nothing else does. Every two seconds is still well inside
/// the sustain window, so it changes when a shed is decided by at most one tick
/// and never whether.
///
/// Measured on this host after the fact, from the daemon's reaped-child CPU:
/// 20 invocations in 20 seconds costing 0.40 core-seconds, or **2% of one
/// core** — about 20 ms per call once fork overhead is counted, against the
/// 14 ms the binary takes on its own.
const GPU_EVERY: u64 = 2;
const TERM_GRACE: Duration = Duration::from_secs(3);

pub struct Monitor {
    governor: Governor,
    storms: StormDetector,
    psi: memory::PsiTracker,
    vitals: SharedVitals,
    disk_paths: Vec<std::path::PathBuf>,
    /// Held separately because `Governor` owns its config privately and the
    /// overdraft has to be computed before the reading is built.
    vram_reserve_mib: u64,
    /// Last GPU sample, held between the ticks that do not take one.
    gpu: Option<gpu::GpuSnapshot>,
    ticks: u64,
    /// The live facet list, shared with the API so that a hook edited in the
    /// browser takes effect at the next transition rather than at the next
    /// restart of the daemon.
    facets: std::sync::Arc<std::sync::RwLock<Vec<prism_core::config::Facet>>>,
    terminals: std::sync::Arc<prism_core::term::session::SessionManager>,
    events: std::sync::Arc<prism_core::events::EventLog>,
}

impl Monitor {
    pub fn new(
        profile: Profile,
        vitals: SharedVitals,
        terminals: std::sync::Arc<prism_core::term::session::SessionManager>,
        events: std::sync::Arc<prism_core::events::EventLog>,
        facets: std::sync::Arc<std::sync::RwLock<Vec<prism_core::config::Facet>>>,
    ) -> Self {
        let (storms, skipped) = StormDetector::new(profile.storm);
        for reason in &skipped {
            info!(rule = %reason, "storm rule inactive on this host");
        }
        info!(
            active_rules = storms.rule_count(),
            inactive_rules = skipped.len(),
            "storm detector ready"
        );

        let disk_paths = if profile.governor.disk_paths.is_empty() {
            disk::default_paths()
        } else {
            profile.governor.disk_paths.clone()
        };
        for usage in disk::sample(&disk_paths) {
            info!(
                path = %usage.path.display(),
                free_gib = format!("{:.1}", usage.available_mib() as f64 / 1024.0),
                used_pct = format!("{:.0}%", usage.used_pct()),
                "watching filesystem"
            );
        }

        let vram_reserve_mib = profile.governor.vram_reserve_mib;

        Self {
            governor: Governor::new(profile.governor),
            storms,
            psi: memory::PsiTracker::new(),
            vitals,
            disk_paths,
            vram_reserve_mib,
            gpu: None,
            ticks: 0,
            facets,
            terminals,
            events,
        }
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        let mut next = Instant::now();
        loop {
            next += TICK;
            self.tick();
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                // Fell behind, most likely because the machine is thrashing.
                // Resynchronise rather than accumulating debt and then busy
                // looping to catch up, which would add load at the worst moment.
                next = now;
            }
        }
    }

    fn tick(&mut self) {
        if self.ticks % GPU_EVERY == 0 {
            // `None` on a host with no NVIDIA driver, and on every later tick
            // too — so the absent case costs one failed spawn every two
            // seconds and nothing else. Deliberately not cached as "known
            // absent": a driver loaded after prismd started should be picked
            // up without a restart, which matters on a machine where the
            // contractor is installing things while it runs.
            self.gpu = gpu::sample();
        }
        self.ticks = self.ticks.wrapping_add(1);

        let spill_liability_mib = self.spill_liability();
        let vram = self.gpu.as_ref().map(|g| VramVitals {
            total_mib: g.memory.total_mib,
            ours_mib: g.ours_mib(),
            foreign_mib: g.foreign_mib(),
            budget_mib: g.budget_mib(self.vram_reserve_mib),
            overdraft_mib: g.overdraft_mib(self.vram_reserve_mib),
            spill_liability_mib: spill_liability_mib.unwrap_or(0),
        });

        match memory::sample() {
            Ok(mem) => {
                let stall = memory::sample_psi()
                    .and_then(|raw| self.psi.update(raw))
                    .unwrap_or_default();

                let disks = disk::sample(&self.disk_paths);
                let tightest = disk::tightest(&disks);
                let reading = Reading {
                    stall_full: stall.full,
                    headroom_mib: mem.honest_headroom_kb / 1024,
                    disk_free_mib: tightest.map(|d| d.available_mib()),
                    vram_overdraft_mib: vram.as_ref().map(|v| v.overdraft_mib),
                    spill_liability_mib,
                };

                let tier_now = self.governor.tier();

                // Publish before acting, so the dashboard reflects the state
                // that motivated any intervention rather than its aftermath.
                if let Ok(mut vitals) = self.vitals.write() {
                    *vitals =
                        Vitals::from_sample(&mem, stall.full, tier_now, tightest, vram.clone());
                }

                if let Some(tier) = self.governor.observe(&reading) {
                    // Ask before constraining. This runs on every transition,
                    // including down to Green — a workload that shed two
                    // models needs to be told when it may load them again, and
                    // nothing else will tell it. Whether a given tier means
                    // give up or take back is the workload's reading of the
                    // tier in the body, not Prism's.
                    self.ask_facets_to_yield(tier, &reading, vram.as_ref());

                    // De-escalate terminal retention with the tier. Holding a
                    // quarter of a megabyte of scrollback per session is a
                    // luxury a failing machine cannot afford.
                    crate::term_api::apply_tier(&self.terminals, tier);
                    self.terminals.reap();

                    self.events.push_detailed(
                        match tier {
                            prism_core::governor::Tier::Green => prism_core::events::Level::Info,
                            prism_core::governor::Tier::Amber => prism_core::events::Level::Warn,
                            _ => prism_core::events::Level::Error,
                        },
                        "governor",
                        format!("tier {}", tier.as_str()),
                        Some(format!(
                            "{:?} · {:.1}% stall · {:.1} GiB honest headroom{}",
                            self.governor.driver(),
                            stall.full * 100.0,
                            mem.honest_headroom_gib(),
                            vram
                                .as_ref()
                                .filter(|v| v.overdraft_mib > 0 || v.spill_liability_mib > 0)
                                .map(|v| format!(
                                    " · {:.1} GiB VRAM owed ({:.1} GiB foreign) · \
                                     {:.1} GiB would spill to RAM",
                                    v.overdraft_mib as f64 / 1024.0,
                                    v.foreign_mib as f64 / 1024.0,
                                    v.spill_liability_mib as f64 / 1024.0
                                ))
                                .unwrap_or_default()
                        )),
                    );

                    warn!(
                        tier = tier.as_str(),
                        driver = ?self.governor.driver(),
                        stall_full = format!("{:.1}%", stall.full * 100.0),
                        honest_headroom = format!("{:.2} GiB", mem.honest_headroom_gib()),
                        phantom = format!("{:.2} GiB", mem.phantom_headroom_kb() as f64 / 1_048_576.0),
                        disk_free = tightest
                            .map(|d| format!("{:.1} GiB on {}", d.available_mib() as f64 / 1024.0, d.path.display()))
                            .unwrap_or_else(|| "not sensed".into()),
                        vram = vram
                            .as_ref()
                            .map(|v| format!(
                                "{:.1} GiB ours / {:.1} GiB foreign / {:.1} GiB budget",
                                v.ours_mib as f64 / 1024.0,
                                v.foreign_mib as f64 / 1024.0,
                                v.budget_mib as f64 / 1024.0
                            ))
                            .unwrap_or_else(|| "not sensed".into()),
                        headroom_after_spill = format!(
                            "{:.2} GiB",
                            reading.headroom_after_spill_mib() as f64 / 1024.0
                        ),
                        "pressure tier changed"
                    );
                }
            }
            Err(e) => error!(error = %e, "memory sample failed"),
        }

        for verdict in self.detect_storms() {
            self.respond(verdict);
        }
    }

    /// How much RAM would be demanded if every offload-capable facet spilled.
    ///
    /// Only facets that declare `offloads_to_ram` count. A workload that fails
    /// an allocation rather than migrating out of it creates no liability, and
    /// counting its VRAM here would produce a permanent phantom debt on a
    /// machine holding a resident model stack — which is exactly the reading
    /// the VRAM sensor exists to avoid making.
    ///
    /// `None` rather than `Some(0)` when there is no GPU: nothing is being
    /// sensed, which is different from nothing being owed.
    fn spill_liability(&self) -> Option<u64> {
        let gpu = self.gpu.as_ref()?;
        let facets = self.facets.read().ok()?;
        Some(
            facets
                .iter()
                .filter(|f| f.offloads_to_ram)
                .map(|f| gpu.facet_mib(&f.id))
                .sum(),
        )
    }

    /// Fire every configured graceful hook for the new tier.
    ///
    /// Every facet with a hook, not only the ones holding the contended
    /// resource, and that is deliberate. Prism can see that a facet's processes
    /// hold VRAM; it cannot see that a facet is *about to* allocate some, or
    /// that it caches on disk, or that it has a queue it would rather drain
    /// than abandon. Telling everyone the tier and letting each decide is the
    /// same division as everywhere else in this file.
    ///
    /// A facet that is not running is skipped, because a hook that always fails
    /// with connection-refused fills the log with the facet refusing to do
    /// something it was never in a position to do.
    fn ask_facets_to_yield(&self, tier: prism_core::governor::Tier, reading: &Reading,
                           vram: Option<&VramVitals>) {
        let facets = match self.facets.read() {
            Ok(f) => f.clone(),
            Err(_) => return,
        };
        let supervisor = Supervisor::new();
        for facet in facets {
            let Some(hook) = facet.graceful.clone() else { continue };
            // Foreign counts: Prism did not start it and cannot limit it, so
            // asking is the only lever there is.
            if !matches!(
                supervisor.status(&facet.id),
                FacetStatus::Running | FacetStatus::Foreign
            ) {
                continue;
            }
            graceful::spawn(
                facet.id.clone(),
                hook,
                Deficit {
                    facet: facet.id.clone(),
                    tier,
                    vram_overdraft_mib: vram.map(|v| v.overdraft_mib),
                    vram_budget_mib: vram.map(|v| v.budget_mib),
                    headroom_mib: reading.headroom_mib,
                },
            );
        }
    }

    /// Scan `/proc` once and test every rule against the result.
    ///
    /// One pass serves all rules: the process table is read a single time per
    /// tick regardless of how many patterns are configured, so adding rules
    /// costs regex evaluation rather than another walk of `/proc`.
    fn detect_storms(&mut self) -> Vec<StormVerdict> {
        let mut table: Vec<(u32, String)> = Vec::new();
        process::for_each(|pid, cmdline| table.push((pid, cmdline.to_string())));

        let mut cache: HashMap<*const u8, Vec<u32>> = HashMap::new();
        self.storms.evaluate(|re| {
            let key = re.as_str().as_ptr();
            cache
                .entry(key)
                .or_insert_with(|| {
                    table
                        .iter()
                        .filter(|(_, cmd)| re.is_match(cmd))
                        .map(|(pid, _)| *pid)
                        .collect()
                })
                .clone()
        })
    }

    fn respond(&mut self, verdict: StormVerdict) {
        let rss_mib = process::total_rss_kb(verdict.pids.iter().copied()) / 1024;

        if verdict.suppressed {
            self.events.push(
                prism_core::events::Level::Error,
                "storm",
                format!("`{}` recurring after repeated interventions", verdict.rule_id),
            );
            error!(
                rule = %verdict.rule_id,
                count = verdict.count,
                rss_mib,
                "storm recurring after repeated interventions; not acting again. \
                 Manual attention needed."
            );
            return;
        }

        warn!(
            rule = %verdict.rule_id,
            count = verdict.count,
            spawns_per_min = verdict.spawns_per_min,
            rss_mib,
            "{}", verdict.describe()
        );

        match &verdict.action {
            StormAction::Notify => {}
            StormAction::KillMatched => {
                let gone = action::terminate(&verdict.pids, TERM_GRACE);
                // An intervention is recorded with the same weight as an
                // observation, so a later analysis can subtract Prism's own
                // hand from the timeline it is reading.
                self.events.push_detailed(
                    prism_core::events::Level::Action,
                    "storm",
                    format!("contained `{}`", verdict.rule_id),
                    Some(format!(
                        "terminated {} of {} processes, {} MiB reclaimed",
                        gone.len(),
                        verdict.pids.len(),
                        rss_mib
                    )),
                );
                info!(
                    rule = %verdict.rule_id,
                    killed = gone.len(),
                    of = verdict.pids.len(),
                    reclaimed_mib = rss_mib,
                    "storm contained"
                );
            }
            StormAction::Command(argv) => action::spawn_detached(argv),
        }
    }
}
