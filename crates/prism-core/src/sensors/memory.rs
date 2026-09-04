//! Memory sensing, including the honest-headroom correction.
//!
//! On a zram-backed system the kernel's own accounting is misleading: `SwapFree`
//! counts capacity that, when used, consumes the very resource it claims to
//! provide. Swapping out `X` bytes frees `X` but costs `X/ratio` to store the
//! compressed copy, so the true net gain is `X * (1 - 1/ratio)`.
//!
//! Sizing zram at 1.5x RAM therefore advertises headroom that cannot exist, and
//! because free swap remains the OOM killer never fires — the machine thrashes
//! indefinitely instead of shedding a process. Every threshold in Prism is
//! expressed against the corrected figure rather than against `free`.

use std::time::Instant;

/// Assumed compression ratio before any real data has been observed.
///
/// Deliberately pessimistic: zstd on anonymous pages typically achieves ~2.5-3x,
/// so 2.0 under-credits available headroom. Erring low means Prism intervenes
/// slightly early rather than slightly late.
const ASSUMED_COMPRESSION_RATIO: f64 = 2.0;

#[derive(Debug, Clone, Default)]
pub struct MemorySnapshot {
    pub total_kb: u64,
    pub available_kb: u64,
    pub free_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
    /// RAM currently spent storing compressed swap pages.
    pub zram_cost_kb: u64,
    /// Observed compression ratio across all zram devices, if any hold data.
    pub compression_ratio: Option<f64>,
    /// `available_kb` plus the *net* headroom remaining swap can actually yield.
    pub honest_headroom_kb: u64,
}

impl MemorySnapshot {
    /// How much of what the kernel advertises as free is illusory.
    ///
    /// This is the number that explains an unrecoverable hang after the fact.
    pub fn phantom_headroom_kb(&self) -> u64 {
        let naive = self.available_kb.saturating_add(self.swap_free_kb);
        naive.saturating_sub(self.honest_headroom_kb)
    }

    pub fn honest_headroom_gib(&self) -> f64 {
        self.honest_headroom_kb as f64 / 1_048_576.0
    }
}

pub fn sample() -> std::io::Result<MemorySnapshot> {
    let meminfo = std::fs::read_to_string("/proc/meminfo")?;
    let mut snap = MemorySnapshot {
        total_kb: field(&meminfo, "MemTotal:").unwrap_or(0),
        available_kb: field(&meminfo, "MemAvailable:").unwrap_or(0),
        free_kb: field(&meminfo, "MemFree:").unwrap_or(0),
        swap_total_kb: field(&meminfo, "SwapTotal:").unwrap_or(0),
        swap_free_kb: field(&meminfo, "SwapFree:").unwrap_or(0),
        ..Default::default()
    };

    let zram = zram_totals();
    snap.zram_cost_kb = zram.mem_used_kb;
    snap.compression_ratio = zram.ratio();

    // Credit each swap device with only the headroom it can genuinely deliver.
    // Disk swap yields its free space in full; zram yields the compressed
    // fraction. Machines with no swap at all fall through with zero credit.
    let ratio = snap.compression_ratio.unwrap_or(ASSUMED_COMPRESSION_RATIO);
    let zram_efficiency = (1.0 - 1.0 / ratio).max(0.0);

    let mut effective_swap_kb = 0f64;
    let mut classified_free_kb = 0u64;
    for dev in swap_devices() {
        classified_free_kb += dev.free_kb;
        if dev.is_zram {
            effective_swap_kb += dev.free_kb as f64 * zram_efficiency;
        } else {
            effective_swap_kb += dev.free_kb as f64;
        }
    }

    // If /proc/swaps could not be read but meminfo reports swap, fall back to
    // treating the unclassified remainder conservatively (as if it were zram).
    if snap.swap_free_kb > classified_free_kb {
        let unclassified = snap.swap_free_kb - classified_free_kb;
        effective_swap_kb += unclassified as f64 * zram_efficiency;
    }

    snap.honest_headroom_kb = snap.available_kb + effective_swap_kb as u64;
    Ok(snap)
}

fn field(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

struct SwapDevice {
    is_zram: bool,
    free_kb: u64,
}

fn swap_devices() -> Vec<SwapDevice> {
    let Ok(contents) = std::fs::read_to_string("/proc/swaps") else {
        return Vec::new();
    };
    contents
        .lines()
        .skip(1) // header
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let name = cols.next()?;
            let _type = cols.next()?;
            let size_kb: u64 = cols.next()?.parse().ok()?;
            let used_kb: u64 = cols.next()?.parse().ok()?;
            Some(SwapDevice {
                is_zram: name.starts_with("/dev/zram"),
                free_kb: size_kb.saturating_sub(used_kb),
            })
        })
        .collect()
}

#[derive(Default)]
struct ZramTotals {
    orig_bytes: u64,
    compr_bytes: u64,
    mem_used_kb: u64,
}

impl ZramTotals {
    fn ratio(&self) -> Option<f64> {
        // Require a meaningful sample; a nearly-empty device yields a ratio
        // dominated by allocator overhead rather than real compressibility.
        const MIN_SAMPLE_BYTES: u64 = 16 * 1024 * 1024;
        if self.compr_bytes == 0 || self.orig_bytes < MIN_SAMPLE_BYTES {
            return None;
        }
        Some(self.orig_bytes as f64 / self.compr_bytes as f64)
    }
}

/// Aggregate `mm_stat` across every zram device present.
///
/// Fields are: orig_data_size compr_data_size mem_used_total mem_limit
/// mem_used_max same_pages pages_compacted huge_pages — all in bytes.
fn zram_totals() -> ZramTotals {
    let mut totals = ZramTotals::default();
    let Ok(entries) = std::fs::read_dir("/sys/block") else {
        return totals;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("zram") {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(entry.path().join("mm_stat")) else {
            continue;
        };
        let vals: Vec<u64> = stat
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        if vals.len() >= 3 {
            totals.orig_bytes += vals[0];
            totals.compr_bytes += vals[1];
            totals.mem_used_kb += vals[2] / 1024;
        }
    }
    totals
}

// ---------------------------------------------------------------------------
// Pressure Stall Information
// ---------------------------------------------------------------------------

/// Raw PSI counters, in microseconds of stall since boot.
#[derive(Debug, Clone, Copy, Default)]
pub struct PsiRaw {
    pub some_total_us: u64,
    pub full_total_us: u64,
}

/// Stall as a fraction of wall time over the sampling interval, 0.0..=1.0.
#[derive(Debug, Clone, Copy, Default)]
pub struct PsiStall {
    pub some: f64,
    pub full: f64,
}

pub fn read_psi(path: &str) -> Option<PsiRaw> {
    let contents = std::fs::read_to_string(path).ok()?;
    let mut raw = PsiRaw::default();
    for line in contents.lines() {
        let total = line
            .split_whitespace()
            .find_map(|tok| tok.strip_prefix("total=")?.parse::<u64>().ok());
        let Some(total) = total else { continue };
        if line.starts_with("some") {
            raw.some_total_us = total;
        } else if line.starts_with("full") {
            raw.full_total_us = total;
        }
    }
    Some(raw)
}

/// Derives instantaneous stall from successive `total=` counters.
///
/// The kernel's own `avg10` is deliberately not used: it lags by ~10s, and the
/// failure this guards against compounds exponentially. A detector keyed on
/// `avg10` reliably notices a spiral only once it is unrecoverable.
#[derive(Default)]
pub struct PsiTracker {
    last: Option<(PsiRaw, Instant)>,
}

impl PsiTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `None` on the first sample, when no delta exists yet.
    pub fn update(&mut self, raw: PsiRaw) -> Option<PsiStall> {
        self.update_at(raw, Instant::now())
    }

    /// As [`Self::update`], with the sample time supplied.
    ///
    /// Two samples taken within the same microsecond yield no measurable
    /// interval and are reported as `None` rather than dividing by zero.
    pub fn update_at(&mut self, raw: PsiRaw, now: Instant) -> Option<PsiStall> {
        let stall = self.last.and_then(|(prev, then)| {
            let elapsed_us = now.duration_since(then).as_micros() as f64;
            if elapsed_us <= 0.0 {
                return None;
            }
            Some(PsiStall {
                some: (raw.some_total_us.saturating_sub(prev.some_total_us) as f64 / elapsed_us)
                    .clamp(0.0, 1.0),
                full: (raw.full_total_us.saturating_sub(prev.full_total_us) as f64 / elapsed_us)
                    .clamp(0.0, 1.0),
            })
        });
        self.last = Some((raw, now));
        stall
    }
}

/// Convenience: sample memory PSI from the system-wide file.
pub fn sample_psi() -> Option<PsiRaw> {
    read_psi("/proc/pressure/memory")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &str = "\
MemTotal:       32000000 kB
MemFree:         1000000 kB
MemAvailable:    2000000 kB
SwapTotal:      48000000 kB
SwapFree:       32000000 kB
";

    #[test]
    fn parses_meminfo_fields() {
        assert_eq!(field(MEMINFO, "MemTotal:"), Some(32_000_000));
        assert_eq!(field(MEMINFO, "MemAvailable:"), Some(2_000_000));
        assert_eq!(field(MEMINFO, "SwapFree:"), Some(32_000_000));
        assert_eq!(field(MEMINFO, "Nonexistent:"), None);
    }

    #[test]
    fn meminfo_prefix_is_not_confused_by_similar_keys() {
        // "MemFree:" must not match "MemAvailable:" or vice versa.
        assert_eq!(field(MEMINFO, "MemFree:"), Some(1_000_000));
    }

    #[test]
    fn ratio_ignores_undersized_samples() {
        let tiny = ZramTotals {
            orig_bytes: 1024,
            compr_bytes: 512,
            mem_used_kb: 1,
        };
        assert!(tiny.ratio().is_none());

        let real = ZramTotals {
            orig_bytes: 1024 * 1024 * 1024,
            compr_bytes: 512 * 1024 * 1024,
            mem_used_kb: 0,
        };
        assert_eq!(real.ratio(), Some(2.0));
    }

    #[test]
    fn psi_tracker_needs_two_samples() {
        let mut tracker = PsiTracker::new();
        let t0 = Instant::now();
        assert!(tracker.update_at(PsiRaw::default(), t0).is_none());
        assert!(
            tracker
                .update_at(
                    PsiRaw {
                        some_total_us: 100,
                        full_total_us: 50,
                    },
                    t0 + std::time::Duration::from_secs(1)
                )
                .is_some()
        );
    }

    #[test]
    fn psi_stall_is_a_fraction_of_elapsed_time() {
        let mut tracker = PsiTracker::new();
        let t0 = Instant::now();
        tracker.update_at(PsiRaw::default(), t0);
        // Half a second of stall across a one-second interval is 50%.
        let stall = tracker
            .update_at(
                PsiRaw {
                    some_total_us: 500_000,
                    full_total_us: 250_000,
                },
                t0 + std::time::Duration::from_secs(1),
            )
            .unwrap();
        assert!((stall.some - 0.5).abs() < 1e-6);
        assert!((stall.full - 0.25).abs() < 1e-6);
    }

    #[test]
    fn psi_stall_is_clamped_to_unit_interval() {
        let mut tracker = PsiTracker::new();
        let t0 = Instant::now();
        tracker.update_at(PsiRaw::default(), t0);
        // A counter jump larger than the interval must not exceed 100%.
        let stall = tracker
            .update_at(
                PsiRaw {
                    some_total_us: u64::MAX / 2,
                    full_total_us: u64::MAX / 2,
                },
                t0 + std::time::Duration::from_secs(1),
            )
            .unwrap();
        assert!(stall.some <= 1.0 && stall.full <= 1.0);
    }

    #[test]
    fn zero_interval_yields_no_reading_rather_than_dividing_by_zero() {
        let mut tracker = PsiTracker::new();
        let t0 = Instant::now();
        tracker.update_at(PsiRaw::default(), t0);
        assert!(tracker.update_at(PsiRaw::default(), t0).is_none());
    }

    #[test]
    fn phantom_headroom_is_the_gap_between_naive_and_honest() {
        let snap = MemorySnapshot {
            available_kb: 1000,
            swap_free_kb: 1000,
            honest_headroom_kb: 1500,
            ..Default::default()
        };
        assert_eq!(snap.phantom_headroom_kb(), 500);
    }
}
