//! Tiered pressure policy.
//!
//! Tiers are driven by whichever signal is worst: memory stall, honest headroom,
//! or free disk. All are scale-free — a stall fraction, a headroom floor and a
//! free-space floor mean the same thing on a 16 GiB laptop as on a 128 GiB
//! server — which is what lets a profile move between machines unchanged.
//!
//! A tier must hold continuously for `sustain_secs` before it takes effect. A
//! single sampling artefact must never be sufficient to kill a workload that has
//! been running for hours.

use crate::config::GovernorConfig;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Tier {
    #[default]
    Green,
    Amber,
    Red,
    Black,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Green => "green",
            Tier::Amber => "amber",
            Tier::Red => "red",
            Tier::Black => "black",
        }
    }
}

/// One sample of everything the governor considers.
///
/// A struct rather than positional arguments so that adding a signal later
/// cannot silently reorder an existing call site.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reading {
    /// Fraction of wall time fully stalled on memory, 0.0..=1.0.
    pub stall_full: f64,
    /// Honest headroom in MiB — see `sensors::memory`.
    pub headroom_mib: u64,
    /// Free space on the tightest watched mount, if disk is being sensed.
    ///
    /// Absolute rather than percentage: what determines whether the next write
    /// succeeds is how many bytes remain, not what fraction of a 4 TB disk that
    /// happens to be. A host writing multi-gigabyte model files can be at a
    /// comfortable-sounding 90% and still fail the next download.
    pub disk_free_mib: Option<u64>,
    /// How far governed workloads are over their VRAM budget, in MiB, if a GPU
    /// is being sensed.
    ///
    /// Counts up rather than down, and that is not an inconsistency: the other
    /// three signals ask how much is left, this one asks how much is owed. VRAM
    /// is not fungible — a card full of our own models is fully reclaimable and
    /// therefore not pressure at all, while a card with six gigabytes of
    /// somebody else's game on it is pressure even if a gigabyte is free. See
    /// `sensors::gpu`.
    pub vram_overdraft_mib: Option<u64>,
    /// MiB of RAM that would be demanded if every offload-capable facet spilled
    /// its VRAM to host memory right now.
    ///
    /// The signal the other four cannot produce between them. VRAM pressure
    /// does not stay VRAM pressure: a workload that offloads resolves it by
    /// moving weights into RAM, which relieves the card — so the VRAM reading
    /// afterwards looks healthy — and lands the same gigabytes on the axis the
    /// thrash spiral runs along. By the time `headroom_mib` shows it, the
    /// transfer has already happened.
    ///
    /// `None` when there is no GPU, or no facet that offloads.
    pub spill_liability_mib: Option<u64>,
}

impl Reading {
    /// Honest headroom as it would stand after a spill.
    ///
    /// Not a prediction that one will happen — a statement of what is left if
    /// it does.
    pub fn headroom_after_spill_mib(&self) -> u64 {
        self.headroom_mib
            .saturating_sub(self.spill_liability_mib.unwrap_or(0))
    }
}

/// Which signal drove the current tier. Reported so an intervention can say why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Stall,
    Headroom,
    Disk,
    Vram,
    Spill,
    None,
}

pub struct Governor {
    cfg: GovernorConfig,
    current: Tier,
    driver: Driver,
    /// A tier observed but not yet sustained long enough to take effect.
    candidate: Option<(Tier, Instant)>,
}

impl Governor {
    pub fn new(cfg: GovernorConfig) -> Self {
        Self {
            cfg,
            current: Tier::Green,
            driver: Driver::None,
            candidate: None,
        }
    }

    pub fn tier(&self) -> Tier {
        self.current
    }

    /// What is currently driving the tier.
    pub fn driver(&self) -> Driver {
        self.driver
    }

    /// The tier these readings warrant, ignoring sustain, and what drove it.
    pub fn instantaneous(&self, r: &Reading) -> (Tier, Driver) {
        let by_stall = if r.stall_full >= self.cfg.black_stall {
            Tier::Black
        } else if r.stall_full >= self.cfg.red_stall {
            Tier::Red
        } else if r.stall_full >= self.cfg.amber_stall {
            Tier::Amber
        } else {
            Tier::Green
        };

        let by_headroom = if r.headroom_mib <= self.cfg.black_headroom_mib {
            Tier::Black
        } else if r.headroom_mib <= self.cfg.red_headroom_mib {
            Tier::Red
        } else if r.headroom_mib <= self.cfg.amber_headroom_mib {
            Tier::Amber
        } else {
            Tier::Green
        };

        // Absent disk sensing must never manufacture pressure.
        let by_disk = match r.disk_free_mib {
            Some(free) if free <= self.cfg.black_disk_free_mib => Tier::Black,
            Some(free) if free <= self.cfg.red_disk_free_mib => Tier::Red,
            Some(free) if free <= self.cfg.amber_disk_free_mib => Tier::Amber,
            _ => Tier::Green,
        };

        // Absent GPU sensing must never manufacture pressure, same as disk.
        let by_vram = match r.vram_overdraft_mib {
            Some(over) if over >= self.cfg.black_vram_overdraft_mib => Tier::Black,
            Some(over) if over >= self.cfg.red_vram_overdraft_mib => Tier::Red,
            Some(over) if over >= self.cfg.amber_vram_overdraft_mib => Tier::Amber,
            _ => Tier::Green,
        };

        // What headroom would be if every offload-capable facet spilled.
        //
        // Red and Black only, deliberately. This asks "would we survive the
        // thing that is about to happen", not "is something wrong now" — and a
        // machine that would merely be at Amber after a spill is a machine that
        // is fine. Warning constantly about a survivable hypothetical is how a
        // signal becomes furniture, which is the failure the whole daemon is
        // written against.
        let after = self.headroom_after_spill_tier(r);

        let worst = by_stall.max(by_headroom).max(by_disk).max(by_vram).max(after);
        // Attribute to the signal that actually reached the worst tier. Ordered
        // so that when several agree, the most actionable is named.
        let driver = if worst == Tier::Green {
            Driver::None
        } else if after == worst {
            Driver::Spill
        } else if by_vram == worst {
            Driver::Vram
        } else if by_disk == worst {
            Driver::Disk
        } else if by_headroom == worst {
            Driver::Headroom
        } else {
            Driver::Stall
        };
        (worst, driver)
    }

    /// The tier warranted by what headroom would be after a spill.
    ///
    /// Green unless there is a liability to speak of: with no GPU, or no facet
    /// that offloads, this must never manufacture pressure — the same rule as
    /// absent disk and absent GPU sensing.
    fn headroom_after_spill_tier(&self, r: &Reading) -> Tier {
        if r.spill_liability_mib.unwrap_or(0) == 0 {
            return Tier::Green;
        }
        let after = r.headroom_after_spill_mib();
        if after <= self.cfg.black_headroom_mib {
            Tier::Black
        } else if after <= self.cfg.red_headroom_mib {
            Tier::Red
        } else {
            Tier::Green
        }
    }

    /// Feed one sample. Returns `Some(tier)` only when the effective tier
    /// changes, so callers can act on transitions rather than polling.
    pub fn observe(&mut self, reading: &Reading) -> Option<Tier> {
        self.observe_at(reading, Instant::now())
    }

    pub fn observe_at(&mut self, reading: &Reading, now: Instant) -> Option<Tier> {
        let (want, driver) = self.instantaneous(reading);
        if want == self.current {
            self.candidate = None;
            self.driver = driver;
            return None;
        }
        let sustain = Duration::from_secs(self.cfg.sustain_secs);
        match self.candidate {
            Some((tier, since)) if tier == want => {
                if now.duration_since(since) >= sustain {
                    self.current = want;
                    self.driver = driver;
                    self.candidate = None;
                    return Some(want);
                }
            }
            _ => self.candidate = Some((want, now)),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gov() -> Governor {
        Governor::new(GovernorConfig::default())
    }

    /// A comfortable reading: no stall, plenty of memory, plenty of disk, and
    /// governed workloads within their VRAM budget.
    fn calm() -> Reading {
        Reading {
            stall_full: 0.0,
            headroom_mib: 64_000,
            disk_free_mib: Some(500_000),
            vram_overdraft_mib: Some(0),
            spill_liability_mib: Some(0),
        }
    }

    #[test]
    fn starts_green() {
        let g = gov();
        assert_eq!(g.tier(), Tier::Green);
        assert_eq!(g.driver(), Driver::None);
    }

    #[test]
    fn worst_signal_wins() {
        let g = gov();
        // Calm stall and ample disk, but almost no memory headroom.
        let (tier, driver) = g.instantaneous(&Reading {
            headroom_mib: 100,
            ..calm()
        });
        assert_eq!(tier, Tier::Black);
        assert_eq!(driver, Driver::Headroom);

        // Ample everything except stall.
        let (tier, driver) = g.instantaneous(&Reading {
            stall_full: 0.9,
            ..calm()
        });
        assert_eq!(tier, Tier::Black);
        assert_eq!(driver, Driver::Stall);
    }

    #[test]
    fn disk_alone_can_drive_the_tier() {
        // The gap ADR 0001 identified: memory perfectly healthy, disk nearly
        // full, and the machine about to fail every write.
        let g = gov();
        let (tier, driver) = g.instantaneous(&Reading {
            disk_free_mib: Some(100),
            ..calm()
        });
        assert_eq!(tier, Tier::Black);
        assert_eq!(driver, Driver::Disk);
    }

    #[test]
    fn absent_disk_sensing_never_manufactures_pressure() {
        let g = gov();
        let (tier, _) = g.instantaneous(&Reading {
            disk_free_mib: None,
            ..calm()
        });
        assert_eq!(tier, Tier::Green);
    }

    #[test]
    fn vram_overdraft_alone_can_drive_the_tier() {
        // The contention this exists for: host memory and disk perfectly
        // healthy, and a game has taken most of the card out from under a
        // resident model stack.
        let g = gov();
        let (tier, driver) = g.instantaneous(&Reading {
            vram_overdraft_mib: Some(6_600),
            ..calm()
        });
        assert_eq!(tier, Tier::Black);
        assert_eq!(driver, Driver::Vram);
    }

    #[test]
    fn a_small_overdraft_asks_for_the_smallest_shed() {
        let g = gov();
        let (tier, driver) = g.instantaneous(&Reading {
            vram_overdraft_mib: Some(600),
            ..calm()
        });
        assert_eq!(tier, Tier::Amber);
        assert_eq!(driver, Driver::Vram);
    }

    #[test]
    fn absent_gpu_sensing_never_manufactures_pressure() {
        let g = gov();
        let (tier, _) = g.instantaneous(&Reading {
            vram_overdraft_mib: None,
            ..calm()
        });
        assert_eq!(tier, Tier::Green);
    }

    #[test]
    fn a_full_card_of_our_own_models_is_green() {
        // 11.8 GiB resident of a 12.29 GiB card, nothing foreign running. The
        // sensor computes no overdraft, so the governor sees no pressure —
        // which is the whole point of governing foreign demand rather than
        // free memory. Shedding here would destroy the latency budget in
        // order to protect nobody.
        let g = gov();
        let (tier, driver) = g.instantaneous(&Reading {
            vram_overdraft_mib: Some(0),
            ..calm()
        });
        assert_eq!(tier, Tier::Green);
        assert_eq!(driver, Driver::None);
    }

    #[test]
    fn vram_is_named_over_a_coincident_signal_because_it_is_the_actionable_one() {
        // Disk and VRAM both reach Black. The operator can do something about
        // the models in seconds; the disk is a separate problem.
        let g = gov();
        let (tier, driver) = g.instantaneous(&Reading {
            disk_free_mib: Some(100),
            vram_overdraft_mib: Some(6_600),
            ..calm()
        });
        assert_eq!(tier, Tier::Black);
        assert_eq!(driver, Driver::Vram);
    }

    #[test]
    fn a_spill_that_would_be_survivable_is_not_reported() {
        // 9 GiB of headroom and 4 GiB that could land in it. Afterwards there
        // would be 5 GiB left, which is a working machine. Warning here is how
        // a signal becomes furniture.
        let g = gov();
        let (tier, driver) = g.instantaneous(&Reading {
            headroom_mib: 9_216,
            spill_liability_mib: Some(4_096),
            ..calm()
        });
        assert_eq!(tier, Tier::Green);
        assert_eq!(driver, Driver::None);
    }

    #[test]
    fn a_spill_that_would_not_be_survivable_is_caught_before_it_happens() {
        // The case this signal exists for. Host memory looks healthy — 5 GiB of
        // honest headroom is comfortably Green — and the card looks healthy
        // too. But ComfyUI is holding 4.5 GiB of VRAM it answers pressure by
        // moving into RAM, which would leave 500 MiB. Every other signal here
        // reads Green right up until the transfer, and then reads Black.
        let g = gov();
        let r = Reading {
            headroom_mib: 5_120,
            spill_liability_mib: Some(4_608),
            ..calm()
        };
        assert_eq!(g.instantaneous(&Reading { spill_liability_mib: None, ..r }).0, Tier::Green);
        let (tier, driver) = g.instantaneous(&r);
        assert_eq!(tier, Tier::Black);
        assert_eq!(driver, Driver::Spill);
    }

    #[test]
    fn a_facet_that_cannot_spill_owes_nothing() {
        // A resident model stack holding most of the card creates no liability,
        // because it fails an allocation rather than migrating out of one.
        // Counting its VRAM here would produce a permanent phantom debt — the
        // same misreading the VRAM sensor exists to avoid.
        let g = gov();
        let (tier, _) = g.instantaneous(&Reading {
            headroom_mib: 5_120,
            spill_liability_mib: Some(0),
            ..calm()
        });
        assert_eq!(tier, Tier::Green);
    }

    #[test]
    fn absent_gpu_sensing_owes_nothing_either() {
        let g = gov();
        let (tier, _) = g.instantaneous(&Reading {
            headroom_mib: 700,
            spill_liability_mib: None,
            ..calm()
        });
        // 700 MiB is under the 1536 MiB Red floor and over the 512 MiB Black
        // one — so still Red, driven by headroom itself rather than by a
        // hypothesis about a card nobody is watching.
        assert_eq!(tier, Tier::Red);
    }

    #[test]
    fn headroom_after_spill_never_underflows() {
        let r = Reading { headroom_mib: 100, spill_liability_mib: Some(9_000), ..calm() };
        assert_eq!(r.headroom_after_spill_mib(), 0);
    }

    #[test]
    fn the_spill_is_named_over_a_coincident_signal_because_it_explains_the_others() {
        // Disk is also Black. The operator can stop a workload from spilling in
        // seconds; the disk is a separate problem, and naming it here would
        // send them after the wrong one.
        let g = gov();
        let (tier, driver) = g.instantaneous(&Reading {
            headroom_mib: 5_120,
            spill_liability_mib: Some(4_608),
            disk_free_mib: Some(100),
            ..calm()
        });
        assert_eq!(tier, Tier::Black);
        assert_eq!(driver, Driver::Spill);
    }

    #[test]
    fn calm_readings_are_green() {
        let g = gov();
        assert_eq!(g.instantaneous(&calm()).0, Tier::Green);
    }

    #[test]
    fn transient_spike_does_not_change_tier() {
        let mut g = gov();
        let t0 = Instant::now();
        let bad = Reading {
            headroom_mib: 100,
            ..calm()
        };
        assert!(g.observe_at(&bad, t0).is_none());
        // Recovered before the sustain window elapsed.
        assert!(g.observe_at(&calm(), t0 + Duration::from_secs(2)).is_none());
        assert_eq!(g.tier(), Tier::Green);
    }

    #[test]
    fn sustained_pressure_escalates() {
        let mut g = gov();
        let t0 = Instant::now();
        let bad = Reading {
            headroom_mib: 100,
            ..calm()
        };
        assert!(g.observe_at(&bad, t0).is_none());
        assert_eq!(
            g.observe_at(&bad, t0 + Duration::from_secs(11)),
            Some(Tier::Black)
        );
        assert_eq!(g.tier(), Tier::Black);
        assert_eq!(g.driver(), Driver::Headroom);
    }

    #[test]
    fn recovery_also_requires_sustain() {
        let mut g = gov();
        let t0 = Instant::now();
        let bad = Reading {
            headroom_mib: 100,
            ..calm()
        };
        g.observe_at(&bad, t0);
        g.observe_at(&bad, t0 + Duration::from_secs(11));
        assert_eq!(g.tier(), Tier::Black);

        let t1 = t0 + Duration::from_secs(20);
        assert!(g.observe_at(&calm(), t1).is_none());
        assert_eq!(
            g.observe_at(&calm(), t1 + Duration::from_secs(11)),
            Some(Tier::Green)
        );
    }

    #[test]
    fn changing_target_tier_restarts_the_sustain_clock() {
        let mut g = gov();
        let t0 = Instant::now();
        let red = Reading {
            stall_full: 0.25,
            ..calm()
        };
        let amber = Reading {
            stall_full: 0.10,
            ..calm()
        };
        g.observe_at(&red, t0);
        g.observe_at(&amber, t0 + Duration::from_secs(9));
        assert!(g.observe_at(&amber, t0 + Duration::from_secs(12)).is_none());
        assert_eq!(
            g.observe_at(&amber, t0 + Duration::from_secs(20)),
            Some(Tier::Amber)
        );
    }

    #[test]
    fn no_event_emitted_while_tier_is_stable() {
        let mut g = gov();
        let t0 = Instant::now();
        for i in 0..10 {
            assert!(g.observe_at(&calm(), t0 + Duration::from_secs(i)).is_none());
        }
    }
}
