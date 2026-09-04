//! Directory listing.
//!
//! The performance rule from ADR 0001 is that listing must not be N+1. A models
//! directory with 100k entries is normal here, and calling `stat` on every one
//! to render a page of fifty is the difference between instant and unusable.
//!
//! So the scan reads names via `read_dir` — which on Linux gets the file type
//! from `d_type` without a `stat` — sorts on that cheap information, and calls
//! `stat` **only on the slice actually being returned**.
//!
//! Sorting by size or modification time is the exception: those fields do not
//! exist until something is stat'd, so that ordering necessarily pays for the
//! whole directory. The cost is explicit in [`Sort::requires_full_stat`] rather
//! than hidden, so a caller can decide whether to offer it.

use serde::Serialize;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Coarse category, inferred from the extension alone.
///
/// Deliberately not content sniffing: opening every file in a directory to read
/// magic bytes is exactly the N+1 this module exists to avoid. The UI uses this
/// to pick an icon and to decide whether a thumbnail is even worth requesting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Dir,
    Image,
    Video,
    Audio,
    Text,
    Pdf,
    Archive,
    Other,
}

impl Kind {
    pub fn from_extension(name: &str) -> Self {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "bmp" | "tif" | "tiff" | "heic"
            | "svg" => Kind::Image,
            "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" => {
                Kind::Video
            }
            "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" | "wma" => Kind::Audio,
            "txt" | "md" | "rs" | "py" | "js" | "ts" | "json" | "toml" | "yaml" | "yml" | "html"
            | "css" | "sh" | "c" | "h" | "cpp" | "go" | "lua" | "conf" | "log" | "csv" | "xml" => {
                Kind::Text
            }
            "pdf" => Kind::Pdf,
            "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "rar" => Kind::Archive,
            _ => Kind::Other,
        }
    }

    /// Whether a thumbnail could be produced. Saves the UI requesting one for a
    /// file that could never have it.
    pub fn thumbnailable(&self) -> bool {
        matches!(self, Kind::Image | Kind::Video | Kind::Pdf)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub is_dir: bool,
    /// True if the entry is a symlink, whatever it points at.
    pub is_link: bool,
    /// `None` for a directory, or a link whose target cannot be stat'd.
    pub size: Option<u64>,
    /// Unix seconds. `None` if unavailable.
    pub modified: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// Total entries in the directory, before pagination.
    pub total: usize,
    pub offset: usize,
    /// True if any entry could not be read; the listing is still returned.
    pub partial: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// Directories first, then case-insensitive by name. Needs no `stat`.
    #[default]
    Name,
    Size,
    Modified,
}

impl Sort {
    /// True if this ordering forces a `stat` of the entire directory.
    pub fn requires_full_stat(&self) -> bool {
        matches!(self, Sort::Size | Sort::Modified)
    }
}

/// Cheap pre-stat record: everything `read_dir` gives us for free.
struct Shallow {
    name: String,
    is_dir: bool,
    is_link: bool,
}

pub fn list(dir: &Path, offset: usize, limit: usize, sort: Sort) -> std::io::Result<Listing> {
    let mut shallow: Vec<Shallow> = Vec::new();
    let mut partial = false;

    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            partial = true;
            continue;
        };
        // `file_type` here comes from d_type and does not stat.
        let Ok(ft) = entry.file_type() else {
            partial = true;
            continue;
        };
        let is_link = ft.is_symlink();
        // A symlink's d_type says "link", not what it points at. Resolving that
        // needs a stat, so it is deferred to the page slice below; until then a
        // link is provisionally treated as a file for ordering purposes.
        shallow.push(Shallow {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir: ft.is_dir(),
            is_link,
        });
    }

    let total = shallow.len();

    if sort.requires_full_stat() {
        // Explicitly the expensive path: every entry must be stat'd to be
        // ordered at all.
        let mut full: Vec<Entry> = shallow
            .into_iter()
            .map(|s| hydrate(dir, s))
            .collect();
        match sort {
            Sort::Size => full.sort_by(|a, b| b.size.cmp(&a.size)),
            Sort::Modified => full.sort_by(|a, b| b.modified.cmp(&a.modified)),
            Sort::Name => unreachable!(),
        }
        let entries = full.into_iter().skip(offset).take(limit).collect();
        return Ok(Listing {
            entries,
            total,
            offset,
            partial,
        });
    }

    // Cheap path: order on names alone, then stat only what is returned.
    shallow.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let entries = shallow
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|s| hydrate(dir, s))
        .collect();

    Ok(Listing {
        entries,
        total,
        offset,
        partial,
    })
}

/// Attach `stat`-derived fields to one entry.
fn hydrate(dir: &Path, s: Shallow) -> Entry {
    // Follows symlinks deliberately: the operator wants the target's size, and
    // a link resolving outside a root is refused at access time by
    // `path::resolve`, not here. A broken link simply yields no metadata.
    let meta = std::fs::metadata(dir.join(&s.name)).ok();
    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(s.is_dir);

    Entry {
        kind: if is_dir {
            Kind::Dir
        } else {
            Kind::from_extension(&s.name)
        },
        size: meta.as_ref().filter(|m| !m.is_dir()).map(|m| m.len()),
        modified: meta.as_ref().and_then(|m| {
            m.modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        }),
        is_dir,
        is_link: s.is_link,
        name: s.name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "prism-list-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }
        fn file(&self, name: &str, bytes: usize) -> &Self {
            fs::write(self.dir.join(name), vec![b'x'; bytes]).unwrap();
            self
        }
        fn subdir(&self, name: &str) -> &Self {
            fs::create_dir_all(self.dir.join(name)).unwrap();
            self
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn classifies_by_extension() {
        assert_eq!(Kind::from_extension("a.JPG"), Kind::Image);
        assert_eq!(Kind::from_extension("film.mkv"), Kind::Video);
        assert_eq!(Kind::from_extension("song.flac"), Kind::Audio);
        assert_eq!(Kind::from_extension("notes.md"), Kind::Text);
        assert_eq!(Kind::from_extension("doc.pdf"), Kind::Pdf);
        assert_eq!(Kind::from_extension("model.safetensors"), Kind::Other);
        assert_eq!(Kind::from_extension("no_extension"), Kind::Other);
    }

    #[test]
    fn only_previewable_kinds_are_thumbnailable() {
        assert!(Kind::Image.thumbnailable());
        assert!(Kind::Video.thumbnailable());
        assert!(Kind::Pdf.thumbnailable());
        assert!(!Kind::Audio.thumbnailable());
        assert!(!Kind::Other.thumbnailable());
        assert!(!Kind::Dir.thumbnailable());
    }

    #[test]
    fn directories_sort_before_files() {
        let f = Fixture::new("order");
        f.file("aaa.txt", 1).file("zzz.txt", 1).subdir("mmm");
        let l = list(&f.dir, 0, 50, Sort::Name).unwrap();
        assert_eq!(l.entries[0].name, "mmm");
        assert!(l.entries[0].is_dir);
    }

    #[test]
    fn name_sort_is_case_insensitive() {
        let f = Fixture::new("case");
        f.file("Beta.txt", 1).file("alpha.txt", 1);
        let l = list(&f.dir, 0, 50, Sort::Name).unwrap();
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha.txt", "Beta.txt"]);
    }

    #[test]
    fn paginates_and_reports_the_true_total() {
        let f = Fixture::new("page");
        for i in 0..25 {
            f.file(&format!("f{i:02}.txt"), 1);
        }
        let page = list(&f.dir, 10, 5, Sort::Name).unwrap();
        assert_eq!(page.entries.len(), 5);
        assert_eq!(page.total, 25, "total must count the directory, not the page");
        assert_eq!(page.offset, 10);
        assert_eq!(page.entries[0].name, "f10.txt");
    }

    #[test]
    fn offset_beyond_the_end_yields_an_empty_page_not_an_error() {
        let f = Fixture::new("over");
        f.file("only.txt", 1);
        let l = list(&f.dir, 500, 10, Sort::Name).unwrap();
        assert!(l.entries.is_empty());
        assert_eq!(l.total, 1);
    }

    #[test]
    fn reports_sizes_for_files_and_none_for_directories() {
        let f = Fixture::new("size");
        f.file("data.bin", 1234).subdir("adir");
        let l = list(&f.dir, 0, 50, Sort::Name).unwrap();
        let dir_entry = l.entries.iter().find(|e| e.name == "adir").unwrap();
        let file_entry = l.entries.iter().find(|e| e.name == "data.bin").unwrap();
        assert_eq!(dir_entry.size, None);
        assert_eq!(file_entry.size, Some(1234));
    }

    #[test]
    fn size_sort_orders_largest_first() {
        let f = Fixture::new("bysize");
        f.file("small.bin", 10).file("big.bin", 5000).file("mid.bin", 500);
        let l = list(&f.dir, 0, 50, Sort::Size).unwrap();
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["big.bin", "mid.bin", "small.bin"]);
    }

    #[test]
    fn sort_declares_whether_it_costs_a_full_stat() {
        assert!(!Sort::Name.requires_full_stat());
        assert!(Sort::Size.requires_full_stat());
        assert!(Sort::Modified.requires_full_stat());
    }

    #[test]
    fn marks_symlinks_and_survives_broken_ones() {
        let f = Fixture::new("links");
        f.file("real.txt", 5);
        std::os::unix::fs::symlink(f.dir.join("real.txt"), f.dir.join("good")).unwrap();
        std::os::unix::fs::symlink(f.dir.join("gone.txt"), f.dir.join("broken")).unwrap();

        let l = list(&f.dir, 0, 50, Sort::Name).unwrap();
        let good = l.entries.iter().find(|e| e.name == "good").unwrap();
        let broken = l.entries.iter().find(|e| e.name == "broken").unwrap();

        assert!(good.is_link && good.size == Some(5));
        assert!(broken.is_link, "a broken link must still be listed");
        assert_eq!(broken.size, None, "and must not claim a size");
    }

    #[test]
    fn empty_directory_lists_cleanly() {
        let f = Fixture::new("empty");
        let l = list(&f.dir, 0, 50, Sort::Name).unwrap();
        assert_eq!(l.total, 0);
        assert!(!l.partial);
    }

    #[test]
    fn missing_directory_is_an_error() {
        assert!(list(Path::new("/nonexistent/xyzzy"), 0, 50, Sort::Name).is_err());
    }
}
