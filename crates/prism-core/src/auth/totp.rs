//! RFC 6238 TOTP, compatible with Google Authenticator and Microsoft
//! Authenticator.
//!
//! Implemented directly rather than via a crate: the algorithm is short, the RFC
//! publishes test vectors, and this code guards remote filesystem access — so
//! being able to verify it against the standard's own numbers is worth more than
//! the dependency saved. Every vector in RFC 6238 Appendix B is asserted below.
//!
//! Defaults match what the authenticator apps assume: HMAC-SHA1, 30-second
//! steps, 6 digits. Those are not configurable, because a mismatch produces a
//! silent "wrong code" that is miserable to debug from a phone in another city.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;

type HmacSha1 = Hmac<Sha1>;

pub const STEP_SECS: u64 = 30;
pub const DIGITS: u32 = 6;
/// Secret length in bytes. 20 matches the RFC's SHA-1 block and is what the
/// authenticator apps expect.
pub const SECRET_LEN: usize = 20;

/// Generate a new secret from the kernel CSPRNG.
///
/// Reads `/dev/urandom` directly rather than adding a dependency. Failure to
/// read it is fatal by design: silently falling back to a weaker source would
/// produce a secret that looks fine and protects nothing.
pub fn generate_secret() -> std::io::Result<[u8; SECRET_LEN]> {
    use std::io::Read;
    let mut secret = [0u8; SECRET_LEN];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut secret)?;
    Ok(secret)
}

/// The counter (number of time steps) for a given Unix time.
pub fn counter_at(unix_time: u64) -> u64 {
    unix_time / STEP_SECS
}

/// HOTP as defined by RFC 4226 — the primitive TOTP is built from.
pub fn hotp(secret: &[u8], counter: u64, digits: u32) -> u32 {
    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();

    // Dynamic truncation (RFC 4226 §5.3): the low nibble of the last byte
    // selects a 4-byte window, whose top bit is masked off.
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    binary % 10u32.pow(digits)
}

/// The TOTP code valid at a given Unix time.
pub fn totp_at(secret: &[u8], unix_time: u64) -> u32 {
    hotp(secret, counter_at(unix_time), DIGITS)
}

/// Format a code with leading zeros, as the authenticator app displays it.
pub fn format_code(code: u32) -> String {
    format!("{code:0width$}", width = DIGITS as usize)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// Valid. The counter is returned so the caller can record it as consumed.
    Valid { counter: u64 },
    Invalid,
}

/// Verify a submitted code, tolerating `skew` steps either side of now.
///
/// Comparison is constant-time. A naive `==` on the digits leaks, through timing,
/// how much of a guess was correct — which over many attempts narrows a 6-digit
/// space considerably faster than brute force.
///
/// `skew` of 1 (±30 s) accommodates ordinary clock drift between the host and a
/// phone. Larger windows meaningfully widen the guessing surface: each extra
/// step is another valid code at any instant.
pub fn verify(secret: &[u8], submitted: &str, unix_time: u64, skew: u64) -> VerifyOutcome {
    let submitted = submitted.trim();
    // Reject anything that is not exactly the expected shape before doing work.
    if submitted.len() != DIGITS as usize || !submitted.bytes().all(|b| b.is_ascii_digit()) {
        return VerifyOutcome::Invalid;
    }

    let centre = counter_at(unix_time);
    let mut outcome = VerifyOutcome::Invalid;
    for counter in centre.saturating_sub(skew)..=centre.saturating_add(skew) {
        let expected = format_code(hotp(secret, counter, DIGITS));
        // Do not break early on match: returning at the first hit would make the
        // duration of a call depend on which step matched.
        if expected.as_bytes().ct_eq(submitted.as_bytes()).into() {
            outcome = VerifyOutcome::Valid { counter };
        }
    }
    outcome
}

/// RFC 4648 base32, uppercase, unpadded — the encoding authenticator apps expect
/// for manual key entry.
pub fn base32_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer: u16 = 0;
    let mut bits: u8 = 0;

    for &byte in data {
        buffer = (buffer << 8) | byte as u16;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

/// The `otpauth://` URI an authenticator app consumes, by QR or manual entry.
pub fn provisioning_uri(secret: &[u8], issuer: &str, account: &str) -> String {
    format!(
        "otpauth://totp/{issuer}:{account}?secret={}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        base32_encode(secret)
    )
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B uses this ASCII secret for the SHA-1 vectors.
    const RFC_SECRET: &[u8] = b"12345678901234567890";

    /// RFC 6238 Appendix B, truncated from the published 8-digit values to the
    /// 6 digits this implementation uses. If these fail, the implementation is
    /// wrong — not the vectors.
    #[test]
    fn matches_rfc6238_test_vectors() {
        let cases: &[(u64, u32)] = &[
            (59, 94287082 % 1_000_000),
            (1111111109, 7081804 % 1_000_000),
            (1111111111, 14050471 % 1_000_000),
            (1234567890, 89005924 % 1_000_000),
            (2000000000, 69279037 % 1_000_000),
            (20000000000, 65353130 % 1_000_000),
        ];
        for &(time, expected) in cases {
            assert_eq!(
                totp_at(RFC_SECRET, time),
                expected,
                "RFC 6238 vector failed at T={time}"
            );
        }
    }

    #[test]
    fn counter_advances_every_thirty_seconds() {
        assert_eq!(counter_at(0), 0);
        assert_eq!(counter_at(29), 0);
        assert_eq!(counter_at(30), 1);
        assert_eq!(counter_at(59), 1);
        assert_eq!(counter_at(60), 2);
    }

    #[test]
    fn codes_are_six_digits_zero_padded() {
        // Find a counter producing a value below 100000 to exercise padding.
        let mut padded = None;
        for t in 0..5000u64 {
            let code = totp_at(RFC_SECRET, t * 30);
            if code < 100_000 {
                padded = Some(format_code(code));
                break;
            }
        }
        let padded = padded.expect("some code is below 100000");
        assert_eq!(padded.len(), 6, "codes must always render as 6 digits");
    }

    #[test]
    fn accepts_the_current_code() {
        let now = 1_700_000_000;
        let code = format_code(totp_at(RFC_SECRET, now));
        assert!(matches!(
            verify(RFC_SECRET, &code, now, 1),
            VerifyOutcome::Valid { .. }
        ));
    }

    #[test]
    fn accepts_within_skew_and_rejects_beyond_it() {
        let now = 1_700_000_000;
        let previous = format_code(totp_at(RFC_SECRET, now - 30));
        let ancient = format_code(totp_at(RFC_SECRET, now - 300));

        assert!(matches!(
            verify(RFC_SECRET, &previous, now, 1),
            VerifyOutcome::Valid { .. }
        ));
        assert_eq!(
            verify(RFC_SECRET, &ancient, now, 1),
            VerifyOutcome::Invalid,
            "a code from five minutes ago must not be accepted"
        );
    }

    #[test]
    fn zero_skew_rejects_the_previous_step() {
        let now = 1_700_000_000;
        let previous = format_code(totp_at(RFC_SECRET, now - 30));
        assert_eq!(verify(RFC_SECRET, &previous, now, 0), VerifyOutcome::Invalid);
    }

    #[test]
    fn returns_the_counter_so_replay_can_be_prevented() {
        let now = 1_700_000_000;
        let code = format_code(totp_at(RFC_SECRET, now));
        match verify(RFC_SECRET, &code, now, 1) {
            VerifyOutcome::Valid { counter } => assert_eq!(counter, counter_at(now)),
            VerifyOutcome::Invalid => panic!("should have verified"),
        }
    }

    #[test]
    fn rejects_malformed_input_without_panicking() {
        let now = 1_700_000_000;
        for bad in ["", "12345", "1234567", "abcdef", "12 456", "  ", "-12345"] {
            assert_eq!(
                verify(RFC_SECRET, bad, now, 1),
                VerifyOutcome::Invalid,
                "input {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn wrong_code_is_rejected() {
        let now = 1_700_000_000;
        let correct = totp_at(RFC_SECRET, now);
        let wrong = format_code((correct + 1) % 1_000_000);
        assert_eq!(verify(RFC_SECRET, &wrong, now, 1), VerifyOutcome::Invalid);
    }

    #[test]
    fn different_secrets_produce_different_codes() {
        let now = 1_700_000_000;
        let a = totp_at(b"12345678901234567890", now);
        let b = totp_at(b"09876543210987654321", now);
        assert_ne!(a, b);
    }

    /// RFC 4648 §10 test vectors.
    #[test]
    fn base32_matches_rfc4648() {
        assert_eq!(base32_encode(b""), "");
        assert_eq!(base32_encode(b"f"), "MY");
        assert_eq!(base32_encode(b"fo"), "MZXQ");
        assert_eq!(base32_encode(b"foo"), "MZXW6");
        assert_eq!(base32_encode(b"foob"), "MZXW6YQ");
        assert_eq!(base32_encode(b"fooba"), "MZXW6YTB");
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn provisioning_uri_is_well_formed() {
        let uri = provisioning_uri(RFC_SECRET, "Prism", "raahats@c2");
        assert!(uri.starts_with("otpauth://totp/Prism:raahats@c2?"));
        assert!(uri.contains("secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"));
        assert!(uri.contains("algorithm=SHA1"));
        assert!(uri.contains("digits=6"));
        assert!(uri.contains("period=30"));
    }

    #[test]
    fn generated_secrets_are_the_right_length_and_not_constant() {
        let a = generate_secret().expect("urandom readable");
        let b = generate_secret().expect("urandom readable");
        assert_eq!(a.len(), SECRET_LEN);
        assert_ne!(a, b, "two generated secrets must not be identical");
        assert_ne!(a, [0u8; SECRET_LEN], "secret must not be all zeroes");
    }
}
