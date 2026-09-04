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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
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
}

/// Which signal drove the current tier. Reported so an intervention can say why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Stall,
    Headroom,
    Disk,
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

        let worst = by_stall.max(by_headroom).max(by_disk);
        // Attribute to the signal that actually reached the worst tier. Ordered
        // so that when several agree, the most actionable is named.
        let driver = if worst == Tier::Green {
            Driver::None
        } else if by_disk == worst {
            Driver::Disk
        } else if by_headroom == worst {
            Driver::Headroom
        } else {
            Driver::Stall
        };
        (worst, driver)
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

    /// A comfortable reading: no stall, plenty of memory, plenty of disk.
    fn calm() -> Reading {
        Reading {
            stall_full: 0.0,
            headroom_mib: 64_000,
            disk_free_mib: Some(500_000),
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
