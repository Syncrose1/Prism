//! Signed session tokens.
//!
//! A token is self-contained and stateless: its claims travel inside it and are
//! authenticated by an HMAC over the whole payload. There is no server-side
//! session table to keep consistent, which suits a daemon that must keep working
//! while the machine it manages is falling over.
//!
//! The signing key lives in the state directory at mode 0600 and is generated on
//! first run. Losing it invalidates every issued token, which is the correct
//! failure mode.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Bumped if the payload layout ever changes, so old tokens fail closed rather
/// than being misparsed into different claims.
const TOKEN_VERSION: &str = "v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claims {
    /// When the session itself expires.
    pub expires_at: u64,
    /// When a TOTP code was last successfully presented. Drives the `Fresh`
    /// tier: holding a valid session is not the same as having proved
    /// possession of the phone recently.
    pub totp_verified_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    Malformed,
    BadSignature,
    Expired,
    WrongVersion,
}

pub struct SessionKey {
    key: [u8; 32],
}

impl SessionKey {
    pub fn from_bytes(key: [u8; 32]) -> Self {
        Self { key }
    }

    pub fn generate() -> std::io::Result<Self> {
        use std::io::Read;
        let mut key = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut key)?;
        Ok(Self { key })
    }

    /// Load the key from disk, creating it on first run.
    ///
    /// Written with mode 0600 before any content, so the secret is never briefly
    /// world-readable between creation and chmod.
    pub fn load_or_create(path: &std::path::Path) -> std::io::Result<Self> {
        use std::io::Read;
        if let Ok(mut file) = std::fs::File::open(path) {
            let mut key = [0u8; 32];
            file.read_exact(&mut key)?;
            return Ok(Self::from_bytes(key));
        }

        let key = Self::generate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_private(path, &key.key)?;
        Ok(key)
    }

    fn sign(&self, payload: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("HMAC accepts any key length");
        mac.update(payload.as_bytes());
        hex(&mac.finalize().into_bytes())
    }

    pub fn issue(&self, claims: Claims) -> String {
        let payload = format!(
            "{TOKEN_VERSION}.{}.{}",
            claims.expires_at, claims.totp_verified_at
        );
        let signature = self.sign(&payload);
        format!("{payload}.{signature}")
    }

    /// Verify a token and return its claims.
    ///
    /// The signature is checked *before* the expiry, and compared in constant
    /// time, so neither validity nor content is inferable from timing or from
    /// which error comes back for a forged token.
    pub fn validate(&self, token: &str, now: u64) -> Result<Claims, TokenError> {
        let parts: Vec<&str> = token.trim().split('.').collect();
        let [version, expires, verified, signature] = parts.as_slice() else {
            return Err(TokenError::Malformed);
        };
        if *version != TOKEN_VERSION {
            return Err(TokenError::WrongVersion);
        }

        let payload = format!("{version}.{expires}.{verified}");
        let expected = self.sign(&payload);
        let signature_ok: bool = expected.as_bytes().ct_eq(signature.as_bytes()).into();
        if !signature_ok {
            return Err(TokenError::BadSignature);
        }

        let (Ok(expires_at), Ok(totp_verified_at)) =
            (expires.parse::<u64>(), verified.parse::<u64>())
        else {
            return Err(TokenError::Malformed);
        };

        if now >= expires_at {
            return Err(TokenError::Expired);
        }
        Ok(Claims {
            expires_at,
            totp_verified_at,
        })
    }
}

/// Create a file readable only by its owner, before writing anything into it.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SessionKey {
        SessionKey::from_bytes([7u8; 32])
    }

    fn claims(now: u64) -> Claims {
        Claims {
            expires_at: now + 3600,
            totp_verified_at: now,
        }
    }

    #[test]
    fn round_trips() {
        let now = 1_700_000_000;
        let k = key();
        let token = k.issue(claims(now));
        assert_eq!(k.validate(&token, now), Ok(claims(now)));
    }

    #[test]
    fn rejects_a_tampered_payload() {
        let now = 1_700_000_000;
        let k = key();
        let token = k.issue(claims(now));
        // Extend the expiry by hand — the classic forgery attempt.
        let parts: Vec<&str> = token.split('.').collect();
        let forged = format!("{}.{}.{}.{}", parts[0], now + 999_999, parts[2], parts[3]);
        assert_eq!(k.validate(&forged, now), Err(TokenError::BadSignature));
    }

    #[test]
    fn rejects_a_token_signed_with_another_key() {
        let now = 1_700_000_000;
        let token = key().issue(claims(now));
        let other = SessionKey::from_bytes([9u8; 32]);
        assert_eq!(other.validate(&token, now), Err(TokenError::BadSignature));
    }

    #[test]
    fn rejects_expired_tokens() {
        let now = 1_700_000_000;
        let k = key();
        let token = k.issue(Claims {
            expires_at: now + 10,
            totp_verified_at: now,
        });
        assert_eq!(k.validate(&token, now + 11), Err(TokenError::Expired));
    }

    #[test]
    fn expiry_boundary_is_exclusive() {
        let now = 1_700_000_000;
        let k = key();
        let token = k.issue(Claims {
            expires_at: now + 10,
            totp_verified_at: now,
        });
        assert!(k.validate(&token, now + 9).is_ok());
        assert_eq!(k.validate(&token, now + 10), Err(TokenError::Expired));
    }

    #[test]
    fn rejects_malformed_tokens_without_panicking() {
        let k = key();
        for bad in [
            "",
            "garbage",
            "v1.1.2",
            "v1.1.2.3.4",
            "v1.notanumber.2.deadbeef",
            "....",
        ] {
            assert!(
                k.validate(bad, 1_700_000_000).is_err(),
                "token {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_a_different_version() {
        let now = 1_700_000_000;
        let k = key();
        let token = k.issue(claims(now)).replacen("v1", "v2", 1);
        assert_eq!(k.validate(&token, now), Err(TokenError::WrongVersion));
    }

    #[test]
    fn signature_is_checked_before_expiry() {
        // A forged *and* expired token must report BadSignature, never Expired:
        // reporting Expired would confirm the signature was accepted, telling an
        // attacker their forgery worked.
        let now = 1_700_000_000;
        let k = key();
        let forged = format!("v1.{}.{}.{}", now - 100, now - 100, "00".repeat(32));
        assert_eq!(k.validate(&forged, now), Err(TokenError::BadSignature));
    }

    #[test]
    fn key_file_is_created_private_and_is_stable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("prism-key-test-{}", std::process::id()));
        let path = dir.join("session.key");
        let _ = std::fs::remove_dir_all(&dir);

        let first = SessionKey::load_or_create(&path).expect("create");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "session key must not be readable by others");

        // Reloading must yield the same key, or every restart logs everyone out.
        let second = SessionKey::load_or_create(&path).expect("reload");
        let now = 1_700_000_000;
        let token = first.issue(claims(now));
        assert!(second.validate(&token, now).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
