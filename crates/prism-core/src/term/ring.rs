//! Bounded scrollback.
//!
//! A detached session retains recent output so reattaching shows context rather
//! than a blank screen. That buffer must be bounded: a chatty process — a
//! training loop, a build, anything with a progress bar — would otherwise grow
//! it without limit.
//!
//! Prism is a daemon whose entire purpose is preventing memory exhaustion. An
//! unbounded buffer here would be that failure, caused by the thing built to
//! stop it. The cap is not a nicety.
//!
//! When the cap is exceeded the *oldest* bytes are discarded, because a terminal
//! reattach wants the most recent output. The count of discarded bytes is kept
//! so the UI can say so rather than silently presenting a truncated history as
//! if it were complete.

use std::collections::VecDeque;

pub const DEFAULT_CAPACITY: usize = 256 * 1024;

#[derive(Debug)]
pub struct Ring {
    buf: VecDeque<u8>,
    cap: usize,
    dropped: u64,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::new(),
            // A zero-capacity ring would divide the logic by zero cases; treat
            // it as "retain nothing" explicitly instead.
            cap,
            dropped: 0,
        }
    }

    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.cap == 0 {
            self.dropped += bytes.len() as u64;
            return;
        }

        // A single write larger than the whole buffer: keep only its tail.
        if bytes.len() >= self.cap {
            let keep = &bytes[bytes.len() - self.cap..];
            self.dropped += (self.buf.len() + (bytes.len() - self.cap)) as u64;
            self.buf.clear();
            self.buf.extend(keep);
            return;
        }

        self.buf.extend(bytes);
        let overflow = self.buf.len().saturating_sub(self.cap);
        if overflow > 0 {
            self.buf.drain(..overflow);
            self.dropped += overflow as u64;
        }
    }

    /// Everything currently retained, oldest first.
    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Bytes discarded to stay within the cap. Non-zero means the history shown
    /// on reattach is incomplete, which the UI should admit.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Shrink the cap, discarding oldest bytes to fit.
    ///
    /// Used when the governor de-escalates: at Red tier, retaining a quarter of
    /// a megabyte per session is a luxury the machine cannot afford.
    pub fn set_capacity(&mut self, cap: usize) {
        self.cap = cap;
        let overflow = self.buf.len().saturating_sub(cap);
        if overflow > 0 {
            self.buf.drain(..overflow);
            self.dropped += overflow as u64;
        }
    }
}

impl Default for Ring {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_short_output_verbatim() {
        let mut r = Ring::new(64);
        r.push(b"hello ");
        r.push(b"world");
        assert_eq!(r.snapshot(), b"hello world");
        assert_eq!(r.dropped(), 0);
    }

    #[test]
    fn discards_oldest_when_full() {
        let mut r = Ring::new(8);
        r.push(b"aaaaaaaa"); // exactly full
        r.push(b"bcd");
        // Keeps the most recent 8 bytes — a reattach wants the newest output.
        assert_eq!(r.snapshot(), b"aaaaabcd");
        assert_eq!(r.len(), 8);
        assert_eq!(r.dropped(), 3);
    }

    #[test]
    fn never_exceeds_capacity_under_sustained_writes() {
        let mut r = Ring::new(100);
        for _ in 0..1000 {
            r.push(b"0123456789");
        }
        assert_eq!(r.len(), 100, "the cap is the whole point");
        assert!(r.dropped() > 9000);
    }

    #[test]
    fn a_single_oversized_write_keeps_its_tail() {
        let mut r = Ring::new(10);
        r.push(b"xxxx");
        r.push(b"ABCDEFGHIJKLMNOP"); // 16 bytes into a 10-byte ring
        assert_eq!(r.snapshot(), b"GHIJKLMNOP");
        assert_eq!(r.len(), 10);
    }

    #[test]
    fn zero_capacity_retains_nothing_and_does_not_panic() {
        let mut r = Ring::new(0);
        r.push(b"anything at all");
        assert!(r.is_empty());
        assert_eq!(r.dropped(), 15);
    }

    #[test]
    fn empty_ring_snapshots_empty() {
        let r = Ring::new(64);
        assert!(r.snapshot().is_empty());
        assert!(r.is_empty());
    }

    #[test]
    fn shrinking_capacity_drops_oldest_to_fit() {
        // The de-escalation path: Red tier shrinks retention.
        let mut r = Ring::new(100);
        r.push(&[b'z'; 100]);
        r.set_capacity(10);
        assert_eq!(r.len(), 10);
        assert_eq!(r.capacity(), 10);
        assert_eq!(r.dropped(), 90);
    }

    #[test]
    fn growing_capacity_keeps_existing_content() {
        let mut r = Ring::new(10);
        r.push(b"0123456789");
        r.set_capacity(100);
        assert_eq!(r.snapshot(), b"0123456789");
        assert_eq!(r.dropped(), 0);
    }

    #[test]
    fn binary_output_is_preserved_byte_for_byte() {
        // Terminal output is escape sequences, not text; no lossy handling.
        let mut r = Ring::new(64);
        let esc = b"\x1b[2J\x1b[H\x00\xff\xfe";
        r.push(esc);
        assert_eq!(r.snapshot(), esc);
    }

    #[test]
    fn clear_empties_but_keeps_the_dropped_tally() {
        let mut r = Ring::new(4);
        r.push(b"abcdefgh");
        let before = r.dropped();
        r.clear();
        assert!(r.is_empty());
        assert_eq!(r.dropped(), before, "history of loss must survive a clear");
    }
}
