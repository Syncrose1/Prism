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
use crate::api::{SharedVitals, Vitals};
use prism_core::config::Profile;
use prism_core::governor::Governor;
use prism_core::sensors::{memory, process};
use prism_core::watchdog::storm::{StormAction, StormDetector, StormVerdict};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

const TICK: Duration = Duration::from_secs(1);
const TERM_GRACE: Duration = Duration::from_secs(3);

pub struct Monitor {
    governor: Governor,
    storms: StormDetector,
    psi: memory::PsiTracker,
    vitals: SharedVitals,
}

impl Monitor {
    pub fn new(profile: Profile, vitals: SharedVitals) -> Self {
        let (storms, skipped) = StormDetector::new(profile.storm);
        for reason in &skipped {
            info!(rule = %reason, "storm rule inactive on this host");
        }
        info!(
            active_rules = storms.rule_count(),
            inactive_rules = skipped.len(),
            "storm detector ready"
        );

        Self {
            governor: Governor::new(profile.governor),
            storms,
            psi: memory::PsiTracker::new(),
            vitals,
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
        match memory::sample() {
            Ok(mem) => {
                let stall = memory::sample_psi()
                    .and_then(|raw| self.psi.update(raw))
                    .unwrap_or_default();

                let headroom_mib = mem.honest_headroom_kb / 1024;
                let tier_now = self.governor.tier();

                // Publish before acting, so the dashboard reflects the state
                // that motivated any intervention rather than its aftermath.
                if let Ok(mut vitals) = self.vitals.write() {
                    *vitals = Vitals::from_sample(&mem, stall.full, tier_now);
                }

                if let Some(tier) = self.governor.observe(stall.full, headroom_mib) {
                    warn!(
                        tier = tier.as_str(),
                        stall_full = format!("{:.1}%", stall.full * 100.0),
                        honest_headroom = format!("{:.2} GiB", mem.honest_headroom_gib()),
                        phantom = format!("{:.2} GiB", mem.phantom_headroom_kb() as f64 / 1_048_576.0),
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
