//! Confining requested paths to their root.
//!
//! This is the security boundary of the file manager. Everything else — listing,
//! previews, downloads — is built on the guarantee that a request cannot address
//! a byte outside a configured root.
//!
//! The rule is: **resolve fully, then verify containment.** Textual checks are
//! not sufficient, because the filesystem has three separate ways to leave a
//! directory that look innocent as strings:
//!
//! * `..` components, including ones hidden inside an otherwise-valid path
//! * absolute paths supplied where a relative one was expected
//! * **symlinks**, which are the important one — `root/link` can point anywhere,
//!   and no amount of string inspection reveals it
//!
//! So resolution goes through `canonicalize`, which follows every symlink and
//! resolves every `..` against the real filesystem, and containment is checked
//! on the result. A path that does not exist cannot be canonicalised, so
//! creation is handled by resolving the *parent* and appending a single
//! validated component.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// The named root is not configured.
    UnknownRoot,
    /// Resolved outside its root — traversal, symlink escape, or absolute path.
    Escapes,
    /// Does not exist.
    NotFound,
    /// A component was not a plain name (`..`, `/`, or an embedded NUL).
    InvalidComponent,
}

impl PathError {
    /// Deliberately uniform for `Escapes` and `NotFound`.
    ///
    /// Distinguishing them tells a caller whether a path outside the root
    /// exists, which is a filesystem oracle. The log records the real reason.
    pub fn public_message(&self) -> &'static str {
        match self {
            PathError::UnknownRoot => "no such root",
            PathError::Escapes | PathError::NotFound => "not found",
            PathError::InvalidComponent => "invalid path",
        }
    }
}

/// A root the file manager may serve.
#[derive(Debug, Clone)]
pub struct Root {
    pub name: String,
    /// Canonicalised at construction; comparisons rely on it being real.
    pub path: PathBuf,
    pub writable: bool,
}

impl Root {
    /// Canonicalise a configured root, rejecting one that does not exist.
    ///
    /// Resolving once at startup means every later containment check compares
    /// two already-real paths, rather than re-deriving the root each request.
    pub fn new(name: impl Into<String>, path: impl AsRef<Path>, writable: bool) -> Option<Self> {
        Some(Self {
            name: name.into(),
            path: path.as_ref().canonicalize().ok()?,
            writable,
        })
    }
}

/// Reject anything that is not a plain, single path segment.
///
/// Applied to each component of the *requested* relative path before it is
/// joined, so an obviously hostile request is refused before it ever touches the
/// filesystem. This is defence in depth: containment is still verified after
/// resolution, and that check is the one that must be correct.
fn is_plain_component(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.contains('/')
        && !part.contains('\\')
        && !part.contains('\0')
}

/// Resolve `relative` within `root`, guaranteeing the result is inside it.
///
/// The returned path is canonical: symlinks followed, `..` resolved. A symlink
/// inside the root that points outside it resolves to its target and is then
/// rejected by the containment check — which is the entire reason resolution
/// happens before verification rather than after.
pub fn resolve(root: &Root, relative: &str) -> Result<PathBuf, PathError> {
    let mut candidate = root.path.clone();

    for part in relative.split('/') {
        // Tolerate empty segments from leading, trailing or doubled slashes.
        if part.is_empty() || part == "." {
            continue;
        }
        if !is_plain_component(part) {
            return Err(PathError::InvalidComponent);
        }
        candidate.push(part);
    }

    let resolved = candidate.canonicalize().map_err(|_| PathError::NotFound)?;
    if !resolved.starts_with(&root.path) {
        return Err(PathError::Escapes);
    }
    Ok(resolved)
}

/// Express a resolved path relative to its root, for display and for building
/// links back into the API.
pub fn relative_to(root: &Root, resolved: &Path) -> String {
    resolved
        .strip_prefix(&root.path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Find a root by name.
pub fn find<'a>(roots: &'a [Root], name: &str) -> Result<&'a Root, PathError> {
    roots
        .iter()
        .find(|r| r.name == name)
        .ok_or(PathError::UnknownRoot)
}

/// True if `path` has no `..` or absolute components — used for paths that must
/// be validated *before* they exist, such as an upload destination.
pub fn is_safe_new_path(relative: &str) -> bool {
    let p = Path::new(relative);
    !relative.is_empty()
        && p.components().all(|c| matches!(c, Component::Normal(_)))
        && !relative.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        dir: PathBuf,
        root: Root,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "prism-path-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("root/sub")).unwrap();
            fs::write(dir.join("root/file.txt"), b"inside").unwrap();
            fs::write(dir.join("root/sub/nested.txt"), b"nested").unwrap();
            fs::create_dir_all(dir.join("outside")).unwrap();
            fs::write(dir.join("outside/secret.txt"), b"SECRET").unwrap();

            let root = Root::new("test", dir.join("root"), false).expect("root exists");
            Self { dir, root }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn resolves_a_file_in_the_root() {
        let f = Fixture::new("basic");
        let p = resolve(&f.root, "file.txt").unwrap();
        assert_eq!(fs::read_to_string(p).unwrap(), "inside");
    }

    #[test]
    fn resolves_a_nested_file() {
        let f = Fixture::new("nested");
        let p = resolve(&f.root, "sub/nested.txt").unwrap();
        assert_eq!(fs::read_to_string(p).unwrap(), "nested");
    }

    #[test]
    fn empty_path_resolves_to_the_root_itself() {
        let f = Fixture::new("empty");
        assert_eq!(resolve(&f.root, "").unwrap(), f.root.path);
    }

    #[test]
    fn tolerates_redundant_separators() {
        let f = Fixture::new("seps");
        for variant in ["/sub/nested.txt", "sub//nested.txt", "./sub/./nested.txt", "sub/nested.txt/"] {
            assert!(
                resolve(&f.root, variant).is_ok(),
                "{variant:?} should resolve"
            );
        }
    }

    #[test]
    fn rejects_dotdot_traversal() {
        let f = Fixture::new("dotdot");
        for attack in [
            "../outside/secret.txt",
            "sub/../../outside/secret.txt",
            "..",
            "sub/..",
        ] {
            assert_eq!(
                resolve(&f.root, attack),
                Err(PathError::InvalidComponent),
                "{attack:?} must be refused"
            );
        }
    }

    #[test]
    fn rejects_absolute_paths() {
        let f = Fixture::new("abs");
        // Leading slash is treated as an empty first segment, so this becomes a
        // lookup of `etc/passwd` inside the root, which does not exist.
        assert_eq!(resolve(&f.root, "/etc/passwd"), Err(PathError::NotFound));
    }

    #[test]
    fn rejects_embedded_nul() {
        let f = Fixture::new("nul");
        assert_eq!(
            resolve(&f.root, "file.txt\0.png"),
            Err(PathError::InvalidComponent)
        );
    }

    /// The important one: a symlink is invisible to any textual check.
    #[test]
    fn rejects_a_symlink_pointing_outside_the_root() {
        let f = Fixture::new("symlink");
        std::os::unix::fs::symlink(f.dir.join("outside/secret.txt"), f.root.path.join("escape"))
            .unwrap();

        assert_eq!(
            resolve(&f.root, "escape"),
            Err(PathError::Escapes),
            "a symlink out of the root must not be followed"
        );
    }

    #[test]
    fn rejects_a_symlinked_directory_pointing_outside() {
        let f = Fixture::new("symdir");
        std::os::unix::fs::symlink(f.dir.join("outside"), f.root.path.join("linkdir")).unwrap();
        assert_eq!(
            resolve(&f.root, "linkdir/secret.txt"),
            Err(PathError::Escapes)
        );
    }

    #[test]
    fn allows_a_symlink_that_stays_inside_the_root() {
        let f = Fixture::new("symin");
        std::os::unix::fs::symlink(f.root.path.join("sub/nested.txt"), f.root.path.join("ok"))
            .unwrap();
        let p = resolve(&f.root, "ok").unwrap();
        assert_eq!(fs::read_to_string(p).unwrap(), "nested");
    }

    #[test]
    fn missing_file_is_not_found() {
        let f = Fixture::new("missing");
        assert_eq!(resolve(&f.root, "nope.txt"), Err(PathError::NotFound));
    }

    #[test]
    fn escape_and_not_found_are_indistinguishable_to_a_caller() {
        // Otherwise the API becomes an oracle for what exists outside the root.
        assert_eq!(
            PathError::Escapes.public_message(),
            PathError::NotFound.public_message()
        );
    }

    #[test]
    fn relative_to_round_trips() {
        let f = Fixture::new("rel");
        let p = resolve(&f.root, "sub/nested.txt").unwrap();
        assert_eq!(relative_to(&f.root, &p), "sub/nested.txt");
    }

    #[test]
    fn unknown_root_is_reported() {
        let roots: Vec<Root> = Vec::new();
        assert_eq!(find(&roots, "nope").unwrap_err(), PathError::UnknownRoot);
    }

    #[test]
    fn nonexistent_root_cannot_be_constructed() {
        assert!(Root::new("bad", "/nonexistent/xyzzy", false).is_none());
    }

    #[test]
    fn new_path_validation_rejects_traversal_and_absolutes() {
        assert!(is_safe_new_path("a/b/c.txt"));
        assert!(!is_safe_new_path("../a.txt"));
        assert!(!is_safe_new_path("/etc/passwd"));
        assert!(!is_safe_new_path(""));
        assert!(!is_safe_new_path("a/../../b"));
        assert!(!is_safe_new_path("a\0b"));
    }
}
