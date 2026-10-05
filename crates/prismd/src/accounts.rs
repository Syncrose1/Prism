//! **Other accounts** (`prism_core::config::AccountsConfig`): the people of
//! another program on this PC sign in to Prism with their own password. Prism
//! runs the provider command with one JSON line and reads one back; it names
//! no program. POLARIS's accounts arrive this way (`polarisd account
//! provider`), and so could anyone's.
//!
//! The provider's primary account signs in as the owner; everyone else as a
//! guest, held by [guest_may] to looking at files, unless the host config
//! names them an owner. Wrong passwords count with every other wrong guess,
//! so guessing an account locks out like guessing a code.

use crate::api::{AppState, err_json, session_token};
use axum::{Json, Router, extract::{ConnectInfo, State}, http::{HeaderMap, Method, StatusCode, header}, response::{IntoResponse, Response}, routing::{get, post}};
use prism_core::access::{Access, Kind};
use prism_core::auth::session::TokenKind;
use prism_core::auth::totp;
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/people", get(people))
        .route("/api/auth/account", post(sign_in))
        .route("/api/auth/me", get(me))
}

const WHO_COOKIE: &str = "prism_who";

/// Ask the provider. Off the async threads, with a deadline: a provider that
/// hangs must never hang sign-in for everyone.
async fn ask(state: &AppState, req: Value) -> Option<Value> {
    let cfg = state.accounts.clone();
    let (program, args) = cfg.provider.split_first()?;
    let (program, args) = (program.clone(), args.to_vec());
    let env = cfg.env.clone();
    let run = tokio::task::spawn_blocking(move || -> Option<Value> {
        use std::io::Write;
        let mut child = std::process::Command::new(program).args(args).envs(env)
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
            .spawn().ok()?;
        child.stdin.take()?.write_all(format!("{req}\n").as_bytes()).ok()?;
        let out = child.wait_with_output().ok()?;
        serde_json::from_slice(String::from_utf8_lossy(&out.stdout).lines().last()?.as_bytes()).ok()
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), run).await.ok()?.ok()?
}

/// Who's here: names and colours, for the sign-in screen. Only when a
/// provider is set; never a password, never more than a name.
async fn people(State(state): State<AppState>) -> Response {
    if state.accounts.provider.is_empty() { return Json(json!({ "people": [] })).into_response() }
    match ask(&state, json!({ "op": "people" })).await {
        Some(v) => Json(json!({ "people": v["people"].as_array().cloned().unwrap_or_default().into_iter().map(|p| json!({
            "id": p["id"], "name": p["name"], "colours": p["colours"], "owner": is_owner(&state, &p),
        })).collect::<Vec<_>>() })).into_response(),
        None => Json(json!({ "people": [], "trouble": "the account provider didn't answer" })).into_response(),
    }
}

fn is_owner(state: &AppState, p: &Value) -> bool {
    p["primary"].as_bool().unwrap_or(false)
        || state.accounts.owners.iter().any(|o| Some(o.as_str()) == p["name"].as_str() || Some(o.as_str()) == p["id"].as_str())
}

#[derive(Deserialize)]
struct SignIn { who: String, password: String }

async fn sign_in(State(state): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Json(b): Json<SignIn>) -> Response {
    let now = totp::now_unix();
    if state.accounts.provider.is_empty() { return err_json(StatusCode::NOT_FOUND, "no_accounts", "this PRISM signs in with its own code only") }
    if let Some(secs) = state.auth.locked(now) { return err_json(StatusCode::TOO_MANY_REQUESTS, "locked_out", &format!("too many wrong tries; try again in {secs}s")) }
    let log = |kind: Kind, who: &str, what: &str| {
        let _ = state.access.append(kind, Access { who: who.into(), for_account: None, how: "account".into(), from: peer.ip().to_string(), what: what.into() }, now);
    };
    let answer = ask(&state, json!({ "op": "verify", "who": b.who, "password": b.password })).await;
    match answer {
        Some(v) if v["ok"].as_bool() == Some(true) => {
            let owner = is_owner(&state, &v);
            let name = v["name"].as_str().unwrap_or("someone").to_string();
            let token = state.auth.account_session(owner, now);
            log(Kind::SignedIn, &name, if owner { "signed in, as the owner" } else { "signed in, as a guest" });
            let ttl = state.auth.policy().session_ttl_secs;
            let mut r = Json(json!({ "ok": true, "name": name, "role": if owner { "owner" } else { "guest" } })).into_response();
            let h = r.headers_mut();
            if let Ok(c) = format!("prism_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ttl}").parse() { h.append(header::SET_COOKIE, c); }
            // For showing the name only: authority is the signed session, never this.
            let safe: String = name.chars().filter(|c| c.is_alphanumeric() || " -_.".contains(*c)).take(40).collect();
            if let Ok(c) = format!("{WHO_COOKIE}={}; Path=/; SameSite=Strict; Max-Age={ttl}", safe.replace(' ', "%20")).parse() { h.append(header::SET_COOKIE, c); }
            r
        }
        Some(_) => {
            log(Kind::Refused, "unknown", "a wrong account password");
            match state.auth.account_refused(now) {
                Some(secs) => err_json(StatusCode::TOO_MANY_REQUESTS, "locked_out", &format!("too many wrong tries; try again in {secs}s")),
                None => err_json(StatusCode::UNAUTHORIZED, "wrong_password", "that password doesn't open this account"),
            }
        }
        None => err_json(StatusCode::SERVICE_UNAVAILABLE, "provider_down", "the account provider didn't answer"),
    }
}

/// Who this session is: owner or guest, and the name to show.
async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let kind = state.auth.session_kind(session_token(&headers).as_deref(), totp::now_unix());
    let name = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).and_then(|c| c.split(';').find_map(|p| {
        let (k, v) = p.split_once('=')?; (k.trim() == WHO_COOKIE).then(|| v.trim().replace("%20", " "))
    }));
    match kind {
        Some(TokenKind::Guest) => Json(json!({ "role": "guest", "name": name })).into_response(),
        Some(_) => Json(json!({ "role": "owner", "name": name })).into_response(),
        None => err_json(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first"),
    }
}

/// What a guest may do: look at files (and their pictures and films), and
/// keep their place. Nothing that runs, changes or reveals the PC.
pub fn guest_may(method: &Method, path: &str) -> bool {
    let read = *method == Method::GET || *method == Method::HEAD;
    match path {
        "/" | "/api/health" | "/api/system" | "/api/auth/me" | "/api/auth/people" | "/api/auth/prompt" | "/api/vitals" => read,
        "/api/auth/logout" => true,
        p if p.starts_with("/ui/") => read,
        p if ["/api/files/roots", "/api/files/list", "/api/files/raw", "/api/files/thumb", "/api/files/preview", "/api/files/zip", "/api/files/media", "/api/files/stream"].contains(&p) => read,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guest_looks_at_files_and_does_nothing_else() {
        let get = Method::GET;
        assert!(guest_may(&get, "/api/files/list") && guest_may(&get, "/api/files/raw") && guest_may(&get, "/ui/os.js"));
        assert!(!guest_may(&Method::POST, "/api/files/upload"));
        assert!(!guest_may(&Method::POST, "/api/files/delete"));
        for p in ["/api/term", "/api/facets", "/api/links", "/api/access", "/api/events", "/api/config/prism", "/api/workspace", "/facet/comfyui/", "/api/discover"] {
            assert!(!guest_may(&get, p), "{p}");
        }
        assert!(guest_may(&Method::POST, "/api/auth/logout"));
    }
}
