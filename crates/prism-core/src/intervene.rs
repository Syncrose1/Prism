//! **What the governor does at Red and Black** (`architecture.md` §4.2): who
//! is responsible, what to do about it, and when to stop trying.
//!
//! Pure policy: no processes are signalled here. The monitor feeds it what it
//! sensed and carries out the [`Step`] it returns, so every rule below is
//! tested without a machine to break.
//!
//! * **Attribute before acting.** Memory pressure is laid at the facet with
//!   the most `memory.current + memory.swap.current`, weighted by how fast it
//!   grew over the last [`GROWTH_WINDOW`]: a workload climbing by a gigabyte a
//!   minute is the cause even when a larger one sits still. VRAM pressure is
//!   laid at the facet holding the most of the card; a spill at the largest
//!   facet that would spill. A full disk is laid at nobody: stopping a
//!   workload frees no bytes, so it is said, never acted on.
//! * **Red asks, Black stops.** At Red the attributed facet's own hook is
//!   called (ComfyUI: unload models). At Black a facet **Prism launched** is
//!   stopped (SIGTERM, a grace, then `cgroup.kill`). A facet Prism didn't
//!   launch is never stopped, only asked: it isn't Prism's to end, and
//!   processes that aren't facets are never touched at all.
//! * **Cooldowns, and flap protection.** One action per facet per
//!   [`COOLDOWN`]; the [`FLAP_LIMIT`]th inside [`FLAP_WINDOW`] stops automatic
//!   action on it and says so instead. An intervention loop is worse than the
//!   original fault.
//! * **Every action is verified**: [`Verdict`] checks, a while later, the
//!   capability the action was meant to restore (the facet gone, headroom or
//!   the card's debt back), not that the command returned.

use crate::governor::{Driver, Tier};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

pub const GROWTH_WINDOW: Duration = Duration::from_secs(30);
pub const COOLDOWN: Duration = Duration::from_secs(60);
pub const FLAP_WINDOW: Duration = Duration::from_secs(600);
pub const FLAP_LIMIT: usize = 3;
/// How long after acting the result is judged.
pub const VERIFY_AFTER: Duration = Duration::from_secs(15);

/// One facet, as sensed this tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FacetUse {
    pub id: String,
    /// Prism launched it (its unit is Prism's): it may be stopped.
    pub ours: bool,
    /// It has a graceful hook to ask.
    pub hook: bool,
    /// `memory.current + memory.swap.current`, MiB.
    pub memory_mib: u64,
    /// VRAM its processes hold, MiB.
    pub vram_mib: u64,
    /// It declares `offloads_to_ram`.
    pub offloads: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Call the facet's graceful hook.
    Ask(String),
    /// SIGTERM, grace, then `cgroup.kill`.
    Stop(String),
    /// Nothing Prism may do; said once per episode.
    Say(String),
}

/// The governor's hands, with memory of what they did.
#[derive(Default)]
pub struct Interventions {
    /// Each facet's memory over the growth window.
    seen: HashMap<String, VecDeque<(Instant, u64)>>,
    /// When each facet was last acted on.
    acted: HashMap<String, VecDeque<Instant>>,
    /// Facets given up on (flapping), until their window clears.
    held: HashMap<String, Instant>,
    /// What has already been said this episode, so it is said once.
    said: Option<String>,
}

impl Interventions {
    pub fn new() -> Self { Self::default() }

    /// Remember this tick's memory, for growth. Called every tick, at any
    /// tier, so growth is known the moment pressure arrives.
    pub fn sense(&mut self, uses: &[FacetUse], now: Instant) {
        for u in uses {
            let h = self.seen.entry(u.id.clone()).or_default();
            h.push_back((now, u.memory_mib));
            while h.front().is_some_and(|(t, _)| now.duration_since(*t) > GROWTH_WINDOW) { h.pop_front(); }
        }
        self.seen.retain(|id, _| uses.iter().any(|u| &u.id == id));
    }

    /// MiB per minute over the window, never negative.
    fn growth(&self, id: &str) -> u64 {
        let Some(h) = self.seen.get(id) else { return 0 };
        let (Some((t0, m0)), Some((t1, m1))) = (h.front(), h.back()) else { return 0 };
        let secs = t1.duration_since(*t0).as_secs_f64();
        if secs < 1.0 { return 0 }
        (m1.saturating_sub(*m0) as f64 * 60.0 / secs) as u64
    }

    /// Who is responsible for pressure driven by [driver].
    pub fn attribute<'a>(&self, driver: Driver, uses: &'a [FacetUse]) -> Option<&'a FacetUse> {
        match driver {
            Driver::Vram => uses.iter().filter(|u| u.vram_mib > 0).max_by_key(|u| u.vram_mib),
            Driver::Spill => uses.iter().filter(|u| u.offloads && u.vram_mib > 0).max_by_key(|u| u.vram_mib),
            // A minute of growth counts double what's already held: the cause
            // of a spiral is what's climbing.
            Driver::Stall | Driver::Headroom => uses.iter().filter(|u| u.memory_mib > 0).max_by_key(|u| u.memory_mib + 2 * self.growth(&u.id)),
            Driver::Disk | Driver::None => None,
        }
    }

    /// What to do now. `None` while waiting out a cooldown, below Red, or
    /// after something has already been said.
    pub fn decide(&mut self, tier: Tier, driver: Driver, uses: &[FacetUse], now: Instant) -> Option<Step> {
        if tier < Tier::Red {
            self.said = None;
            return None;
        }
        self.held.retain(|_, since| now.duration_since(*since) < FLAP_WINDOW);
        let step = match self.attribute(driver, uses) {
            None if driver == Driver::Disk => Step::Say("the disk is nearly full: stopping a workload frees no space, so nothing is stopped".into()),
            None => Step::Say("pressure with no facet to attribute it to: nothing Prism launched is responsible, and nothing else is touched".into()),
            Some(u) if self.held.contains_key(&u.id) => Step::Say(format!("`{}` keeps needing intervention; automatic action on it has stopped", u.id)),
            Some(u) => {
                let last = self.acted.get(&u.id).and_then(|a| a.back()).copied();
                if last.is_some_and(|t| now.duration_since(t) < COOLDOWN) { return None }
                let step = match (tier, u.ours, u.hook) {
                    (Tier::Black, true, _) => Step::Stop(u.id.clone()),
                    (_, _, true) => Step::Ask(u.id.clone()),
                    (Tier::Black, false, false) => Step::Say(format!("`{}` is responsible, but Prism didn't launch it and it has no hook to ask", u.id)),
                    _ => Step::Say(format!("`{}` is responsible and has no hook to ask; it is stopped if this reaches Black", u.id)),
                };
                if !matches!(step, Step::Say(_)) {
                    let a = self.acted.entry(u.id.clone()).or_default();
                    a.push_back(now);
                    while a.front().is_some_and(|t| now.duration_since(*t) > FLAP_WINDOW) { a.pop_front(); }
                    if a.len() >= FLAP_LIMIT { self.held.insert(u.id.clone(), now); }
                }
                step
            }
        };
        if let Step::Say(s) = &step {
            if self.said.as_deref() == Some(s.as_str()) { return None }
            self.said = Some(s.clone());
        }
        Some(step)
    }
}

/// An action taken, waiting to be judged.
#[derive(Debug, Clone)]
pub struct Pending {
    pub step: Step,
    pub at: Instant,
    pub driver: Driver,
    pub headroom_mib: u64,
    pub vram_overdraft_mib: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not yet time to judge.
    Wait,
    /// It worked: said how.
    Relieved(String),
    /// It didn't: said why, so the next step isn't mistaken for a fix.
    Failed(String),
}

impl Pending {
    /// Judge the action against what is sensed now. [still_running] is
    /// whether the facet acted on still runs.
    pub fn judge(&self, now: Instant, still_running: bool, headroom_mib: u64, vram_overdraft_mib: u64) -> Verdict {
        if now.duration_since(self.at) < VERIFY_AFTER { return Verdict::Wait }
        let id = match &self.step { Step::Ask(id) | Step::Stop(id) => id, Step::Say(_) => return Verdict::Relieved(String::new()) };
        if matches!(self.step, Step::Stop(_)) && still_running {
            return Verdict::Failed(format!("`{id}` is still running after being stopped"));
        }
        let (before, after, what) = match self.driver {
            Driver::Vram | Driver::Spill => (self.vram_overdraft_mib, vram_overdraft_mib, "VRAM owed"),
            _ => (self.headroom_mib, headroom_mib, "headroom"),
        };
        let better = if what == "headroom" { after > before } else { after < before };
        let mib = |m: u64| format!("{:.1} GiB", m as f64 / 1024.0);
        if better { Verdict::Relieved(format!("{what} {} → {}", mib(before), mib(after))) }
        else { Verdict::Failed(format!("`{id}` was {} but {what} didn't improve ({} → {})", if matches!(self.step, Step::Stop(_)) { "stopped" } else { "asked to yield" }, mib(before), mib(after))) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(id: &str, ours: bool, hook: bool, mem: u64, vram: u64) -> FacetUse {
        FacetUse { id: id.into(), ours, hook, memory_mib: mem, vram_mib: vram, offloads: false }
    }

    #[test]
    fn nothing_happens_below_red() {
        let mut i = Interventions::new();
        assert_eq!(i.decide(Tier::Amber, Driver::Headroom, &[u("a", true, true, 9000, 0)], Instant::now()), None);
    }

    #[test]
    fn red_asks_black_stops_and_only_what_prism_launched_is_stopped() {
        let t = Instant::now();
        let uses = [u("comfy", true, true, 9000, 0)];
        assert_eq!(Interventions::new().decide(Tier::Red, Driver::Headroom, &uses, t), Some(Step::Ask("comfy".into())));
        assert_eq!(Interventions::new().decide(Tier::Black, Driver::Headroom, &uses, t), Some(Step::Stop("comfy".into())));
        // Not Prism's: asked even at Black, never stopped.
        let foreign = [u("comfy", false, true, 9000, 0)];
        assert_eq!(Interventions::new().decide(Tier::Black, Driver::Headroom, &foreign, t), Some(Step::Ask("comfy".into())));
        let mute = [u("comfy", false, false, 9000, 0)];
        assert!(matches!(Interventions::new().decide(Tier::Black, Driver::Headroom, &mute, t), Some(Step::Say(_))));
    }

    #[test]
    fn the_climbing_workload_is_blamed_over_a_larger_still_one() {
        let mut i = Interventions::new();
        let t = Instant::now();
        // big holds 8 GiB still; small climbs 2 GiB in 20 s (6 GiB a minute).
        i.sense(&[u("big", true, true, 8000, 0), u("small", true, true, 1000, 0)], t);
        let now = [u("big", true, true, 8000, 0), u("small", true, true, 3000, 0)];
        i.sense(&now, t + Duration::from_secs(20));
        assert_eq!(i.attribute(Driver::Headroom, &now).unwrap().id, "small");
        // Without growth, size decides.
        let fresh = Interventions::new();
        assert_eq!(fresh.attribute(Driver::Headroom, &now).unwrap().id, "big");
    }

    #[test]
    fn the_card_is_laid_at_whoever_holds_it_and_a_spill_at_whoever_would_spill() {
        let i = Interventions::new();
        let mut comfy = u("comfy", true, true, 2000, 4000);
        comfy.offloads = true;
        let uses = [u("llama", true, false, 1000, 9000), comfy];
        assert_eq!(i.attribute(Driver::Vram, &uses).unwrap().id, "llama");
        assert_eq!(i.attribute(Driver::Spill, &uses).unwrap().id, "comfy");
    }

    #[test]
    fn a_full_disk_is_said_and_never_acted_on() {
        let mut i = Interventions::new();
        let t = Instant::now();
        let s = i.decide(Tier::Black, Driver::Disk, &[u("a", true, true, 9000, 0)], t);
        assert!(matches!(s, Some(Step::Say(m)) if m.contains("disk")));
        // Said once per episode, not every tick.
        assert_eq!(i.decide(Tier::Black, Driver::Disk, &[], t + Duration::from_secs(1)), None);
        // A new episode says it again.
        i.decide(Tier::Green, Driver::None, &[], t + Duration::from_secs(2));
        assert!(i.decide(Tier::Black, Driver::Disk, &[], t + Duration::from_secs(3)).is_some());
    }

    #[test]
    fn cooldown_then_flap_protection() {
        let mut i = Interventions::new();
        let t = Instant::now();
        let uses = [u("comfy", true, true, 9000, 0)];
        assert_eq!(i.decide(Tier::Red, Driver::Headroom, &uses, t), Some(Step::Ask("comfy".into())));
        // Inside the cooldown: nothing.
        assert_eq!(i.decide(Tier::Red, Driver::Headroom, &uses, t + Duration::from_secs(30)), None);
        assert_eq!(i.decide(Tier::Red, Driver::Headroom, &uses, t + Duration::from_secs(61)), Some(Step::Ask("comfy".into())));
        // The third inside ten minutes is the last.
        assert_eq!(i.decide(Tier::Red, Driver::Headroom, &uses, t + Duration::from_secs(122)), Some(Step::Ask("comfy".into())));
        let s = i.decide(Tier::Red, Driver::Headroom, &uses, t + Duration::from_secs(183));
        assert!(matches!(s, Some(Step::Say(ref m)) if m.contains("stopped")), "{s:?}");
        assert_eq!(i.decide(Tier::Red, Driver::Headroom, &uses, t + Duration::from_secs(250)), None);
        // Once the window clears, it may act again.
        assert_eq!(i.decide(Tier::Red, Driver::Headroom, &uses, t + Duration::from_secs(122 + 601)), Some(Step::Ask("comfy".into())));
    }

    #[test]
    fn an_action_is_judged_by_what_it_restored() {
        let t = Instant::now();
        let p = Pending { step: Step::Stop("comfy".into()), at: t, driver: Driver::Headroom, headroom_mib: 400, vram_overdraft_mib: 0 };
        assert_eq!(p.judge(t + Duration::from_secs(5), false, 6000, 0), Verdict::Wait);
        assert!(matches!(p.judge(t + VERIFY_AFTER, true, 6000, 0), Verdict::Failed(m) if m.contains("still running")));
        assert!(matches!(p.judge(t + VERIFY_AFTER, false, 6000, 0), Verdict::Relieved(_)));
        assert!(matches!(p.judge(t + VERIFY_AFTER, false, 300, 0), Verdict::Failed(m) if m.contains("didn't improve")));
        let v = Pending { step: Step::Ask("llama".into()), at: t, driver: Driver::Vram, headroom_mib: 9000, vram_overdraft_mib: 3000 };
        assert!(matches!(v.judge(t + VERIFY_AFTER, true, 9000, 0), Verdict::Relieved(m) if m.contains("VRAM")));
    }
}
