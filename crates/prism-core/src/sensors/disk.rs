//! Filesystem capacity sensing.
//!
//! Added after the survey in ADR 0001 noticed that `/home` on the host this was
//! written for sits at 95% (44 GiB free) while Prism senses only memory. That is
//! the wrong emphasis: with image generation writing outputs and language models
//! measured in gigabytes, the machine is considerably closer to exhausting disk
//! than RAM, and a full filesystem wedges a desktop session as effectively as a
//! thrash spiral — applications fail to write state, logs stop, and the
//! compositor can lose its own runtime files.
//!
//! Free space is reported as `bavail` (blocks available to unprivileged users)
//! rather than `bfree`. The two differ by the root reserve, typically 5%, and
//! only `bavail` answers the question that matters: *can this process actually
//! write?* Reporting `bfree` would overstate headroom in exactly the way
//! `SwapFree` overstates memory — the same class of comfortable lie this project
//! already corrects for once.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct MountUsage {
    pub path: PathBuf,
    pub total_kb: u64,
    /// Available to unprivileged writers, excluding the root reserve.
    pub available_kb: u64,
    pub inodes_total: u64,
    pub inodes_free: u64,
}

impl MountUsage {
    pub fn used_kb(&self) -> u64 {
        self.total_kb.saturating_sub(self.available_kb)
    }

    /// Percentage of usable capacity consumed, 0.0..=100.0.
    pub fn used_pct(&self) -> f64 {
        if self.total_kb == 0 {
            return 0.0;
        }
        (self.used_kb() as f64 / self.total_kb as f64) * 100.0
    }

    pub fn available_mib(&self) -> u64 {
        self.available_kb / 1024
    }

    /// Inode exhaustion is a distinct failure: a filesystem can be 40% empty by
    /// bytes and still refuse to create a file. Thumbnail and model caches
    /// generate many small files, which is exactly the workload that hits it.
    pub fn inodes_used_pct(&self) -> f64 {
        if self.inodes_total == 0 {
            return 0.0; // btrfs and friends report 0; not a failure signal.
        }
        let used = self.inodes_total.saturating_sub(self.inodes_free);
        (used as f64 / self.inodes_total as f64) * 100.0
    }
}

/// Sample one path's filesystem.
///
/// Returns `None` for a path that cannot be stat'd — an unmounted or removed
/// root should drop out of monitoring rather than abort the whole sample.
pub fn sample_path(path: &Path) -> Option<MountUsage> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;

    // SAFETY: `statvfs` writes into a struct we own and reads a NUL-terminated
    // path we keep alive for the call. A bad path returns -1 rather than
    // touching memory.
    let stat = unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) != 0 {
            return None;
        }
        stat
    };

    // f_frsize is the fragment size and the correct multiplier for block counts;
    // f_bsize is the preferred I/O size and is not necessarily the same.
    let block = if stat.f_frsize > 0 {
        stat.f_frsize as u64
    } else {
        stat.f_bsize as u64
    };

    Some(MountUsage {
        path: path.to_path_buf(),
        total_kb: (stat.f_blocks as u64).saturating_mul(block) / 1024,
        available_kb: (stat.f_bavail as u64).saturating_mul(block) / 1024,
        inodes_total: stat.f_files as u64,
        inodes_free: stat.f_ffree as u64,
    })
}

/// Sample several paths, dropping any that cannot be read.
pub fn sample(paths: &[PathBuf]) -> Vec<MountUsage> {
    paths.iter().filter_map(|p| sample_path(p)).collect()
}

/// The tightest mount among those sampled, by available bytes.
///
/// The governor keys on the worst mount rather than an average: filling any one
/// filesystem breaks the things that live on it, and averaging a full `/home`
/// against an empty `/boot` would hide precisely the condition worth acting on.
pub fn tightest(usages: &[MountUsage]) -> Option<&MountUsage> {
    usages.iter().min_by_key(|u| u.available_kb)
}

/// Paths worth watching when the operator has configured none.
///
/// `/` and `$HOME` cover the cases that actually wedge a session. Missing paths
/// are dropped by [`sample`], so listing a path that does not exist is harmless.
pub fn default_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/")];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if home != Path::new("/") {
            paths.push(home);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(total_kb: u64, available_kb: u64) -> MountUsage {
        MountUsage {
            path: "/test".into(),
            total_kb,
            available_kb,
            inodes_total: 0,
            inodes_free: 0,
        }
    }

    #[test]
    fn samples_a_real_filesystem() {
        let root = sample_path(Path::new("/")).expect("/ is always mountable");
        assert!(root.total_kb > 0, "root filesystem must report a size");
        assert!(root.available_kb <= root.total_kb);
    }

    #[test]
    fn missing_path_yields_none_rather_than_panicking() {
        assert!(sample_path(Path::new("/nonexistent/xyzzy/nope")).is_none());
    }

    #[test]
    fn sample_drops_unreadable_paths() {
        let paths = vec![PathBuf::from("/"), PathBuf::from("/nonexistent/xyzzy")];
        assert_eq!(sample(&paths).len(), 1);
    }

    #[test]
    fn used_percentage_is_computed_from_available_not_free() {
        // 100 GiB total, 5 GiB available -> 95% used, matching what `df` shows
        // and what the operator actually experiences.
        let u = usage(100 * 1024 * 1024, 5 * 1024 * 1024);
        assert!((u.used_pct() - 95.0).abs() < 0.01);
    }

    #[test]
    fn empty_filesystem_reports_zero_used() {
        let u = usage(1000, 1000);
        assert_eq!(u.used_pct(), 0.0);
    }

    #[test]
    fn zero_sized_filesystem_does_not_divide_by_zero() {
        let u = usage(0, 0);
        assert_eq!(u.used_pct(), 0.0);
        assert_eq!(u.inodes_used_pct(), 0.0);
    }

    #[test]
    fn filesystems_reporting_no_inodes_are_not_flagged() {
        // btrfs reports f_files = 0; that must read as 0% used, not 100%.
        let u = usage(1000, 500);
        assert_eq!(u.inodes_used_pct(), 0.0);
    }

    #[test]
    fn inode_pressure_is_detected_independently_of_bytes() {
        let mut u = usage(100 * 1024 * 1024, 90 * 1024 * 1024); // 10% full by bytes
        u.inodes_total = 1000;
        u.inodes_free = 10; // 99% full by inodes
        assert!(u.used_pct() < 15.0);
        assert!(
            u.inodes_used_pct() > 95.0,
            "a filesystem can be empty by bytes and unable to create a file"
        );
    }

    #[test]
    fn tightest_picks_the_worst_mount_not_the_average() {
        let mounts = vec![
            usage(100 * 1024 * 1024, 90 * 1024 * 1024),
            usage(100 * 1024 * 1024, 1024),
            usage(100 * 1024 * 1024, 50 * 1024 * 1024),
        ];
        assert_eq!(tightest(&mounts).unwrap().available_kb, 1024);
    }

    #[test]
    fn tightest_of_nothing_is_none() {
        assert!(tightest(&[]).is_none());
    }

    #[test]
    fn default_paths_include_root_and_are_sampleable() {
        let paths = default_paths();
        assert!(paths.contains(&PathBuf::from("/")));
        assert!(!sample(&paths).is_empty());
    }
}
