//! Tiered authentication.
//!
//! The operator asked for convenience *and* privacy, which are only compatible
//! if the cost of authenticating scales with what is being reached. Three tiers:
//!
//! | Tier | Requires | Guards |
//! |---|---|---|
//! | `Public` | tailnet reachability only | health readouts, the login page |
//! | `Session` | a valid session cookie | starting/stopping facets, limits |
//! | `Fresh` | a TOTP code within the last few minutes | files, media, config changes |
//!
//! The network boundary and the auth boundary are deliberately independent: the
//! server binds the tailnet interface, so even a total failure of this module
//! does not expose Prism to the internet, and vice versa.
//!
//! `Fresh` exists because a 30-day session on a phone is a different security
//! proposition from proving possession of that phone right now. Reading a memory
//! graph should not demand a code; downloading the filesystem should.

pub mod session;
pub mod totp;

use session::{Claims, SessionKey, TokenError};
use std::collections::HashSet;
use std::sync::Mutex;

/// What an endpoint demands of its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sensitivity {
    Public,
    Session,
    Fresh,
}

#[derive(Debug, Clone, Copy)]
pub struct AuthPolicy {
    /// How long a TOTP verification counts as "fresh".
    pub fresh_window_secs: u64,
    /// Session lifetime.
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
            // Long enough to browse and download without re-entering a code,
            // short enough that a borrowed unlocked phone is not a filesystem.
            fresh_window_secs: 900,
            // 30 days: the dashboard should not demand a login every visit.
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
    /// Authenticated, but this tier needs a fresh code.
    NeedsFreshCode,
    /// No valid session.
    Unauthenticated,
    /// Too many failures; retry later.
    LockedOut { retry_after_secs: u64 },
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
            consumed: Mutex::new(HashSet::new()),
            failures: Mutex::new(Failures {
                count: 0,
                locked_until: 0,
            }),
        }
    }

    pub fn policy(&self) -> &AuthPolicy {
        &self.policy
    }

    /// Submit a TOTP code. On success, issue or refresh a session.
    pub fn submit_code(&self, code: &str, now: u64) -> (CodeOutcome, Option<String>) {
        if let Some(retry) = self.locked_for(now) {
            return (
                CodeOutcome::LockedOut {
                    retry_after_secs: retry,
                },
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
                    return (CodeOutcome::Replayed, None);
                }
                drop(consumed);

                self.reset_failures();
                let token = self.key.issue(Claims {
                    expires_at: now + self.policy.session_ttl_secs,
                    totp_verified_at: now,
                });
                (CodeOutcome::Accepted, Some(token))
            }
            totp::VerifyOutcome::Invalid => {
                let retry = self.record_failure(now);
                match retry {
                    Some(secs) => (
                        CodeOutcome::LockedOut {
                            retry_after_secs: secs,
                        },
                        None,
                    ),
                    None => (CodeOutcome::Rejected, None),
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

        match required {
            Sensitivity::Public => AuthOutcome::Granted,
            Sensitivity::Session => AuthOutcome::Granted,
            Sensitivity::Fresh => {
                let age = now.saturating_sub(claims.totp_verified_at);
                if age <= self.policy.fresh_window_secs {
                    AuthOutcome::Granted
                } else {
                    AuthOutcome::NeedsFreshCode
                }
            }
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

    fn auth() -> Authenticator {
        Authenticator::new(
            SessionKey::from_bytes([3u8; 32]),
            SECRET.to_vec(),
            AuthPolicy::default(),
        )
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
    fn valid_code_issues_a_session_that_opens_all_tiers() {
        let a = auth();
        let (outcome, token) = a.submit_code(&code_at(NOW), NOW);
        assert_eq!(outcome, CodeOutcome::Accepted);
        let token = token.expect("token issued");

        for tier in [Sensitivity::Public, Sensitivity::Session, Sensitivity::Fresh] {
            assert_eq!(
                a.authorize(Some(&token), tier, NOW),
                AuthOutcome::Granted,
                "tier {tier:?} should be open immediately after a code"
            );
        }
    }

    #[test]
    fn freshness_decays_but_the_session_survives() {
        let a = auth();
        let (_, token) = a.submit_code(&code_at(NOW), NOW);
        let token = token.unwrap();
        let later = NOW + a.policy().fresh_window_secs + 1;

        // Still logged in for ordinary control...
        assert_eq!(
            a.authorize(Some(&token), Sensitivity::Session, later),
            AuthOutcome::Granted
        );
        // ...but files need proof of the phone again.
        assert_eq!(
            a.authorize(Some(&token), Sensitivity::Fresh, later),
            AuthOutcome::NeedsFreshCode
        );
    }

    #[test]
    fn freshness_boundary_is_inclusive() {
        let a = auth();
        let (_, token) = a.submit_code(&code_at(NOW), NOW);
        let token = token.unwrap();
        let edge = NOW + a.policy().fresh_window_secs;
        assert_eq!(
            a.authorize(Some(&token), Sensitivity::Fresh, edge),
            AuthOutcome::Granted
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
        // Otherwise anyone who captured one code could lock the operator out by
        // resubmitting it.
        let a = auth();
        let code = code_at(NOW);
        a.submit_code(&code, NOW);
        for _ in 0..10 {
            assert_eq!(a.submit_code(&code, NOW).0, CodeOutcome::Replayed);
        }
        let (_, token) = a.submit_code(&code_at(NOW + 60), NOW + 60);
        assert!(token.is_some(), "real operator must still be able to log in");
    }

    #[test]
    fn repeated_wrong_codes_lock_out() {
        let a = auth();
        let wrong = "000000";
        let mut locked = false;
        for _ in 0..a.policy().max_failures {
            if let CodeOutcome::LockedOut { .. } = a.submit_code(wrong, NOW).0 {
                locked = true;
            }
        }
        assert!(locked, "should lock out after max_failures");
        // And a correct code is refused while locked.
        assert!(matches!(
            a.submit_code(&code_at(NOW), NOW).0,
            CodeOutcome::LockedOut { .. }
        ));
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
    fn successful_login_clears_the_failure_count() {
        let a = auth();
        // Four failures, one short of lockout.
        for _ in 0..(a.policy().max_failures - 1) {
            a.submit_code("000000", NOW);
        }
        assert_eq!(a.submit_code(&code_at(NOW), NOW).0, CodeOutcome::Accepted);
        // The counter reset, so four more failures must not lock out.
        for _ in 0..(a.policy().max_failures - 1) {
            assert_eq!(a.submit_code("000000", NOW + 60).0, CodeOutcome::Rejected);
        }
    }

    #[test]
    fn forged_token_is_unauthenticated_at_every_tier() {
        let a = auth();
        for tier in [Sensitivity::Session, Sensitivity::Fresh] {
            assert_eq!(
                a.authorize(Some("v1.99999999999.99999999999.deadbeef"), tier, NOW),
                AuthOutcome::Unauthenticated
            );
        }
    }

    #[test]
    fn expired_session_is_unauthenticated_not_merely_stale() {
        let a = auth();
        let (_, token) = a.submit_code(&code_at(NOW), NOW);
        let token = token.unwrap();
        let expired = NOW + a.policy().session_ttl_secs + 1;
        assert_eq!(
            a.authorize(Some(&token), Sensitivity::Session, expired),
            AuthOutcome::Unauthenticated
        );
    }

    #[test]
    fn tiers_are_ordered_by_strictness() {
        assert!(Sensitivity::Public < Sensitivity::Session);
        assert!(Sensitivity::Session < Sensitivity::Fresh);
    }
}
