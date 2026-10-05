//! **Links, not doors** (POLARIS docs/fabric.md §8): a way into exactly one
//! folder, for a while, doing only what was chosen. The person on the other
//! end sees that folder and nothing above or beside it; nothing about the PC
//! exists for them.
//!
//! * **Time-limited**, always; optionally **one-time**: the first device to
//!   open it keeps it, and any other is refused.
//! * **Exactly scoped**: the folder becomes the link's own root, so the same
//!   containment check that guards Files ([`crate::files::path::resolve`])
//!   guards every request through the link.
//! * **Profiles**: what the link allows ([`Perms`]). Presets live in the
//!   UI; a custom set is remembered in [`Store::history`] so the next share
//!   is one tap.
//! * "Only what they sent" ([`Perms::own`]): the link remembers what came in
//!   through it, and shows only that.
//!
//! The token is the credential: 128 random bits, never shown in full again
//! after it's made, compared in constant time.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Perms {
    /// See what is in the folder.
    pub see: bool,
    /// See only what was uploaded through this link.
    pub own: bool,
    /// Open pictures, music and films in the page.
    pub stream: bool,
    /// Download.
    pub down: bool,
    /// Upload.
    pub up: bool,
    /// Rename and remove.
    pub edit: bool,
}

impl Perms {
    /// Can anything at all be listed?
    pub fn lists(&self) -> bool { self.see || self.own }
    /// Does the link write to the folder?
    pub fn writes(&self) -> bool { self.up || self.edit }
    /// In a sentence, for the page and the log.
    pub fn say(&self) -> String {
        let mut v = Vec::new();
        if self.see { v.push("see the folder") } else if self.own { v.push("see what they send") }
        if self.stream { v.push("play") }
        if self.down { v.push("download") }
        if self.up { v.push("upload") }
        if self.edit { v.push("rename and remove") }
        if v.is_empty() { "nothing yet".into() } else { v.join(", ") }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// A short id for the owner's list and for revoking; not a credential.
    pub id: String,
    /// The credential: hex, 32 characters.
    pub token: String,
    pub label: String,
    /// Which file root, and the folder inside it.
    pub root: String,
    pub path: String,
    pub perms: Perms,
    pub created: u64,
    pub expires: u64,
    pub once: bool,
    /// The device a one-time link went to.
    #[serde(default)]
    pub bound: Option<String>,
    #[serde(default)]
    pub revoked: bool,
    /// What came in through it (relative to its folder), for `own`.
    #[serde(default)]
    pub mine: Vec<String>,
    #[serde(default)]
    pub visits: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Unknown,
    Expired,
    Revoked,
    /// One-time, and already opened on another device.
    Taken,
}

impl Refusal {
    pub fn say(&self) -> &'static str {
        match self {
            Refusal::Unknown => "This link doesn't open anything. Check that it was copied whole.",
            Refusal::Expired => "This link has expired. Ask whoever shared it for a new one.",
            Refusal::Revoked => "This link was taken back by whoever shared it.",
            Refusal::Taken => "This link has already been opened on another device, and it works for one device only.",
        }
    }
}

impl Link {
    /// May [device] use this link at [now]? A one-time link not yet opened
    /// is bound to the first device that asks, by [Store::admit].
    pub fn check(&self, device: Option<&str>, now: u64) -> Result<(), Refusal> {
        if self.revoked { return Err(Refusal::Revoked) }
        if now >= self.expires { return Err(Refusal::Expired) }
        if self.once && let Some(b) = &self.bound && Some(b.as_str()) != device { return Err(Refusal::Taken) }
        Ok(())
    }

    /// May this relative path (inside the link's folder) be seen?
    pub fn shows(&self, relative: &str) -> bool {
        if self.perms.see { return true }
        if !self.perms.own { return false }
        let r = relative.trim_matches('/');
        r.is_empty() || self.mine.iter().any(|m| m == r || m.starts_with(&format!("{r}/")))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub links: Vec<Link>,
    /// Custom profiles used before, newest first, so the next share is one tap.
    #[serde(default)]
    pub history: Vec<Perms>,
}

/// Every link, and the profiles used before, in one file under the state
/// directory, written atomically.
pub fn path(state_dir: &Path) -> PathBuf { state_dir.join("links.json") }

impl Store {
    pub fn load(state_dir: &Path) -> Store {
        std::fs::read_to_string(path(state_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    pub fn save(&self, state_dir: &Path) -> std::io::Result<()> {
        let p = path(state_dir);
        let tmp = p.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)); }
        std::fs::rename(tmp, p)
    }

    /// Make a link. [custom] adds its profile to the history.
    pub fn make(&mut self, token: String, label: String, root: String, path: String, perms: Perms, lasts_secs: u64, once: bool, custom: bool, now: u64) -> Link {
        let link = Link { id: token[..8].to_string(), token, label, root, path, perms, created: now, expires: now + lasts_secs.clamp(60, 90 * 86_400), once, bound: None, revoked: false, mine: Vec::new(), visits: 0 };
        self.links.push(link.clone());
        if custom {
            self.history.retain(|p| *p != perms);
            self.history.insert(0, perms);
            self.history.truncate(8);
        }
        // Long-gone links don't need keeping: a week past their end.
        self.links.retain(|l| l.expires + 7 * 86_400 > now);
        link
    }

    /// The link a token names, compared in constant time.
    pub fn find(&self, token: &str) -> Option<usize> {
        use subtle::ConstantTimeEq;
        self.links.iter().position(|l| l.token.len() == token.len() && bool::from(l.token.as_bytes().ct_eq(token.as_bytes())))
    }

    /// Let [device] in, binding a one-time link to it the first time.
    pub fn admit(&mut self, token: &str, device: &str, now: u64) -> Result<Link, Refusal> {
        let i = self.find(token).ok_or(Refusal::Unknown)?;
        let l = &mut self.links[i];
        l.check(Some(device), now)?;
        if l.once && l.bound.is_none() { l.bound = Some(device.to_string()) }
        l.visits += 1;
        Ok(l.clone())
    }

    pub fn revoke(&mut self, id: &str) -> bool {
        match self.links.iter_mut().find(|l| l.id == id) { Some(l) => { l.revoked = true; true } None => false }
    }

    /// Remember a file that came in through a link (for `own`).
    pub fn note_mine(&mut self, token: &str, relative: &str) {
        if let Some(i) = self.find(token) {
            let r = relative.trim_matches('/').to_string();
            if !self.links[i].mine.contains(&r) { self.links[i].mine.push(r) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drop_off() -> Perms { Perms { up: true, own: true, ..Default::default() } }
    const T: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn a_link_ends_when_it_says_and_when_it_is_taken_back() {
        let mut s = Store::default();
        let l = s.make(T.into(), "FLACs".into(), "scratch".into(), "From friends".into(), drop_off(), 3600, false, false, 1000);
        assert_eq!(l.id, "01234567");
        assert!(s.admit(T, "phone", 1000).is_ok());
        assert_eq!(s.admit(T, "phone", 1000 + 3600), Err(Refusal::Expired));
        assert!(s.revoke("01234567"));
        assert_eq!(s.admit(T, "phone", 1001), Err(Refusal::Revoked));
        assert_eq!(s.admit("nope", "phone", 1001), Err(Refusal::Unknown));
    }

    #[test]
    fn a_one_time_link_keeps_the_first_device_and_refuses_the_rest() {
        let mut s = Store::default();
        s.make(T.into(), "x".into(), "r".into(), "".into(), drop_off(), 3600, true, false, 0);
        assert!(s.admit(T, "laptop", 10).is_ok());
        assert!(s.admit(T, "laptop", 20).is_ok(), "the same device again");
        assert_eq!(s.admit(T, "phone", 30), Err(Refusal::Taken));
    }

    #[test]
    fn only_what_they_sent_is_shown_with_own() {
        let mut s = Store::default();
        s.make(T.into(), "x".into(), "r".into(), "".into(), drop_off(), 3600, false, false, 0);
        s.note_mine(T, "Kind of Blue/01 So What.flac");
        let l = &s.links[0];
        assert!(l.shows(""));
        assert!(l.shows("Kind of Blue"), "the folder holding what they sent");
        assert!(l.shows("Kind of Blue/01 So What.flac"));
        assert!(!l.shows("My private.flac"));
        let mut see = l.clone(); see.perms.see = true;
        assert!(see.shows("My private.flac"));
    }

    #[test]
    fn custom_profiles_are_remembered_newest_first_without_repeats() {
        let mut s = Store::default();
        let a = Perms { see: true, stream: true, ..Default::default() };
        s.make(T.into(), "a".into(), "r".into(), "".into(), a, 60, false, true, 0);
        s.make("fedcba9876543210fedcba9876543210".into(), "b".into(), "r".into(), "".into(), drop_off(), 60, false, true, 0);
        s.make("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(), "c".into(), "r".into(), "".into(), a, 60, false, true, 0);
        assert_eq!(s.history, vec![a, drop_off()]);
    }

    #[test]
    fn a_link_lasts_between_a_minute_and_ninety_days() {
        let mut s = Store::default();
        assert_eq!(s.make(T.into(), "x".into(), "r".into(), "".into(), drop_off(), 1, false, false, 0).expires, 60);
        assert_eq!(s.make("b".repeat(32), "x".into(), "r".into(), "".into(), drop_off(), u64::MAX / 2, false, false, 0).expires, 90 * 86_400);
    }
}
