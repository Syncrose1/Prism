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
//!
//! **Durable** ([`EventLog::durable`], `architecture.md` §4.5): every event is
//! also appended to a file, so the Timeline still holds *the minutes before the
//! machine died* after it comes back. The writing happens on a thread of its
//! own, through a bounded queue the monitor never waits on: a disk stalled by
//! the thrash being recorded must not stall the thing recording it. Warnings,
//! errors and Prism's own actions are synced at once; observations ride along
//! with the next sync. The file rotates at [`FILE_LIMIT`], keeping one previous
//! file, so it is bounded on disk as in memory.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// The durable file rotates past this size.
pub const FILE_LIMIT: u64 = 1 << 20;

pub const CAPACITY: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub unix: u64,
    pub level: Level,
    /// Which subsystem: "governor", "storm", "facet", "terminal", "prism".
    /// Owned only when read back from disk.
    pub source: Cow<'static, str>,
    pub message: String,
    /// Optional structured extras, for a detail view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Default)]
pub struct EventLog {
    events: RwLock<VecDeque<Event>>,
    disk: Option<SyncSender<Event>>,
}

impl EventLog {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(VecDeque::with_capacity(CAPACITY)),
            disk: None,
        }
    }

    /// A log that also lives at [path], starting from what was there: the
    /// events from before a crash or a reset are the first thing the Timeline
    /// shows. A file that can't be opened leaves the log in memory only, said
    /// as its first event, never as a failure to start.
    pub fn durable(path: &Path) -> Self {
        let mut events = VecDeque::with_capacity(CAPACITY);
        for e in read_back(path) {
            if events.len() >= CAPACITY { events.pop_front(); }
            events.push_back(e);
        }
        let mut log = Self { events: RwLock::new(events), disk: None };
        match writer(path.to_path_buf()) {
            Ok(tx) => log.disk = Some(tx),
            Err(e) => log.push_detailed(Level::Warn, "prism", "the timeline is kept in memory only", Some(format!("{}: {e}", path.display()))),
        }
        log
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
        let event = Event {
            unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            level,
            source: Cow::Borrowed(source),
            message: message.into(),
            detail,
        };
        // Never waits: a full queue means the disk is the thing stalling, and
        // the event is still kept in memory.
        if let Some(tx) = &self.disk {
            let _ = tx.try_send(event.clone());
        }
        let mut events = self.events.write().expect("event log poisoned");
        if events.len() >= CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
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

fn previous(path: &Path) -> PathBuf { path.with_extension("1.jsonl") }

/// What a durable log left behind, oldest first: the rotated file, then the
/// current one. A torn last line (the power went mid-write) is skipped.
fn read_back(path: &Path) -> Vec<Event> {
    let mut out = Vec::new();
    for p in [previous(path), path.to_path_buf()] {
        if let Ok(text) = std::fs::read_to_string(&p) {
            out.extend(text.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()));
        }
    }
    let skip = out.len().saturating_sub(CAPACITY);
    out.drain(..skip);
    out
}

fn writer(path: PathBuf) -> std::io::Result<SyncSender<Event>> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let open = |p: &Path| std::fs::OpenOptions::new().create(true).append(true).open(p);
    let mut file = open(&path)?;
    let (tx, rx) = sync_channel::<Event>(256);
    std::thread::Builder::new().name("prism-timeline".into()).spawn(move || {
        for e in rx {
            let Ok(mut line) = serde_json::to_string(&e) else { continue };
            line.push('\n');
            if file.write_all(line.as_bytes()).is_err() { continue }
            if e.level != Level::Info { let _ = file.sync_data(); }
            if file.metadata().map(|m| m.len()).unwrap_or(0) > FILE_LIMIT {
                let _ = file.sync_data();
                let _ = std::fs::rename(&path, previous(&path));
                match open(&path) { Ok(f) => file = f, Err(_) => return }
            }
        }
    })?;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_durable_log_comes_back_after_a_restart_and_stays_bounded_on_disk() {
        let dir = std::env::temp_dir().join(format!("prism-events-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("timeline.jsonl");
        {
            let log = EventLog::durable(&path);
            log.push(Level::Info, "governor", "before");
            log.push_detailed(Level::Action, "storm", "contained", Some("12 processes".into()));
        }
        // The writer thread drains its queue; give it a moment.
        for _ in 0..50 {
            if read_back(&path).len() == 2 { break }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // A torn line from a power cut is skipped, not fatal.
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"unix\":1,\"lev").unwrap();
        let again = EventLog::durable(&path);
        let r = again.recent(10);
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!((r[0].message.as_str(), r[0].source.as_ref(), r[0].level), ("contained", "storm", Level::Action));
        // Rotation: past the limit the file moves aside, one previous kept.
        let big = "x".repeat(4096);
        for _ in 0..300 { again.push(Level::Warn, "test", big.clone()); }
        for _ in 0..100 {
            if previous(&path).exists() { break }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(previous(&path).exists());
        assert!(std::fs::metadata(&path).unwrap().len() <= FILE_LIMIT + 8192);
        let _ = std::fs::remove_dir_all(&dir);
    }

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
