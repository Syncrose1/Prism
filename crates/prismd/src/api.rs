//! The tailnet HTTP API.
//!
//! Every route declares its [`Sensitivity`] in one table. Authorisation is
//! applied from that table rather than inside each handler, so adding an
//! endpoint cannot accidentally default to open — the failure mode of
//! per-handler checks is a missing line, and a missing line here is a
//! compile-time gap in a match rather than a silently public route.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use prism_core::auth::{AuthOutcome, Authenticator, CodeOutcome, LoginPrompt, Sensitivity, totp};
use prism_core::config::Facet;
use prism_core::governor::Tier;
use prism_core::sensors::disk::MountUsage;
use prism_core::sensors::memory::MemorySnapshot;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tracing::{error, info, warn};

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
    /// The card, if there is one. Reported split by owner rather than as a
    /// single used figure, because on this host the two halves mean opposite
    /// things: see `prism_core::sensors::gpu`.
    pub vram: Option<VramVitals>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VramVitals {
    pub total_mib: u64,
    /// Held by governed workloads — reclaimable on demand.
    pub ours_mib: u64,
    /// Held by everything else — the signal that actually raises the tier.
    pub foreign_mib: u64,
    /// What governed workloads may hold right now.
    pub budget_mib: u64,
    /// How much they must give back. Zero when within budget.
    pub overdraft_mib: u64,
    /// RAM that would be demanded if every offload-capable facet spilled its
    /// VRAM to host memory. The number that connects a full card to a memory
    /// incident — see `governor::Reading::spill_liability_mib`.
    pub spill_liability_mib: u64,
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
        vram: Option<VramVitals>,
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
            vram,
        }
    }
}

pub type SharedVitals = Arc<RwLock<Vitals>>;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<Authenticator>,
    /// Signing in from the machine Prism runs on, without reaching for a phone
    /// that is guarding a secret already sitting on this disk. See
    /// `prism_core::auth::console`.
    pub console_key: Arc<String>,
    pub grants: Arc<prism_core::auth::console::Grants>,
    /// **The bridge** (POLARIS on this machine): a 0600 key in the state
    /// directory, sent as `Authorization: Bridge <key>`, from loopback only.
    /// Reading it proves what reading `totp.secret` would (the owning user on
    /// this host), so it authorises like a session; every change made with it
    /// is in the access log under `polaris`, for the account named in
    /// `X-Prism-For`.
    pub bridge_key: Arc<String>,
    /// Who signed in, from where, and what they changed: sealed
    /// (`prism_core::access`).
    pub access: Arc<prism_core::access::AccessLog>,
    /// Share links (`prism_core::links`), kept in `$STATE/links.json`.
    pub links: Arc<std::sync::Mutex<prism_core::links::Store>>,
    /// Other programs' accounts (`[accounts]` in prism.toml).
    pub accounts: Arc<prism_core::config::AccountsConfig>,
    /// The file roots open to guests (`guests = true`).
    pub guest_roots: Arc<std::collections::HashSet<String>>,
    /// The port Prism itself is serving on, so the discovery sweep does not
    /// offer the operator their own desktop as an app to add to it.
    pub port: u16,
    pub vitals: SharedVitals,
    pub facets: Arc<RwLock<Vec<Facet>>>,
    pub profile_path: Arc<std::path::PathBuf>,
    pub state_dir: Arc<std::path::PathBuf>,
    pub events: Arc<prism_core::events::EventLog>,
    /// Whether hardware encoding is available, decided once at startup.
    pub nvenc: bool,
    pub proxy: crate::proxy::ProxyClient,
    pub proxy_tls: crate::proxy::TlsProxyClient,
    /// Facets discovered to require TLS, so the wasted plain attempt happens
    /// once rather than on every request.
    pub tls_backends: Arc<RwLock<std::collections::HashSet<String>>>,
    pub terminals: Arc<prism_core::term::session::SessionManager>,
    pub roots: Arc<Vec<prism_core::files::path::Root>>,
    pub thumb_dir: Arc<std::path::PathBuf>,
}

pub fn router(state: AppState) -> Router {
    let audited = state.clone();
    Router::new()
        .route("/api/access", get(access))
        .route("/api/config/{name}", get(config_get).put(config_put))
        .route("/api/health", get(health))
        .route("/api/auth/login", post(login))
        .route("/api/auth/console", post(console_grant))
        .route("/auth/console", get(console_redeem))
        .route("/api/auth/prompt", get(login_prompt))
        .route("/api/auth/logout", post(logout))
        .route("/api/vitals", get(vitals))
        .merge(crate::discover::routes())
        .route("/api/events", get(events))
        // Critical Functions Mode. Merged rather than nested so it shares no
        // middleware with the API surface — see ADR 0002.
        .merge(crate::rescue::routes())
        .merge(crate::term_api::routes())
        .merge(crate::files_api::routes())
        .merge(crate::facets_api::routes())
        .merge(crate::links_api::routes())
        .merge(crate::accounts::routes())
        .merge(crate::workspace::routes())
        .merge(crate::proxy::routes())
        .route("/", get(crate::ui::index))
        .route("/classic", get(crate::ui::classic))
        .route("/ui/{*path}", get(crate::ui::asset))
        .layer(axum::middleware::from_fn_with_state(audited, audit))
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

/// The bridge key a request carries, if any.
pub(crate) fn bridge_token(headers: &HeaderMap) -> Option<String> {
    headers.get("authorization")?.to_str().ok()?.strip_prefix("Bridge ").map(|k| k.trim().to_string())
}

/// Whether a request carries the right bridge key (compared in constant time).
pub(crate) fn is_bridge(state: &AppState, headers: &HeaderMap) -> bool {
    use subtle::ConstantTimeEq;
    match bridge_token(headers) {
        Some(k) if !state.bridge_key.is_empty() => k.as_bytes().ct_eq(state.bridge_key.as_bytes()).into(),
        _ => false,
    }
}

/// Enforce a tier, returning the error response to send if it is not met.
pub(crate) fn require(state: &AppState, headers: &HeaderMap, need: Sensitivity) -> Option<Response> {
    if is_bridge(state, headers) { return None }
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

/// End the session on this device, keeping the device enrolment.
///
/// The two tokens exist precisely so this can be cheap. Clearing the session
/// means the next visit asks for the password; clearing the enrolment would
/// mean finding an authenticator, which is the friction the operator asked to
/// be rid of. So only the session cookie is dropped.
///
/// Nothing on the host is touched: workloads, terminals and their windows keep
/// running. Locking the screen you are looking at should not interrupt work on
/// a machine you are not.
///
/// Unauthenticated deliberately — refusing to log out someone whose session
/// has already expired would only strand them on a page they cannot leave.
async fn logout() -> Response {
    (
        StatusCode::OK,
        [(
            axum::http::header::SET_COOKIE,
            // Max-Age=0 with a matching path is what actually removes it;
            // an expiry in the past on a different path leaves it in place.
            "prism_session=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax",
        )],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

/// What the login screen should ask for. Public: it reveals only whether *this*
/// browser is already enrolled, which that browser necessarily knows.
#[derive(Deserialize)]
struct ConsoleKeyRequest {
    key: String,
}

#[derive(Serialize)]
struct ConsoleGrantResponse {
    grant: String,
}

/// Exchange the console key for a single-use grant.
///
/// Holding the key means being able to read a 0600 file in the state directory,
/// which is the same capability as reading `totp.secret` and minting codes
/// forever. This does not widen anything; it makes an already-open door
/// convenient instead of pretending it is shut.
async fn console_grant(
    State(state): State<AppState>,
    Json(body): Json<ConsoleKeyRequest>,
) -> Response {
    use prism_core::auth::console;
    if !console::key_matches(&state.console_key, &body.key) {
        // Deliberately identical to any other refusal, and deliberately NOT
        // counted toward the authenticator's lockout: a wrong console key must
        // not be able to lock the real operator out of their phone route.
        warn!("console key rejected");
        return err_json(StatusCode::UNAUTHORIZED, "bad_key", "console key not recognised");
    }
    match state.grants.mint(totp::now_unix()) {
        Ok(grant) => Json(ConsoleGrantResponse { grant }).into_response(),
        Err(e) => {
            error!(err = %e, "could not mint a console grant");
            err_json(StatusCode::INTERNAL_SERVER_ERROR, "no_grant", "could not mint a grant")
        }
    }
}

#[derive(Deserialize)]
struct ConsoleRedeem {
    grant: String,
}

/// Spend a grant and land in the shell, signed in.
///
/// A redirect rather than JSON, because the point is that a browser opened by
/// `prismd open` arrives already authenticated with no page in between. The
/// cookies are set here, server-side, so they keep `HttpOnly` — a token handed
/// to JavaScript to set for itself would not.
async fn console_redeem(
    State(state): State<AppState>,
    Query(q): Query<ConsoleRedeem>,
) -> Response {
    let now = totp::now_unix();
    if !state.grants.spend(&q.grant, now) {
        warn!("console grant refused (spent, expired, or never issued)");
        return err_json(
            StatusCode::UNAUTHORIZED,
            "bad_grant",
            "that grant is spent or expired — run `prismd open` again",
        );
    }
    let policy = *state.auth.policy();
    let (session, device) = state.auth.issue_console_session(now);
    info!("signed in from the console");

    let mut response = Redirect::to("/").into_response();
    let out = response.headers_mut();
    if let Ok(v) = session_cookie(&session, policy.session_ttl_secs).parse() {
        out.append(axum::http::header::SET_COOKIE, v);
    }
    if let Ok(v) = device_cookie(&device, policy.device_ttl_secs).parse() {
        out.append(axum::http::header::SET_COOKIE, v);
    }
    response
}

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

pub(crate) fn err_json(status: StatusCode, error: &'static str, detail: impl Into<String>) -> Response {
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

// ---------------------------------------------------------------------------
// The access log, and the bridge's audit
// ---------------------------------------------------------------------------

/// Every sign-in, refusal and change, written to the sealed access log on
/// its way out, with the peer it came from. One layer rather than a line in
/// each handler, so a new route that changes something is logged without
/// anyone remembering to. The bridge key is refused from anywhere but
/// loopback: the key is this machine's, and never needs to cross a network.
async fn audit(
    State(state): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use prism_core::access::{Access, Kind};
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let headers = req.headers().clone();
    let bridged = bridge_token(&headers).is_some();
    let for_account = headers.get("x-prism-for").and_then(|v| v.to_str().ok()).map(|v| v.chars().filter(|c| c.is_alphanumeric() || "-_. ".contains(*c)).take(64).collect::<String>());
    let from = peer.ip().to_string();
    let log = |kind: Kind, who: &str, how: &str, what: String| {
        let a = Access { who: who.into(), for_account: for_account.clone(), how: how.into(), from: from.clone(), what };
        if let Err(e) = state.access.append(kind, a, totp::now_unix()) { error!(error = %e, "access log write failed"); }
    };
    if bridged && !peer.ip().is_loopback() {
        log(Kind::Refused, "unknown", "bridge", format!("{method} {path}: the bridge key, from off this machine"));
        return err_json(StatusCode::FORBIDDEN, "bridge_is_local", "the bridge key is accepted from this machine only");
    }
    // A guest's session reaches only what a guest may: looking at files.
    if !bridged && state.auth.session_kind(session_token(&headers).as_deref(), totp::now_unix()) == Some(prism_core::auth::session::TokenKind::Guest)
        && !path.starts_with("/l/") && path != "/rescue" {
        // And only in the folders opened to guests.
        let root = req.uri().query().unwrap_or("").split('&').find_map(|p| p.strip_prefix("root=")).map(|r| r.replace("%20", " "));
        let in_guest_root = path == "/api/files/roots" || !path.starts_with("/api/files/") || root.is_some_and(|r| state.guest_roots.contains(&r));
        if !crate::accounts::guest_may(&method, &path) || !in_guest_root {
            return err_json(StatusCode::FORBIDDEN, "guest", "a guest can look at the folders opened to guests, and nothing more");
        }
    }
    // A sign-in's body says how (a code or a password); it is small, so it
    // is read here and handed on.
    let (req, how) = if path == "/api/auth/login" {
        let (parts, body) = req.into_parts();
        let bytes = axum::body::to_bytes(body, 64 * 1024).await.unwrap_or_default();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let how = if v["code"].as_str().is_some_and(|c| !c.trim().is_empty()) { "code" } else { "password" };
        (axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes)), how)
    } else { (req, "") };
    let ok_bridge = bridged && is_bridge(&state, &headers);
    let res = next.run(req).await;
    let status = res.status();
    let good = status.is_success() || status.is_redirection();
    match (path.as_str(), &method) {
        ("/api/auth/login", _) => log(if good { Kind::SignedIn } else { Kind::Refused }, if good { "admin" } else { "unknown" }, how, format!("sign-in: {}", status.as_u16())),
        ("/auth/console", _) => log(if good { Kind::SignedIn } else { Kind::Refused }, if good { "admin" } else { "unknown" }, "console", "signed in from this machine".into()),
        ("/api/auth/logout", _) => log(Kind::SignedOut, "admin", "session", "signed out".into()),
        ("/api/auth/console", _) => {}
        // Signs itself in the log, with the account's own name (accounts.rs).
        ("/api/auth/account", _) => {}
        (p, m) if *m != axum::http::Method::GET && *m != axum::http::Method::HEAD && p.starts_with("/api/") => {
            let who = if ok_bridge { "polaris" } else { "admin" };
            let how = if ok_bridge { "bridge" } else { "session" };
            if good { log(Kind::Changed, who, how, format!("{method} {path}")) }
            else if status == StatusCode::UNAUTHORIZED { log(Kind::Refused, "unknown", if bridged { "bridge" } else { "session" }, format!("{method} {path}")) }
        }
        _ if bridged && !ok_bridge && status == StatusCode::UNAUTHORIZED => log(Kind::Refused, "unknown", "bridge", format!("{method} {path}: a wrong bridge key")),
        _ => {}
    }
    res
}

#[derive(Deserialize)]
struct AccessQuery {
    who: Option<String>,
    limit: Option<usize>,
}

/// The access log, newest first, with whether its chain is intact.
async fn access(State(state): State<AppState>, headers: HeaderMap, Query(q): Query<AccessQuery>) -> Response {
    if let Some(r) = require(&state, &headers, Sensitivity::Session) { return r }
    let entries = state.access.recent(q.limit.unwrap_or(200).min(2000), q.who.as_deref());
    Json(serde_json::json!({ "verified": state.access.verify(), "entries": entries })).into_response()
}

/// The two config files POLARIS (or the operator) may read and change.
fn config_path(state: &AppState, name: &str) -> Option<std::path::PathBuf> {
    let dir = state.profile_path.parent()?.to_path_buf();
    match name { "prism" => Some(dir.join("prism.toml")), "profile" => Some((*state.profile_path).clone()), _ => None }
}

async fn config_get(State(state): State<AppState>, headers: HeaderMap, axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    if let Some(r) = require(&state, &headers, Sensitivity::Session) { return r }
    let Some(path) = config_path(&state, &name) else { return err_json(StatusCode::NOT_FOUND, "no_such_config", "prism or profile") };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    ([(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
}

/// Replace a config file, only if it parses as what it is: a file Prism
/// would refuse never reaches the disk. The one it replaces becomes the last
/// good copy. Facets apply at once; the governor and the server read theirs
/// at the next start.
async fn config_put(State(state): State<AppState>, headers: HeaderMap, axum::extract::Path(name): axum::extract::Path<String>, body: String) -> Response {
    if let Some(r) = require(&state, &headers, Sensitivity::Session) { return r }
    let Some(path) = config_path(&state, &name) else { return err_json(StatusCode::NOT_FOUND, "no_such_config", "prism or profile") };
    let parsed: Result<Option<Vec<Facet>>, String> = match name.as_str() {
        "prism" => toml::from_str::<prism_core::config::HostConfig>(&body).map(|_| None).map_err(|e| e.to_string()),
        _ => toml::from_str::<prism_core::config::Profile>(&body).map(|p| Some(p.facet)).map_err(|e| e.to_string()),
    };
    let facets = match parsed { Ok(f) => f, Err(e) => return err_json(StatusCode::UNPROCESSABLE_ENTITY, "does_not_parse", e) };
    let good = path.with_extension("toml.last-good");
    if path.is_file() { let _ = std::fs::copy(&path, &good); }
    let tmp = path.with_extension("toml.new");
    if let Err(e) = std::fs::write(&tmp, &body).and_then(|_| std::fs::rename(&tmp, &path)) {
        return err_json(StatusCode::INTERNAL_SERVER_ERROR, "write_failed", e.to_string());
    }
    if let Some(f) = facets && let Ok(mut live) = state.facets.write() { *live = f; }
    state.events.push(prism_core::events::Level::Action, "prism", format!("{name}.toml changed"));
    Json(serde_json::json!({ "ok": true, "applies": if name == "profile" { "facets now; the governor at the next start" } else { "at the next start" } })).into_response()
}
