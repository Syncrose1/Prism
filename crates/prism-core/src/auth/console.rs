//! Signing in from the machine Prism is running on.
//!
//! Asking for a phone to reach a service hosted on the desk in front of you is
//! friction that buys nothing, and this is why it buys nothing:
//!
//! > The authenticator secret is a file at `$STATE/totp.secret`, mode 0600.
//! > Anyone who can read it can mint codes forever.
//!
//! So *filesystem access as the owning user on this host* is already at least as
//! strong as the second factor. A console sign-in that proves exactly that
//! capability adds no exposure — it takes a path that was already open and makes
//! it convenient instead of pretending it is closed.
//!
//! ## Why not just trust loopback
//!
//! Because loopback is not a place. `enrol.rs` already says it, and the point
//! generalises: SSH forwards a port from anywhere, so a "local-only" route is
//! silently a remote route for anyone holding SSH. Loopback proves nothing about
//! who is asking.
//!
//! Reading a 0600 file in the user's state directory *is* a proof. A forwarded
//! port does not carry filesystem access, and anyone who has a shell as this
//! user could read `totp.secret` regardless.
//!
//! ## Why a grant, and not the key in the URL
//!
//! The obvious shortcut is `GET /auth/console?key=<the console key>`. It would
//! work, and the exposure argument above mostly holds — but a URL is the leakiest
//! thing on a computer. It sits in history, in the address bar over a shared
//! screen, in a `Referer` on the next click. A long-lived credential should never
//! be one.
//!
//! So the key buys a **grant**: single-use, sixty seconds, and worthless the
//! moment it is spent. What ends up in the URL bar is a token that has already
//! stopped working by the time anyone reads it.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

/// How long a grant is worth anything. Long enough for a browser to start cold,
/// short enough that a stale one in a shell's scrollback is inert.
const GRANT_TTL_SECS: u64 = 60;

/// Read `/dev/urandom` directly, as the rest of this module's neighbours do,
/// rather than adding a dependency for thirty-two bytes.
fn random_hex(bytes: usize) -> std::io::Result<String> {
    let mut buf = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Load the console key, creating it if this host has never had one.
///
/// Mode 0600, set **before** any content exists — the same order `enrol.rs`
/// uses, because a secret that is briefly world-readable was world-readable.
pub fn load_or_create_key(path: &Path) -> std::io::Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let key = random_hex(32)?;
    write_private(path, key.as_bytes())?;
    Ok(key)
}

fn write_private(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(content)?;
    f.write_all(b"\n")
}

/// Single-use, short-lived grants, held in memory only.
///
/// In memory on purpose: a grant that survived a restart would be a credential
/// on disk with none of the care the real ones get, and there is no reason for
/// one to outlive the process that minted it.
#[derive(Default)]
pub struct Grants {
    issued: Mutex<HashMap<String, u64>>,
}

impl Grants {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a grant. The caller has already proved it holds the console key.
    pub fn mint(&self, now: u64) -> std::io::Result<String> {
        let token = random_hex(24)?;
        let mut issued = self.issued.lock().expect("grants poisoned");
        // Sweep on mint rather than on a timer: the map is tiny, this is the
        // only thing that grows it, and a background task to expire two entries
        // would be more machinery than the thing it maintains.
        issued.retain(|_, expires| *expires > now);
        issued.insert(token.clone(), now + GRANT_TTL_SECS);
        Ok(token)
    }

    /// Spend a grant. True at most once per grant, ever.
    pub fn spend(&self, token: &str, now: u64) -> bool {
        let mut issued = self.issued.lock().expect("grants poisoned");
        issued.retain(|_, expires| *expires > now);
        issued.remove(token).is_some()
    }
}

/// Compare in constant time, so a wrong key cannot be narrowed by timing it.
pub fn key_matches(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.trim().as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_is_worth_exactly_one_use() {
        let g = Grants::new();
        let t = g.mint(1000).expect("urandom readable");
        assert!(g.spend(&t, 1000));
        assert!(!g.spend(&t, 1000), "a spent grant must never work twice");
    }

    #[test]
    fn a_grant_expires() {
        let g = Grants::new();
        let t = g.mint(1000).expect("urandom readable");
        assert!(!g.spend(&t, 1000 + GRANT_TTL_SECS + 1));
    }

    #[test]
    fn an_unknown_grant_is_refused() {
        let g = Grants::new();
        assert!(!g.spend("not-a-grant", 1000));
    }

    #[test]
    fn key_comparison_rejects_near_misses_and_lengths() {
        assert!(key_matches("abc123", "abc123"));
        assert!(key_matches("abc123", " abc123\n"));
        assert!(!key_matches("abc123", "abc124"));
        assert!(!key_matches("abc123", "abc12"));
        assert!(!key_matches("abc123", ""));
    }

    #[test]
    fn a_created_key_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("prism-console-{}", std::process::id()));
        let path = dir.join("console.key");
        let key = load_or_create_key(&path).expect("writable temp dir");
        assert_eq!(key.len(), 64, "32 bytes as hex");
        let mode = std::fs::metadata(&path).expect("exists").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "a secret must never be group- or world-readable");
        // and it is stable across calls, or every restart would invalidate it
        assert_eq!(key, load_or_create_key(&path).expect("readable"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
