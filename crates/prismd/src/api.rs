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
use prism_core::auth::{AuthOutcome, Authenticator, CodeOutcome, LoginPrompt, Sensitivity, totp};
use prism_core::config::Facet;
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
    pub facets: Arc<RwLock<Vec<Facet>>>,
    pub profile_path: Arc<std::path::PathBuf>,
    pub state_dir: Arc<std::path::PathBuf>,
    pub events: Arc<prism_core::events::EventLog>,
    pub proxy: crate::proxy::ProxyClient,
    pub terminals: Arc<prism_core::term::session::SessionManager>,
    pub roots: Arc<Vec<prism_core::files::path::Root>>,
    pub thumb_dir: Arc<std::path::PathBuf>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/login", post(login))
        .route("/api/auth/prompt", get(login_prompt))
        .route("/api/vitals", get(vitals))
        .route("/api/events", get(events))
        // Critical Functions Mode. Merged rather than nested so it shares no
        // middleware with the API surface — see ADR 0002.
        .merge(crate::rescue::routes())
        .merge(crate::term_api::routes())
        .merge(crate::files_api::routes())
        .merge(crate::facets_api::routes())
        .merge(crate::workspace::routes())
        .merge(crate::proxy::routes())
        .route("/", get(crate::ui::index))
        .route("/ui/{*path}", get(crate::ui::asset))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Authorisation
// ---------------------------------------------------------------------------

const SESSION_COOKIE: &str = "prism_session";
/// Long-lived, and separate on purpose: clearing a session must not un-enrol the
/// browser, or every sign-in would need the phone again.
const DEVICE_COOKIE: &str = "prism_device";

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookies = headers.get("cookie")?.to_str().ok()?;
    cookies.split(';').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        (k.trim() == name).then(|| v.trim().to_string())
    })
}

/// The session token, from either the cookie or a bearer header.
///
/// The header form exists so the CLI and scripts do not need a cookie jar.
pub(crate) fn session_token(headers: &HeaderMap) -> Option<String> {
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok())
        && let Some(bearer) = auth.strip_prefix("Bearer ")
    {
        return Some(bearer.trim().to_string());
    }
    cookie(headers, SESSION_COOKIE)
}

pub(crate) fn device_token(headers: &HeaderMap) -> Option<String> {
    cookie(headers, DEVICE_COOKIE)
}

fn session_cookie(token: &str, ttl: u64) -> String {
    format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ttl}")
}

fn device_cookie(token: &str, ttl: u64) -> String {
    format!("{DEVICE_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ttl}")
}

/// Enforce a tier, returning the error response to send if it is not met.
pub(crate) fn require(state: &AppState, headers: &HeaderMap, need: Sensitivity) -> Option<Response> {
    let now = totp::now_unix();
    match state.auth.authorize(session_token(headers).as_deref(), need, now) {
        AuthOutcome::Granted => None,
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
    /// What this host can actually do. Reported rather than assumed so a
    /// degraded environment — a container without cgroup delegation, a kernel
    /// without PSI — is visible instead of silently pretending to contain
    /// workloads it cannot.
    platform: prism_core::platform::Capabilities,
}

/// Public by design: a liveness probe that required auth would be useless for
/// answering "is Prism itself still up?" from a phone.
async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        service: "prismd",
        version: env!("CARGO_PKG_VERSION"),
        platform: prism_core::platform::capabilities(),
    })
}

#[derive(Deserialize)]
struct LoginRequest {
    /// An authenticator code, when enrolling this browser.
    #[serde(default)]
    code: Option<String>,
    /// The unlock password, when the browser is already enrolled.
    #[serde(default)]
    password: Option<String>,
}

#[derive(Serialize)]
struct LoginResponse {
    ok: bool,
    /// Also returned in the body so non-browser clients need no cookie jar.
    token: String,
    /// True when this sign-in also enrolled the browser.
    enrolled: bool,
}

#[derive(Serialize)]
struct PromptResponse {
    /// "password" when this browser is enrolled and a password is set,
    /// otherwise "code".
    prompt: &'static str,
    has_password: bool,
}

/// What the login screen should ask for. Public: it reveals only whether *this*
/// browser is already enrolled, which that browser necessarily knows.
async fn login_prompt(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let now = totp::now_unix();
    let prompt = state
        .auth
        .prompt_for(device_token(&headers).as_deref(), now);
    Json(PromptResponse {
        prompt: match prompt {
            LoginPrompt::Password => "password",
            LoginPrompt::Code => "code",
        },
        has_password: state.auth.has_password(),
    })
    .into_response()
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Response {
    let now = totp::now_unix();
    let policy = *state.auth.policy();

    // A code enrols the browser and unlocks in one step; a password only
    // unlocks, and only on a browser already enrolled.
    let (outcome, session, device) = match (&body.code, &body.password) {
        (Some(code), _) if !code.trim().is_empty() => state.auth.submit_code(code, now),
        (_, Some(pw)) if !pw.is_empty() => {
            let (o, s) = state
                .auth
                .submit_password(pw, device_token(&headers).as_deref(), now);
            (o, s, None)
        }
        _ => {
            return err_json(
                StatusCode::BAD_REQUEST,
                "no_credential",
                "provide a code or a password",
            );
        }
    };

    match (outcome, session) {
        (CodeOutcome::Accepted, Some(token)) => {
            info!(enrolled = device.is_some(), "signed in");
            // Two Set-Cookie headers when enrolling, which needs a hand-built
            // response — a header map cannot hold the same name twice via the
            // tuple form.
            let body = Json(LoginResponse {
                ok: true,
                token: token.clone(),
                enrolled: device.is_some(),
            });
            let mut response = body.into_response();
            let out = response.headers_mut();
            if let Ok(v) = session_cookie(&token, policy.session_ttl_secs).parse() {
                out.append(axum::http::header::SET_COOKIE, v);
            }
            if let Some(d) = &device
                && let Ok(v) = device_cookie(d, policy.device_ttl_secs).parse()
            {
                out.append(axum::http::header::SET_COOKIE, v);
            }
            response
        }
        (CodeOutcome::Replayed, _) => {
            warn!("authenticator code replayed");
            err_json(
                StatusCode::UNAUTHORIZED,
                "code_already_used",
                "that code has already been used; wait for the next one",
            )
        }
        (CodeOutcome::LockedOut { retry_after_secs }, _) => err_json(
            StatusCode::TOO_MANY_REQUESTS,
            "locked_out",
            format!("too many attempts; retry in {retry_after_secs}s"),
        ),
        _ => {
            warn!("sign-in rejected");
            err_json(StatusCode::UNAUTHORIZED, "invalid", "incorrect")
        }
    }
}

fn err_json(status: StatusCode, error: &'static str, detail: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorBody {
            error,
            detail: detail.into(),
        }),
    )
        .into_response()
}

/// The Timeline's source. Prism's own actions appear here alongside what it
/// observed, which is the point — see `events.rs`.
async fn events(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(denied) = require(&state, &headers, Sensitivity::Session) {
        return denied;
    }
    Json(state.events.recent(200)).into_response()
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
        assert_eq!(session_token(&headers).as_deref(), Some("abc.def"));
    }

    #[test]
    fn extracts_session_cookie() {
        let headers = headers_with("cookie", "other=1; prism_session=tok.en; another=2");
        assert_eq!(session_token(&headers).as_deref(), Some("tok.en"));
    }

    #[test]
    fn ignores_similarly_named_cookies() {
        // Must not match `prism_session_backup` or `not_prism_session`.
        let headers = headers_with("cookie", "prism_session_backup=nope; not_prism_session=no");
        assert_eq!(session_token(&headers), None);
    }

    #[test]
    fn no_credentials_yields_none() {
        assert_eq!(session_token(&HeaderMap::new()), None);
    }

    #[test]
    fn non_bearer_authorization_is_ignored() {
        let headers = headers_with("authorization", "Basic dXNlcjpwYXNz");
        assert_eq!(session_token(&headers), None);
    }

    #[test]
    fn session_and_device_cookies_are_read_independently() {
        // Clearing a session must not un-enrol the browser, so the two must
        // never be confused for one another.
        let headers = headers_with("cookie", "prism_device=dev.tok; prism_session=sess.tok");
        assert_eq!(session_token(&headers).as_deref(), Some("sess.tok"));
        assert_eq!(device_token(&headers).as_deref(), Some("dev.tok"));
    }

    #[test]
    fn a_device_cookie_alone_yields_no_session() {
        let headers = headers_with("cookie", "prism_device=dev.tok");
        assert_eq!(session_token(&headers), None);
        assert_eq!(device_token(&headers).as_deref(), Some("dev.tok"));
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
