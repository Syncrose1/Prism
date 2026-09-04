//! Tiered pressure policy.
//!
//! Tiers are driven by PSI stall and honest headroom, whichever is worse. Both
//! are scale-free — a stall fraction and a headroom floor mean the same thing on
//! a 16 GiB laptop as on a 128 GiB server — which is what lets a profile move
//! between machines unchanged.
//!
//! A tier must hold continuously for `sustain_secs` before it takes effect. A
//! single sampling artefact must never be sufficient to kill a workload that
//! has been running for hours.

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

pub struct Governor {
    cfg: GovernorConfig,
    current: Tier,
    /// A tier observed but not yet sustained long enough to take effect.
    candidate: Option<(Tier, Instant)>,
}

impl Governor {
    pub fn new(cfg: GovernorConfig) -> Self {
        Self {
            cfg,
            current: Tier::Green,
            candidate: None,
        }
    }

    pub fn tier(&self) -> Tier {
        self.current
    }

    /// The tier the current readings warrant, ignoring sustain.
    pub fn instantaneous(&self, stall_full: f64, headroom_mib: u64) -> Tier {
        let by_stall = if stall_full >= self.cfg.black_stall {
            Tier::Black
        } else if stall_full >= self.cfg.red_stall {
            Tier::Red
        } else if stall_full >= self.cfg.amber_stall {
            Tier::Amber
        } else {
            Tier::Green
        };
        let by_headroom = if headroom_mib <= self.cfg.black_headroom_mib {
            Tier::Black
        } else if headroom_mib <= self.cfg.red_headroom_mib {
            Tier::Red
        } else if headroom_mib <= self.cfg.amber_headroom_mib {
            Tier::Amber
        } else {
            Tier::Green
        };
        by_stall.max(by_headroom)
    }

    /// Feed one sample. Returns `Some(tier)` only when the effective tier
    /// changes, so callers can act on transitions rather than polling.
    pub fn observe(&mut self, stall_full: f64, headroom_mib: u64) -> Option<Tier> {
        self.observe_at(stall_full, headroom_mib, Instant::now())
    }

    pub fn observe_at(&mut self, stall_full: f64, headroom_mib: u64, now: Instant) -> Option<Tier> {
        let want = self.instantaneous(stall_full, headroom_mib);
        if want == self.current {
            self.candidate = None;
            return None;
        }
        let sustain = Duration::from_secs(self.cfg.sustain_secs);
        match self.candidate {
            Some((tier, since)) if tier == want => {
                if now.duration_since(since) >= sustain {
                    self.current = want;
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

    #[test]
    fn starts_green() {
        assert_eq!(gov().tier(), Tier::Green);
    }

    #[test]
    fn worst_of_stall_and_headroom_wins() {
        let g = gov();
        // Calm stall but almost no headroom must still read as Black.
        assert_eq!(g.instantaneous(0.0, 100), Tier::Black);
        // Ample headroom but total stall must also read as Black.
        assert_eq!(g.instantaneous(0.9, 64_000), Tier::Black);
        assert_eq!(g.instantaneous(0.0, 64_000), Tier::Green);
    }

    #[test]
    fn transient_spike_does_not_change_tier() {
        let mut g = gov();
        let t0 = Instant::now();
        assert!(g.observe_at(0.9, 100, t0).is_none());
        // Recovered before the sustain window elapsed.
        assert!(g.observe_at(0.0, 64_000, t0 + Duration::from_secs(2)).is_none());
        assert_eq!(g.tier(), Tier::Green);
    }

    #[test]
    fn sustained_pressure_escalates() {
        let mut g = gov();
        let t0 = Instant::now();
        assert!(g.observe_at(0.9, 100, t0).is_none());
        let changed = g.observe_at(0.9, 100, t0 + Duration::from_secs(11));
        assert_eq!(changed, Some(Tier::Black));
        assert_eq!(g.tier(), Tier::Black);
    }

    #[test]
    fn recovery_also_requires_sustain() {
        let mut g = gov();
        let t0 = Instant::now();
        g.observe_at(0.9, 100, t0);
        g.observe_at(0.9, 100, t0 + Duration::from_secs(11));
        assert_eq!(g.tier(), Tier::Black);

        let t1 = t0 + Duration::from_secs(20);
        assert!(g.observe_at(0.0, 64_000, t1).is_none());
        assert_eq!(
            g.observe_at(0.0, 64_000, t1 + Duration::from_secs(11)),
            Some(Tier::Green)
        );
    }

    #[test]
    fn changing_target_tier_restarts_the_sustain_clock() {
        let mut g = gov();
        let t0 = Instant::now();
        g.observe_at(0.25, 64_000, t0); // candidate Red
        // Switches to Amber before Red could be sustained.
        g.observe_at(0.10, 64_000, t0 + Duration::from_secs(9));
        assert!(g.observe_at(0.10, 64_000, t0 + Duration::from_secs(12)).is_none());
        assert_eq!(
            g.observe_at(0.10, 64_000, t0 + Duration::from_secs(20)),
            Some(Tier::Amber)
        );
    }

    #[test]
    fn no_event_emitted_while_tier_is_stable() {
        let mut g = gov();
        let t0 = Instant::now();
        for i in 0..10 {
            assert!(g.observe_at(0.0, 64_000, t0 + Duration::from_secs(i)).is_none());
        }
    }
}
