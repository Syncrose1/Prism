//! Authentication.
//!
//! Two factors, but not on a timer.
//!
//! *Operator, 2026-09-05: "the auth is genuinely super annoying, it runs on a
//! timer, rather than on a session. Auth code to validate the device, password
//! to access the app more quickly."*
//!
//! The original design gated sensitive actions on having shown a TOTP code
//! within the last fifteen minutes. That is defensible on paper and hostile in
//! practice: it meant reaching for a phone repeatedly during ordinary use, and
//! an auth scheme that irritating is one an operator eventually disables.
//!
//! What replaced it is how a phone works:
//!
//! | Step | Factor | Frequency |
//! |---|---|---|
//! | Enrol this browser | authenticator code | once per device |
//! | Unlock | password | when the session lapses |
//! | Use | the session | no interruption |
//!
//! The **device token** proves a browser once belonged to the operator. On its
//! own it authorises nothing — it only makes the quick unlock available, so a
//! stolen laptop still needs the password. The **session token** is what
//! actually authorises requests, and it does not expire mid-use.
//!
//! Two tiers remain, and neither involves a clock:
//!
//! | Tier | Requires |
//! |---|---|
//! | `Public` | tailnet reachability only — health, the login page |
//! | `Session` | an unlocked session |
//!
//! The network boundary and the auth boundary stay independent: the server
//! binds the tailnet interface, so a failure of this module does not expose
//! Prism to the internet, and vice versa.

pub mod password;
pub mod session;
pub mod totp;

use session::{Claims, SessionKey, TokenError, TokenKind};
use std::collections::HashSet;
use std::sync::Mutex;

/// What an endpoint demands of its caller.
///
/// `Fresh` used to sit above `Session` and required a recent TOTP code. It was
/// removed rather than relaxed: a tier defined by a clock cannot be made
/// pleasant, only less frequent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sensitivity {
    Public,
    Session,
}

#[derive(Debug, Clone, Copy)]
pub struct AuthPolicy {
    /// How long a browser stays enrolled.
    pub device_ttl_secs: u64,
    /// How long an unlocked session lasts.
    pub session_ttl_secs: u64,
    /// TOTP steps of clock tolerance either side of now.
    pub totp_skew_steps: u64,
    /// Failed attempts before lockout.
    pub max_failures: u32,
    /// How long a lockout lasts.
    pub lockout_secs: u64,
}

impl Default for AuthPolicy {
    fn default() -> Self {
        Self {
            // A year. Enrolling a device is a deliberate act with the phone in
            // hand; making it expire quietly would reintroduce the very
            // interruption this design removes.
            device_ttl_secs: 365 * 24 * 3600,
            // 30 days. Long enough that the password is rare, short enough that
            // a forgotten browser does not stay unlocked indefinitely.
            session_ttl_secs: 30 * 24 * 3600,
            // ±30s of drift. Each extra step is another simultaneously-valid
            // code, so this stays tight.
            totp_skew_steps: 1,
            max_failures: 5,
            lockout_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    Granted,
    /// No valid session. The client decides what to show: a password prompt if
    /// the device is enrolled, otherwise an authenticator code.
    Unauthenticated,
    /// Too many failures; retry later.
    LockedOut { retry_after_secs: u64 },
}

/// What the login screen should ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginPrompt {
    /// This browser is enrolled and a password is set: ask for the password.
    Password,
    /// Not enrolled, or no password set: ask for an authenticator code.
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeOutcome {
    Accepted,
    Rejected,
    /// Correct code, but already used. A valid code is single-use within its
    /// step, so an intercepted code cannot be replayed inside its 30s life.
    Replayed,
    LockedOut { retry_after_secs: u64 },
}

struct Failures {
    count: u32,
    locked_until: u64,
}

pub struct Authenticator {
    key: SessionKey,
    secret: Vec<u8>,
    policy: AuthPolicy,
    /// Argon2 hash of the unlock password, when one is set.
    password_hash: Mutex<Option<String>>,
    /// Counters already spent. Pruned as time advances.
    consumed: Mutex<HashSet<u64>>,
    failures: Mutex<Failures>,
}

impl Authenticator {
    pub fn new(key: SessionKey, secret: Vec<u8>, policy: AuthPolicy) -> Self {
        Self {
            key,
            secret,
            policy,
            password_hash: Mutex::new(None),
            consumed: Mutex::new(HashSet::new()),
            failures: Mutex::new(Failures {
                count: 0,
                locked_until: 0,
            }),
        }
    }

    pub fn with_password_hash(self, hash: Option<String>) -> Self {
        *self.password_hash.lock().expect("password hash poisoned") = hash;
        self
    }

    pub fn has_password(&self) -> bool {
        self.password_hash
            .lock()
            .expect("password hash poisoned")
            .is_some()
    }

    pub fn policy(&self) -> &AuthPolicy {
        &self.policy
    }

    /// What the login screen should ask this caller for.
    ///
    /// A password prompt only appears once the browser is enrolled *and* a
    /// password exists; otherwise the code is the only way in, which keeps a
    /// fresh device from being offered a shortcut it cannot use.
    pub fn prompt_for(&self, device_token: Option<&str>, now: u64) -> LoginPrompt {
        if !self.has_password() {
            return LoginPrompt::Code;
        }
        match device_token.map(|t| self.key.validate(t, now)) {
            Some(Ok(claims)) if claims.kind == TokenKind::Device => LoginPrompt::Password,
            _ => LoginPrompt::Code,
        }
    }

    /// Unlock with the password. Requires an enrolled device.
    ///
    /// Without that requirement the password would be a single factor reachable
    /// from any browser on the tailnet, which is materially weaker than what it
    /// replaced.
    pub fn submit_password(
        &self,
        password: &str,
        device_token: Option<&str>,
        now: u64,
    ) -> (CodeOutcome, Option<String>) {
        if let Some(retry) = self.locked_for(now) {
            return (CodeOutcome::LockedOut { retry_after_secs: retry }, None);
        }
        if self.prompt_for(device_token, now) != LoginPrompt::Password {
            return (CodeOutcome::Rejected, None);
        }
        let stored = self.password_hash.lock().expect("password hash poisoned").clone();
        let Some(stored) = stored else {
            return (CodeOutcome::Rejected, None);
        };

        if password::verify(password, &stored) {
            self.reset_failures();
            (CodeOutcome::Accepted, Some(self.issue_session(now)))
        } else {
            match self.record_failure(now) {
                Some(secs) => (CodeOutcome::LockedOut { retry_after_secs: secs }, None),
                None => (CodeOutcome::Rejected, None),
            }
        }
    }

    fn issue_session(&self, now: u64) -> String {
        self.key.issue(Claims {
            expires_at: now + self.policy.session_ttl_secs,
            kind: TokenKind::Session,
        })
    }

    fn issue_device(&self, now: u64) -> String {
        self.key.issue(Claims {
            expires_at: now + self.policy.device_ttl_secs,
            kind: TokenKind::Device,
        })
    }

    /// Submit a TOTP code. On success, enrol the device *and* unlock a session.
    ///
    /// Returns `(outcome, session, device)`. Doing both at once means entering a
    /// code is a complete sign-in, not a step toward one.
    pub fn submit_code(
        &self,
        code: &str,
        now: u64,
    ) -> (CodeOutcome, Option<String>, Option<String>) {
        if let Some(retry) = self.locked_for(now) {
            return (
                CodeOutcome::LockedOut {
                    retry_after_secs: retry,
                },
                None,
                None,
            );
        }

        match totp::verify(&self.secret, code, now, self.policy.totp_skew_steps) {
            totp::VerifyOutcome::Valid { counter } => {
                let mut consumed = self.consumed.lock().expect("consumed set poisoned");
                // Prune counters that can no longer be presented anyway.
                let floor = totp::counter_at(now).saturating_sub(self.policy.totp_skew_steps + 1);
                consumed.retain(|c| *c >= floor);

                if !consumed.insert(counter) {
                    // A correct but already-spent code is not a failed guess, so
                    // it must not count toward lockout — otherwise a replayed
                    // code could be used to lock the real operator out.
                    return (CodeOutcome::Replayed, None, None);
                }
                drop(consumed);

                self.reset_failures();
                (
                    CodeOutcome::Accepted,
                    Some(self.issue_session(now)),
                    Some(self.issue_device(now)),
                )
            }
            totp::VerifyOutcome::Invalid => {
                let retry = self.record_failure(now);
                match retry {
                    Some(secs) => (
                        CodeOutcome::LockedOut {
                            retry_after_secs: secs,
                        },
                        None,
                        None,
                    ),
                    None => (CodeOutcome::Rejected, None, None),
                }
            }
        }
    }

    /// Decide whether a request carrying `token` may reach `required`.
    pub fn authorize(
        &self,
        token: Option<&str>,
        required: Sensitivity,
        now: u64,
    ) -> AuthOutcome {
        if required == Sensitivity::Public {
            return AuthOutcome::Granted;
        }
        if let Some(retry) = self.locked_for(now) {
            return AuthOutcome::LockedOut {
                retry_after_secs: retry,
            };
        }

        let Some(token) = token else {
            return AuthOutcome::Unauthenticated;
        };
        let claims = match self.key.validate(token, now) {
            Ok(claims) => claims,
            Err(TokenError::Expired) | Err(TokenError::WrongVersion) => {
                return AuthOutcome::Unauthenticated;
            }
            Err(_) => return AuthOutcome::Unauthenticated,
        };

        // Only a session token authorises. A device token proves enrolment and
        // nothing more, or a stolen laptop would need no password.
        match claims.kind {
            TokenKind::Session => AuthOutcome::Granted,
            TokenKind::Device => AuthOutcome::Unauthenticated,
        }
    }

    fn locked_for(&self, now: u64) -> Option<u64> {
        let failures = self.failures.lock().expect("failure state poisoned");
        (failures.locked_until > now).then(|| failures.locked_until - now)
    }

    fn record_failure(&self, now: u64) -> Option<u64> {
        let mut failures = self.failures.lock().expect("failure state poisoned");
        failures.count += 1;
        if failures.count >= self.policy.max_failures {
            failures.locked_until = now + self.policy.lockout_secs;
            failures.count = 0;
            return Some(self.policy.lockout_secs);
        }
        None
    }

    fn reset_failures(&self) {
        let mut failures = self.failures.lock().expect("failure state poisoned");
        failures.count = 0;
        failures.locked_until = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"12345678901234567890";
    const NOW: u64 = 1_700_000_000;
    const PASSWORD: &str = "a good long password";

    fn auth() -> Authenticator {
        Authenticator::new(
            SessionKey::from_bytes([3u8; 32]),
            SECRET.to_vec(),
            AuthPolicy::default(),
        )
    }

    fn auth_with_password() -> Authenticator {
        auth().with_password_hash(Some(password::hash(PASSWORD).unwrap()))
    }

    fn code_at(t: u64) -> String {
        totp::format_code(totp::totp_at(SECRET, t))
    }

    #[test]
    fn public_needs_nothing() {
        assert_eq!(
            auth().authorize(None, Sensitivity::Public, NOW),
            AuthOutcome::Granted
        );
    }

    #[test]
    fn session_tier_rejects_missing_token() {
        assert_eq!(
            auth().authorize(None, Sensitivity::Session, NOW),
            AuthOutcome::Unauthenticated
        );
    }

    #[test]
    fn a_code_both_unlocks_and_enrols() {
        // Entering a code is a complete sign-in, not a step toward one.
        let a = auth();
        let (outcome, session, device) = a.submit_code(&code_at(NOW), NOW);
        assert_eq!(outcome, CodeOutcome::Accepted);
        assert!(session.is_some() && device.is_some());
        assert_eq!(
            a.authorize(session.as_deref(), Sensitivity::Session, NOW),
            AuthOutcome::Granted
        );
    }

    /// The whole point of the redesign.
    #[test]
    fn a_session_does_not_lapse_while_it_is_being_used() {
        // The old design demanded a fresh code every 15 minutes. A session now
        // lasts its full lifetime with no interruption.
        let a = auth();
        let (_, session, _) = a.submit_code(&code_at(NOW), NOW);
        let token = session.unwrap();
        for hours in [1, 6, 24, 24 * 20] {
            let later = NOW + hours * 3600;
            assert_eq!(
                a.authorize(Some(&token), Sensitivity::Session, later),
                AuthOutcome::Granted,
                "should still be authorised {hours}h later"
            );
        }
    }

    #[test]
    fn a_device_token_alone_authorises_nothing() {
        // Otherwise a stolen laptop would need no password.
        let a = auth();
        let (_, _, device) = a.submit_code(&code_at(NOW), NOW);
        assert_eq!(
            a.authorize(device.as_deref(), Sensitivity::Session, NOW),
            AuthOutcome::Unauthenticated
        );
    }

    #[test]
    fn an_enrolled_device_with_a_password_is_asked_for_the_password() {
        let a = auth_with_password();
        let (_, _, device) = a.submit_code(&code_at(NOW), NOW);
        assert_eq!(a.prompt_for(device.as_deref(), NOW), LoginPrompt::Password);
    }

    #[test]
    fn an_unenrolled_device_is_asked_for_a_code() {
        let a = auth_with_password();
        assert_eq!(a.prompt_for(None, NOW), LoginPrompt::Code);
    }

    #[test]
    fn with_no_password_set_the_code_is_the_only_way_in() {
        // A fresh install must not offer a shortcut that cannot be used.
        let a = auth();
        let (_, _, device) = a.submit_code(&code_at(NOW), NOW);
        assert_eq!(a.prompt_for(device.as_deref(), NOW), LoginPrompt::Code);
        assert_eq!(
            a.submit_password(PASSWORD, device.as_deref(), NOW).0,
            CodeOutcome::Rejected
        );
    }

    #[test]
    fn the_password_unlocks_an_enrolled_device() {
        let a = auth_with_password();
        let (_, _, device) = a.submit_code(&code_at(NOW), NOW);
        let (outcome, session) = a.submit_password(PASSWORD, device.as_deref(), NOW);
        assert_eq!(outcome, CodeOutcome::Accepted);
        assert_eq!(
            a.authorize(session.as_deref(), Sensitivity::Session, NOW),
            AuthOutcome::Granted
        );
    }

    #[test]
    fn the_password_is_useless_without_an_enrolled_device() {
        // Otherwise it would be a single factor reachable from any browser on
        // the tailnet — weaker than what it replaced.
        let a = auth_with_password();
        assert_eq!(
            a.submit_password(PASSWORD, None, NOW).0,
            CodeOutcome::Rejected
        );
    }

    #[test]
    fn a_wrong_password_is_refused() {
        let a = auth_with_password();
        let (_, _, device) = a.submit_code(&code_at(NOW), NOW);
        assert_eq!(
            a.submit_password("not the password", device.as_deref(), NOW).0,
            CodeOutcome::Rejected
        );
    }

    #[test]
    fn a_code_cannot_be_replayed_within_its_window() {
        let a = auth();
        let code = code_at(NOW);
        assert_eq!(a.submit_code(&code, NOW).0, CodeOutcome::Accepted);
        assert_eq!(
            a.submit_code(&code, NOW + 1).0,
            CodeOutcome::Replayed,
            "an intercepted code must not be reusable inside its 30s life"
        );
    }

    #[test]
    fn replay_does_not_count_toward_lockout() {
        // Otherwise anyone who captured one code could lock the operator out.
        let a = auth();
        let code = code_at(NOW);
        a.submit_code(&code, NOW);
        for _ in 0..10 {
            assert_eq!(a.submit_code(&code, NOW).0, CodeOutcome::Replayed);
        }
        let (_, session, _) = a.submit_code(&code_at(NOW + 60), NOW + 60);
        assert!(session.is_some(), "real operator must still be able to log in");
    }

    #[test]
    fn repeated_wrong_codes_lock_out() {
        let a = auth();
        let mut locked = false;
        for _ in 0..a.policy().max_failures {
            if let CodeOutcome::LockedOut { .. } = a.submit_code("000000", NOW).0 {
                locked = true;
            }
        }
        assert!(locked, "should lock out after max_failures");
        assert!(matches!(
            a.submit_code(&code_at(NOW), NOW).0,
            CodeOutcome::LockedOut { .. }
        ));
    }

    #[test]
    fn wrong_passwords_lock_out_too() {
        // The quick path must not be a cheaper way to guess.
        let a = auth_with_password();
        let (_, _, device) = a.submit_code(&code_at(NOW), NOW);
        let mut locked = false;
        for _ in 0..a.policy().max_failures {
            if let CodeOutcome::LockedOut { .. } =
                a.submit_password("wrong", device.as_deref(), NOW).0
            {
                locked = true;
            }
        }
        assert!(locked);
    }

    #[test]
    fn lockout_expires() {
        let a = auth();
        for _ in 0..a.policy().max_failures {
            a.submit_code("000000", NOW);
        }
        let after = NOW + a.policy().lockout_secs + 1;
        assert_eq!(a.submit_code(&code_at(after), after).0, CodeOutcome::Accepted);
    }

    #[test]
    fn forged_tokens_are_unauthenticated() {
        let a = auth();
        assert_eq!(
            a.authorize(Some("v1.99999999999.s.deadbeef"), Sensitivity::Session, NOW),
            AuthOutcome::Unauthenticated
        );
    }

    #[test]
    fn expired_sessions_are_unauthenticated() {
        let a = auth();
        let (_, session, _) = a.submit_code(&code_at(NOW), NOW);
        let expired = NOW + a.policy().session_ttl_secs + 1;
        assert_eq!(
            a.authorize(session.as_deref(), Sensitivity::Session, expired),
            AuthOutcome::Unauthenticated
        );
    }

    #[test]
    fn a_device_stays_enrolled_far_longer_than_a_session() {
        // Re-enrolling needs the phone; re-unlocking does not. The asymmetry is
        // deliberate.
        let p = AuthPolicy::default();
        assert!(p.device_ttl_secs > p.session_ttl_secs * 10);
    }

    #[test]
    fn tiers_are_ordered_by_strictness() {
        assert!(Sensitivity::Public < Sensitivity::Session);
    }
}
