//! Generic process-storm detection.
//!
//! Origin: on 2026-09-04 this machine was found spawning
//! `qs -p .../killDialog.qml` once every ~3 seconds, ~300 MB per generation,
//! reaching 36 processes and 11.2 GiB before intervention. The cause was a
//! self-referential recursion — the dialog spawner loaded a config that
//! instantiated the dialog spawner — gated only on a conflicting tray process
//! continuing to exist.
//!
//! Nothing in that shape is specific to quickshell, so the detector is not
//! either. It matches a command-line pattern and fires on either an absolute
//! count or a spawn *rate*, which is what distinguishes a runaway from a
//! legitimately busy machine. The original incident becomes one config entry.
//!
//! Rate matters more than count. A pool of 40 workers is normal; 40 processes
//! that did not exist ninety seconds ago is not.

use crate::gate::Gate;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

fn default_cooldown() -> u64 {
    30
}
fn default_max_count() -> usize {
    8
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StormRule {
    pub id: String,
    /// Regex matched against the full command line.
    pub pattern: String,
    /// Fire once strictly more than this many matches exist.
    #[serde(default = "default_max_count")]
    pub max_count: usize,
    /// Fire once new matching processes appear faster than this, per minute.
    #[serde(default)]
    pub max_spawns_per_min: Option<f64>,
    #[serde(default)]
    pub action: StormAction,
    #[serde(default)]
    pub enabled_if: Gate,
    /// Minimum seconds between actions for this rule.
    #[serde(default = "default_cooldown")]
    pub cooldown_secs: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum StormAction {
    /// Report only. The safe default: a new rule should never kill on its first
    /// encounter with a pattern its author may have written too broadly.
    #[default]
    Notify,
    /// Terminate exactly the matched processes, leaving everything else alone.
    KillMatched,
    /// Run an arbitrary command, e.g. a compositor-specific restart.
    Command(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StormTrigger {
    CountExceeded,
    RateExceeded,
}

#[derive(Debug, Clone)]
pub struct StormVerdict {
    pub rule_id: String,
    pub trigger: StormTrigger,
    pub count: usize,
    pub spawns_per_min: f64,
    pub pids: Vec<u32>,
    pub action: StormAction,
    /// True once flap protection has demoted this rule to reporting only.
    pub suppressed: bool,
}

impl StormVerdict {
    pub fn describe(&self) -> String {
        let reason = match self.trigger {
            StormTrigger::CountExceeded => format!("{} processes", self.count),
            StormTrigger::RateExceeded => format!("{:.0} spawns/min", self.spawns_per_min),
        };
        format!("storm `{}`: {reason}", self.rule_id)
    }
}

/// How many actions on one rule within `FLAP_WINDOW` before Prism stops acting.
///
/// An intervention loop is worse than the fault it is chasing, particularly on
/// an unattended machine. Past this point the rule reports and does nothing.
const FLAP_LIMIT: usize = 3;
const FLAP_WINDOW: Duration = Duration::from_secs(600);
const RATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Default)]
struct RuleState {
    known_pids: HashSet<u32>,
    spawns: Vec<Instant>,
    actions: Vec<Instant>,
    last_action: Option<Instant>,
    /// Set once the first scan has established a baseline.
    primed: bool,
}

pub struct StormDetector {
    rules: Vec<(StormRule, Regex)>,
    state: HashMap<String, RuleState>,
}

impl StormDetector {
    /// Compiles rules, discarding any with an invalid regex or unmet capability
    /// gate. A profile authored on another machine loses only what it cannot
    /// support here.
    pub fn new(rules: Vec<StormRule>) -> (Self, Vec<String>) {
        let mut compiled = Vec::new();
        let mut skipped = Vec::new();
        for rule in rules {
            match rule.enabled_if.evaluate() {
                crate::gate::GateOutcome::Blocked(why) => {
                    skipped.push(format!("{}: {why}", rule.id));
                    continue;
                }
                crate::gate::GateOutcome::Satisfied => {}
            }
            match Regex::new(&rule.pattern) {
                Ok(re) => compiled.push((rule, re)),
                Err(e) => skipped.push(format!("{}: invalid pattern ({e})", rule.id)),
            }
        }
        (
            Self {
                rules: compiled,
                state: HashMap::new(),
            },
            skipped,
        )
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// One detection pass. Returns a verdict per rule that has tripped.
    ///
    /// `matches` supplies the pids matching each pattern, injected rather than
    /// scanned internally so the whole detector is testable without spawning
    /// real processes.
    pub fn evaluate<F>(&mut self, mut matches: F) -> Vec<StormVerdict>
    where
        F: FnMut(&Regex) -> Vec<u32>,
    {
        let now = Instant::now();
        let mut verdicts = Vec::new();

        for (rule, re) in &self.rules {
            let pids = matches(re);
            let state = self.state.entry(rule.id.clone()).or_default();

            let current: HashSet<u32> = pids.iter().copied().collect();

            // The first pass only establishes a baseline. Without this, every
            // pre-existing process would register as a spawn and a rule could
            // fire spuriously the moment Prism starts.
            if state.primed {
                let new_count = current.difference(&state.known_pids).count();
                state.spawns.extend(std::iter::repeat_n(now, new_count));
            } else {
                state.primed = true;
            }
            state.known_pids = current;
            state.spawns.retain(|t| now.duration_since(*t) < RATE_WINDOW);

            let spawns_per_min = state.spawns.len() as f64;
            let count = pids.len();

            let trigger = if count > rule.max_count {
                Some(StormTrigger::CountExceeded)
            } else if rule
                .max_spawns_per_min
                .is_some_and(|limit| spawns_per_min > limit)
            {
                Some(StormTrigger::RateExceeded)
            } else {
                None
            };

            let Some(trigger) = trigger else { continue };

            if let Some(last) = state.last_action
                && now.duration_since(last) < Duration::from_secs(rule.cooldown_secs)
            {
                continue;
            }

            state.actions.retain(|t| now.duration_since(*t) < FLAP_WINDOW);
            let suppressed = state.actions.len() >= FLAP_LIMIT;

            state.last_action = Some(now);
            state.actions.push(now);

            verdicts.push(StormVerdict {
                rule_id: rule.id.clone(),
                trigger,
                count,
                spawns_per_min,
                pids,
                action: if suppressed {
                    StormAction::Notify
                } else {
                    rule.action.clone()
                },
                suppressed,
            });
        }
        verdicts
    }
}

/// Rules that ship by default.
///
/// Only the recursion that motivated this module, expressed generically. It is
/// gated on `qs` being installed, so on a machine without quickshell it simply
/// never loads.
pub fn builtin_rules() -> Vec<StormRule> {
    vec![StormRule {
        id: "quickshell-conflictkiller-recursion".into(),
        pattern: r"qs\s+-p\s+.*killDialog\.qml".into(),
        // The dialog is meant to be a singleton the user dismisses, so two is
        // already conclusive evidence of recursion rather than normal use.
        max_count: 2,
        max_spawns_per_min: Some(4.0),
        action: StormAction::KillMatched,
        enabled_if: Gate {
            binary: Some("qs".into()),
            ..Default::default()
        },
        cooldown_secs: 15,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, max_count: usize, rate: Option<f64>) -> StormRule {
        StormRule {
            id: id.into(),
            pattern: ".*".into(),
            max_count,
            max_spawns_per_min: rate,
            action: StormAction::KillMatched,
            enabled_if: Gate::default(),
            cooldown_secs: 0,
        }
    }

    #[test]
    fn first_pass_only_primes_and_never_fires_on_rate() {
        let (mut det, _) = StormDetector::new(vec![rule("r", 100, Some(1.0))]);
        // Twenty pre-existing processes must not look like twenty spawns.
        let verdicts = det.evaluate(|_| (1..=20).collect());
        assert!(verdicts.is_empty(), "baseline pass must not trip the rule");
    }

    #[test]
    fn count_threshold_fires() {
        let (mut det, _) = StormDetector::new(vec![rule("r", 2, None)]);
        det.evaluate(|_| vec![1]);
        let v = det.evaluate(|_| vec![1, 2, 3]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].trigger, StormTrigger::CountExceeded);
        assert_eq!(v[0].count, 3);
    }

    #[test]
    fn count_at_threshold_does_not_fire() {
        let (mut det, _) = StormDetector::new(vec![rule("r", 2, None)]);
        det.evaluate(|_| vec![1]);
        assert!(det.evaluate(|_| vec![1, 2]).is_empty());
    }

    #[test]
    fn rate_threshold_fires_below_count_threshold() {
        // The killDialog case: count stayed low early on, but generations kept
        // arriving. Rate is what catches it in the first seconds.
        let (mut det, _) = StormDetector::new(vec![rule("r", 1000, Some(3.0))]);
        det.evaluate(|_| vec![]);
        let v = det.evaluate(|_| vec![1, 2, 3, 4]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].trigger, StormTrigger::RateExceeded);
    }

    #[test]
    fn stable_process_set_never_fires_on_rate() {
        let (mut det, _) = StormDetector::new(vec![rule("r", 1000, Some(1.0))]);
        for _ in 0..5 {
            assert!(det.evaluate(|_| (1..=50).collect()).is_empty());
        }
    }

    #[test]
    fn cooldown_suppresses_repeat_actions() {
        let mut r = rule("r", 1, None);
        r.cooldown_secs = 3600;
        let (mut det, _) = StormDetector::new(vec![r]);
        det.evaluate(|_| vec![]);
        assert_eq!(det.evaluate(|_| vec![1, 2, 3]).len(), 1);
        assert!(det.evaluate(|_| vec![1, 2, 3]).is_empty());
    }

    #[test]
    fn flap_protection_demotes_to_notify() {
        let (mut det, _) = StormDetector::new(vec![rule("r", 1, None)]);
        det.evaluate(|_| vec![]);
        for _ in 0..FLAP_LIMIT {
            let v = det.evaluate(|_| vec![1, 2, 3]);
            assert_eq!(v[0].action, StormAction::KillMatched);
            assert!(!v[0].suppressed);
        }
        let v = det.evaluate(|_| vec![1, 2, 3]);
        assert!(v[0].suppressed, "should stop acting after repeated attempts");
        assert_eq!(v[0].action, StormAction::Notify);
    }

    #[test]
    fn unmet_gate_drops_rule() {
        let mut r = rule("r", 1, None);
        r.enabled_if = Gate {
            binary: Some("definitely-not-real-xyzzy".into()),
            ..Default::default()
        };
        let (det, skipped) = StormDetector::new(vec![r]);
        assert_eq!(det.rule_count(), 0);
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn invalid_regex_is_reported_not_panicked() {
        let mut r = rule("bad", 1, None);
        r.pattern = "((((".into();
        let (det, skipped) = StormDetector::new(vec![r]);
        assert_eq!(det.rule_count(), 0);
        assert!(skipped[0].contains("invalid pattern"));
    }

    #[test]
    fn builtin_pattern_matches_the_real_incident_cmdline() {
        let re = Regex::new(&builtin_rules()[0].pattern).unwrap();
        assert!(re.is_match(
            "/usr/bin/qs -p /home/raahats/.config/quickshell/ii/killDialog.qml"
        ));
        // Must not match the legitimate shell instance.
        assert!(!re.is_match("qs -c ii"));
    }
}
