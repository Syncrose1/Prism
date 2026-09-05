//! The quick unlock.
//!
//! *Operator, 2026-09-05: "the auth is genuinely super annoying, it runs on a
//! timer, rather than on a session. Auth code to validate the device, password
//! to access the app more quickly."*
//!
//! They are right, and the original design was the mistake. Gating file access
//! on "a TOTP code within the last 15 minutes" meant reaching for a phone every
//! quarter of an hour of ordinary use — theoretically defensible, practically
//! hostile, and hostile in a way that pushes an operator toward turning auth off
//! entirely.
//!
//! The replacement is the pattern a phone uses: a strong factor to enrol the
//! device once, a fast one to unlock it thereafter. The authenticator proves
//! *this browser* is the operator's, durably. The password unlocks a session,
//! quickly, with no timer.
//!
//! Argon2id, because a password hash that leaks should be expensive to attack
//! on a GPU. Parameters are the crate defaults, which track current guidance.

use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::phc::PasswordHash};

/// Minimum length. Deliberately modest: this is a second factor behind a
/// tailnet boundary and a device-trust cookie, not a lone perimeter, and an
/// unreasonable rule here is one an operator works around.
pub const MIN_LENGTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordError {
    TooShort,
    Hashing,
}

impl std::fmt::Display for PasswordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasswordError::TooShort => {
                write!(f, "password must be at least {MIN_LENGTH} characters")
            }
            PasswordError::Hashing => write!(f, "could not hash password"),
        }
    }
}

/// Produce a PHC-format hash, salt included.
pub fn hash(password: &str) -> Result<String, PasswordError> {
    if password.chars().count() < MIN_LENGTH {
        return Err(PasswordError::TooShort);
    }
    // hash_password generates its own salt from the OS RNG.
    let hash: PasswordHash = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|_| PasswordError::Hashing)?;
    Ok(hash.to_string())
}

/// Check a password against a stored hash.
///
/// Argon2's own verification is constant-time with respect to the hash, so a
/// wrong password costs the same as a right one.
pub fn verify(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        // A corrupt hash must fail closed rather than accepting anything.
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_correct_password_verifies() {
        let h = hash("correct horse battery").unwrap();
        assert!(verify("correct horse battery", &h));
    }

    #[test]
    fn a_wrong_password_does_not() {
        let h = hash("correct horse battery").unwrap();
        assert!(!verify("incorrect horse battery", &h));
        assert!(!verify("", &h));
    }

    #[test]
    fn the_same_password_hashes_differently_each_time() {
        // Salted: two accounts with the same password must not share a hash.
        let a = hash("same password here").unwrap();
        let b = hash("same password here").unwrap();
        assert_ne!(a, b);
        assert!(verify("same password here", &a));
        assert!(verify("same password here", &b));
    }

    #[test]
    fn short_passwords_are_refused() {
        assert_eq!(hash("short").unwrap_err(), PasswordError::TooShort);
        assert!(hash("exactly8").is_ok());
    }

    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        // Eight emoji is eight characters; rejecting it as "too short" would be
        // wrong, and counting bytes would accept a much shorter one elsewhere.
        assert!(hash("🔐🔐🔐🔐🔐🔐🔐🔐").is_ok());
    }

    #[test]
    fn a_corrupt_hash_fails_closed() {
        assert!(!verify("anything", "not a phc string"));
        assert!(!verify("anything", ""));
        assert!(!verify("anything", "$argon2id$v=19$garbage"));
    }

    #[test]
    fn the_hash_is_phc_formatted_and_names_argon2id() {
        let h = hash("a good long password").unwrap();
        assert!(h.starts_with("$argon2id$"), "got {h}");
    }

    #[test]
    fn whitespace_is_significant() {
        let h = hash("password with spaces").unwrap();
        assert!(!verify("passwordwithspaces", &h));
        assert!(!verify(" password with spaces", &h));
    }
}
