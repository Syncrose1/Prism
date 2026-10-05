//! **The access log**: who signed in to Prism, when, from where, and what
//! they changed, in a form that can be added to and never quietly edited.
//!
//! Every entry carries the SHA-256 of the entry before it, and its own hash
//! covers that link. Changing, removing or reordering any entry breaks every
//! hash after it, which [`verify`] finds and names. That is tamper-*evident*,
//! not tamper-proof: someone with this user's shell could rewrite the whole
//! chain from the start. The remedy for that is to anchor the newest hash
//! somewhere they can't reach (another device over the overlay, or
//! `chattr +a`, which needs root), and the chain is what makes either worth
//! doing.
//!
//! Entries are rare (sign-ins, refusals, changes), so each is synced to disk
//! before the request that caused it is answered.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The hash a chain starts from.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A sign-in that worked.
    SignedIn,
    /// A sign-in refused (wrong code or password, locked out).
    Refused,
    /// A change made: a facet, a limit, a config file.
    Changed,
    /// Signed out.
    SignedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub unix: u64,
    pub kind: Kind,
    /// Who: an account name, `admin` (Prism's own sign-in), `polaris` (the
    /// local bridge, with the account it acted for in [`Entry::for_account`]),
    /// or `unknown` for a refusal.
    pub who: String,
    /// Acting for: the POLARIS account a bridge request was made for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_account: Option<String>,
    /// How: `code`, `password`, `console`, `bridge`.
    pub how: String,
    /// From: the peer address.
    pub from: String,
    /// What, in a sentence (`POST /api/facets/comfyui/limits`).
    pub what: String,
    pub prev: String,
    pub hash: String,
}

impl Entry {
    fn digest(&self) -> String {
        // Everything but the hash itself, in a fixed order.
        let body = serde_json::json!([self.unix, self.kind, self.who, self.for_account, self.how, self.from, self.what, self.prev]);
        let mut h = Sha256::new();
        h.update(body.to_string().as_bytes());
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// What a caller says about one access; the log adds time and the chain.
#[derive(Debug, Clone, Default)]
pub struct Access {
    pub who: String,
    pub for_account: Option<String>,
    pub how: String,
    pub from: String,
    pub what: String,
}

pub struct AccessLog {
    path: PathBuf,
    /// The last hash, so appending never rereads the file.
    last: Mutex<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verified {
    pub entries: usize,
    /// The 1-based line where the chain first breaks, if it does.
    pub broken_at: Option<usize>,
    pub last_hash: String,
}

impl AccessLog {
    /// The log at [path], continuing its chain.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        let last = read(path).last().map(|e| e.hash.clone()).unwrap_or_else(|| GENESIS.into());
        Ok(Self { path: path.to_path_buf(), last: Mutex::new(last) })
    }

    pub fn append(&self, kind: Kind, a: Access, unix: u64) -> std::io::Result<Entry> {
        let mut last = self.last.lock().map_err(|_| std::io::Error::other("access log poisoned"))?;
        let mut e = Entry { unix, kind, who: a.who, for_account: a.for_account, how: a.how, from: a.from, what: a.what, prev: last.clone(), hash: String::new() };
        e.hash = e.digest();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        let mut line = serde_json::to_string(&e).map_err(std::io::Error::other)?;
        line.push('\n');
        f.write_all(line.as_bytes())?;
        f.sync_data()?;
        *last = e.hash.clone();
        Ok(e)
    }

    /// Newest first, optionally only one person's.
    pub fn recent(&self, limit: usize, who: Option<&str>) -> Vec<Entry> {
        read(&self.path).into_iter().rev()
            .filter(|e| who.is_none_or(|w| e.who == w || e.for_account.as_deref() == Some(w)))
            .take(limit).collect()
    }

    pub fn verify(&self) -> Verified { verify(&self.path) }
}

fn read(path: &Path) -> Vec<Entry> {
    std::fs::read_to_string(path).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
}

/// Walk the chain. A line that doesn't parse breaks it as surely as a wrong
/// hash: an entry that can't be read is an entry that was changed.
pub fn verify(path: &Path) -> Verified {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut prev = GENESIS.to_string();
    let mut n = 0;
    for (i, line) in text.lines().enumerate() {
        let ok = serde_json::from_str::<Entry>(line).ok().filter(|e| e.prev == prev && e.digest() == e.hash);
        match ok {
            Some(e) => { prev = e.hash; n += 1; }
            None => return Verified { entries: n, broken_at: Some(i + 1), last_hash: prev },
        }
    }
    Verified { entries: n, broken_at: None, last_hash: prev }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("prism-access-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("access.jsonl")
    }

    fn a(who: &str, what: &str) -> Access { Access { who: who.into(), how: "code".into(), from: "100.82.14.6".into(), what: what.into(), ..Default::default() } }

    #[test]
    fn a_chain_is_kept_across_opens_and_verifies() {
        let p = tmp("chain");
        let log = AccessLog::open(&p).unwrap();
        log.append(Kind::SignedIn, a("admin", "signed in"), 1).unwrap();
        log.append(Kind::Refused, a("unknown", "wrong code"), 2).unwrap();
        drop(log);
        let log = AccessLog::open(&p).unwrap();
        let mut c = a("polaris", "PUT /api/config/profile");
        c.for_account = Some("raahat".into());
        c.how = "bridge".into();
        log.append(Kind::Changed, c, 3).unwrap();
        let v = log.verify();
        assert_eq!((v.entries, v.broken_at), (3, None));
        // Filtered by person, including what was done on their behalf.
        assert_eq!(log.recent(10, Some("raahat")).len(), 1);
        assert_eq!(log.recent(10, None)[0].what, "PUT /api/config/profile");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn editing_or_removing_an_entry_is_found() {
        let p = tmp("tamper");
        let log = AccessLog::open(&p).unwrap();
        for i in 0..4 { log.append(Kind::SignedIn, a("admin", &format!("sign-in {i}")), i).unwrap(); }
        let text = std::fs::read_to_string(&p).unwrap();
        // Change who signed in on the second line.
        std::fs::write(&p, text.replacen("sign-in 1", "sign-in 9", 1)).unwrap();
        assert_eq!(verify(&p).broken_at, Some(2));
        // Remove the second line instead.
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&p, format!("{}\n{}\n{}\n", lines[0], lines[2], lines[3])).unwrap();
        assert_eq!(verify(&p).broken_at, Some(2));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
