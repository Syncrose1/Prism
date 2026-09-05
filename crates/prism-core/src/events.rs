//! The event log behind the Timeline.
//!
//! A bounded, in-memory record of things that happened: tier changes, storms
//! contained, facets started and stopped, sessions killed.
//!
//! One requirement drives the design. `architecture.md` §2 says *"log Prism's
//! own actions into the timeline it reads"* — because on 2026-09-04 two agents
//! investigating an outage each built a confident, evidence-led, wrong theory,
//! and neither could see its own hand in the data. So an intervention is
//! recorded with the same weight as an observation, and both carry a source, so
//! a later analysis can subtract Prism from the picture.
//!
//! Bounded for the obvious reason: an unbounded log in the daemon that exists to
//! prevent memory exhaustion would be an embarrassing way to cause one.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CAPACITY: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Something was observed.
    Info,
    /// Something is wrong but nothing was done.
    Warn,
    /// Prism acted on the machine.
    Action,
    /// Something failed.
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub unix: u64,
    pub level: Level,
    /// Which subsystem: "governor", "storm", "facet", "terminal", "prism".
    pub source: &'static str,
    pub message: String,
    /// Optional structured extras, for a detail view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Default)]
pub struct EventLog {
    events: RwLock<VecDeque<Event>>,
}

impl EventLog {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(VecDeque::with_capacity(CAPACITY)),
        }
    }

    pub fn push(&self, level: Level, source: &'static str, message: impl Into<String>) {
        self.push_detailed(level, source, message, None);
    }

    pub fn push_detailed(
        &self,
        level: Level,
        source: &'static str,
        message: impl Into<String>,
        detail: Option<String>,
    ) {
        let mut events = self.events.write().expect("event log poisoned");
        if events.len() >= CAPACITY {
            events.pop_front();
        }
        events.push_back(Event {
            unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            level,
            source,
            message: message.into(),
            detail,
        });
    }

    /// Most recent first, which is the order a timeline is read in.
    pub fn recent(&self, limit: usize) -> Vec<Event> {
        self.events
            .read()
            .expect("event log poisoned")
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.events.read().expect("event log poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_returns_newest_first() {
        let log = EventLog::new();
        log.push(Level::Info, "governor", "first");
        log.push(Level::Action, "storm", "second");
        let recent = log.recent(10);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].message, "second");
        assert_eq!(recent[1].message, "first");
    }

    #[test]
    fn never_grows_past_its_capacity() {
        // An unbounded log inside the memory-safety daemon would be a poor joke.
        let log = EventLog::new();
        for i in 0..(CAPACITY * 3) {
            log.push(Level::Info, "test", format!("event {i}"));
        }
        assert_eq!(log.len(), CAPACITY);
    }

    #[test]
    fn the_oldest_events_are_the_ones_dropped() {
        let log = EventLog::new();
        for i in 0..(CAPACITY + 10) {
            log.push(Level::Info, "test", format!("event {i}"));
        }
        let all = log.recent(CAPACITY);
        assert_eq!(all[0].message, format!("event {}", CAPACITY + 9));
        assert!(!all.iter().any(|e| e.message == "event 0"));
    }

    #[test]
    fn limit_is_respected() {
        let log = EventLog::new();
        for i in 0..50 {
            log.push(Level::Info, "test", format!("{i}"));
        }
        assert_eq!(log.recent(5).len(), 5);
    }

    #[test]
    fn interventions_are_distinguishable_from_observations() {
        // The whole point: a later analysis must be able to subtract Prism's own
        // actions from the timeline it is reading.
        let log = EventLog::new();
        log.push(Level::Info, "governor", "tier amber");
        log.push(Level::Action, "storm", "killed 12 processes");
        let acted: Vec<_> = log
            .recent(10)
            .into_iter()
            .filter(|e| e.level == Level::Action)
            .collect();
        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].source, "storm");
    }

    #[test]
    fn detail_is_optional_and_omitted_when_absent() {
        let log = EventLog::new();
        log.push(Level::Info, "test", "plain");
        log.push_detailed(Level::Error, "facet", "failed", Some("exit 1".into()));
        let r = log.recent(2);
        assert!(r[0].detail.is_some());
        assert!(r[1].detail.is_none());
        assert!(!serde_json::to_string(&r[1]).unwrap().contains("detail"));
    }

    #[test]
    fn an_empty_log_reads_empty() {
        let log = EventLog::new();
        assert!(log.is_empty());
        assert!(log.recent(10).is_empty());
    }
}
