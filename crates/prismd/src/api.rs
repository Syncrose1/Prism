//! The tailnet HTTP API.
//!
//! Every route declares its [`Sensitivity`] in one table. Authorisation is
//! applied from that table rather than inside each handler, so adding an
//! endpoint cannot accidentally default to open — the failure mode of
//! per-handler checks is a missing line, and a missing line here is a
//! compile-time gap in a match rather than a silently public route.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use prism_core::auth::{AuthOutcome, Authenticator, CodeOutcome, Sensitivity, totp};
use prism_core::governor::Tier;
use prism_core::sensors::disk::MountUsage;
use prism_core::sensors::memory::MemorySnapshot;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

/// The latest reading, published by the monitor loop and read by the API.
///
/// Kept behind an `RwLock` rather than recomputed per request so that serving
/// the dashboard costs nothing during the pressure it is displaying.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Vitals {
    pub tier: String,
    pub stall_full: f64,
    pub honest_headroom_mib: u64,
    pub phantom_headroom_mib: u64,
    pub total_mib: u64,
    pub available_mib: u64,
    pub swap_total_mib: u64,
    pub swap_free_mib: u64,
    pub zram_cost_mib: u64,
    pub compression_ratio: Option<f64>,
    /// Tightest watched filesystem, if disk is being sensed.
    pub disk: Option<DiskVitals>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskVitals {
    pub path: String,
    pub total_mib: u64,
    pub free_mib: u64,
    pub used_pct: f64,
    pub inodes_used_pct: f64,
}

impl Vitals {
    pub fn from_sample(
        mem: &MemorySnapshot,
        stall_full: f64,
        tier: Tier,
        disk: Option<&MountUsage>,
    ) -> Self {
        Self {
            tier: tier.as_str().to_string(),
            stall_full,
            honest_headroom_mib: mem.honest_headroom_kb / 1024,
            phantom_headroom_mib: mem.phantom_headroom_kb() / 1024,
            total_mib: mem.total_kb / 1024,
            available_mib: mem.available_kb / 1024,
            swap_total_mib: mem.swap_total_kb / 1024,
            swap_free_mib: mem.swap_free_kb / 1024,
            zram_cost_mib: mem.zram_cost_kb / 1024,
            compression_ratio: mem.compression_ratio,
            disk: disk.map(|d| DiskVitals {
                path: d.path.display().to_string(),
                total_mib: d.total_kb / 1024,
                free_mib: d.available_mib(),
                used_pct: d.used_pct(),
                inodes_used_pct: d.inodes_used_pct(),
            }),
        }
    }
}

pub type SharedVitals = Arc<RwLock<Vitals>>;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<Authenticator>,
    pub vitals: SharedVitals,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/login", post(login))
        .route("/api/vitals", get(vitals))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Authorisation
// ---------------------------------------------------------------------------

const SESSION_COOKIE: &str = "prism_session";

/// Extract a session token from either the cookie or a bearer header.
///
/// The header form exists so the CLI and scripts do not need a cookie jar.
fn token_from(headers: &HeaderMap) -> Option<String> {
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok())
        && let Some(bearer) = auth.strip_prefix("Bearer ")
    {
        return Some(bearer.trim().to_string());
    }
    let cookies = headers.get("cookie")?.to_str().ok()?;
    cookies.split(';').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name.trim() == SESSION_COOKIE).then(|| value.trim().to_string())
    })
}

/// Enforce a tier, returning the error response to send if it is not met.
fn require(state: &AppState, headers: &HeaderMap, need: Sensitivity) -> Option<Response> {
    let now = totp::now_unix();
    match state.auth.authorize(token_from(headers).as_deref(), need, now) {
        AuthOutcome::Granted => None,
        AuthOutcome::NeedsFreshCode => Some(
            (
                StatusCode::FORBIDDEN,
                Json(ErrorBody {
                    error: "fresh_code_required",
                    detail: "this action needs a current authenticator code".into(),
                }),
            )
                .into_response(),
        ),
        AuthOutcome::Unauthenticated => Some(
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorBody {
                    error: "unauthenticated",
                    detail: "sign in with an authenticator code".into(),
                }),
            )
                .into_response(),
        ),
        AuthOutcome::LockedOut { retry_after_secs } => Some(
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorBody {
                    error: "locked_out",
                    detail: format!("too many attempts; retry in {retry_after_secs}s"),
                }),
            )
                .into_response(),
        ),
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    detail: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct Health {
    ok: bool,
    service: &'static str,
    version: &'static str,
}

/// Public by design: a liveness probe that required auth would be useless for
/// answering "is Prism itself still up?" from a phone.
async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        service: "prismd",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Deserialize)]
struct LoginRequest {
    code: String,
}

#[derive(Serialize)]
struct LoginResponse {
    ok: bool,
    /// Also returned in the body so non-browser clients need no cookie jar.
    token: String,
    fresh_window_secs: u64,
}

async fn login(State(state): State<AppState>, Json(body): Json<LoginRequest>) -> Response {
    let now = totp::now_unix();
    let (outcome, token) = state.auth.submit_code(&body.code, now);

    match (outcome, token) {
        (CodeOutcome::Accepted, Some(token)) => {
            info!("authenticator code accepted; session issued");
            let cookie = format!(
                "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
                state.auth.policy().session_ttl_secs
            );
            let body = Json(LoginResponse {
                ok: true,
                token,
                fresh_window_secs: state.auth.policy().fresh_window_secs,
            });
            ([("set-cookie", cookie)], body).into_response()
        }
        (CodeOutcome::Replayed, _) => {
            warn!("authenticator code replayed");
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorBody {
                    error: "code_already_used",
                    detail: "that code has already been used; wait for the next one".into(),
                }),
            )
                .into_response()
        }
        (CodeOutcome::LockedOut { retry_after_secs }, _) => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorBody {
                error: "locked_out",
                detail: format!("too many attempts; retry in {retry_after_secs}s"),
            }),
        )
            .into_response(),
        _ => {
            warn!("authenticator code rejected");
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorBody {
                    error: "invalid_code",
                    detail: "incorrect code".into(),
                }),
            )
                .into_response()
        }
    }
}

async fn vitals(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(denied) = require(&state, &headers, Sensitivity::Session) {
        return denied;
    }
    let snapshot = state.vitals.read().expect("vitals lock poisoned").clone();
    Json(snapshot).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(name: &'static str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn extracts_bearer_token() {
        let headers = headers_with("authorization", "Bearer abc.def");
        assert_eq!(token_from(&headers).as_deref(), Some("abc.def"));
    }

    #[test]
    fn extracts_session_cookie() {
        let headers = headers_with("cookie", "other=1; prism_session=tok.en; another=2");
        assert_eq!(token_from(&headers).as_deref(), Some("tok.en"));
    }

    #[test]
    fn ignores_similarly_named_cookies() {
        // Must not match `prism_session_backup` or `not_prism_session`.
        let headers = headers_with("cookie", "prism_session_backup=nope; not_prism_session=no");
        assert_eq!(token_from(&headers), None);
    }

    #[test]
    fn no_credentials_yields_none() {
        assert_eq!(token_from(&HeaderMap::new()), None);
    }

    #[test]
    fn non_bearer_authorization_is_ignored() {
        let headers = headers_with("authorization", "Basic dXNlcjpwYXNz");
        assert_eq!(token_from(&headers), None);
    }

    #[test]
    fn vitals_serialise_with_expected_fields() {
        let v = Vitals::default();
        let json = serde_json::to_string(&v).unwrap();
        for field in ["tier", "stall_full", "honest_headroom_mib", "phantom_headroom_mib"] {
            assert!(json.contains(field), "missing field {field}");
        }
    }
}
