//! **A port of its own for each hosted app.** Served under `/facet/<id>/`, an
//! app that writes absolute paths (ComfyUI: `/api/...`, its websocket at
//! `/ws`) breaks, and no amount of HTML rewriting fixes JavaScript. So each
//! exposed app also gets its own Prism port, where it is served at the root
//! and nothing is rewritten.
//!
//! * Opened on demand (`GET /api/facets/{id}/port`), on the addresses Prism
//!   itself listens on (the overlay and loopback), never wider.
//! * Behind the same session: a browser sends a host's cookies whatever the
//!   port, so signing in once covers every app's port.
//! * Stable: each app keeps the port it was given (`$STATE/ports.json`),
//!   counted up from just above Prism's own.

use crate::api::{AppState, require};
use axum::{Json, Router, extract::{Path, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, routing::get};
use prism_core::auth::Sensitivity;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// What the ports module keeps: where Prism listens, and which app has
/// which port (open or not yet).
#[derive(Default)]
pub struct Ports {
    pub ips: Vec<IpAddr>,
    pub base: u16,
    pub given: BTreeMap<String, u16>,
    pub open: std::collections::HashSet<String>,
}

pub type Shared = Arc<Mutex<Ports>>;

fn file(state: &AppState) -> std::path::PathBuf { state.state_dir.join("ports.json") }

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/facets/{id}/port", get(port))
}

async fn port(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = require(&state, &headers, Sensitivity::Session) { return r }
    let exposed = state.facets.read().expect("facets poisoned").iter().any(|f| f.id == id && f.expose.is_some());
    if !exposed { return (StatusCode::NOT_FOUND, "that app shows no page").into_response() }
    match open(&state, &id).await {
        Ok(p) => Json(serde_json::json!({ "port": p })).into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, e).into_response(),
    }
}

/// The app's port, listening; opened now if it wasn't.
async fn open(state: &AppState, id: &str) -> Result<u16, String> {
    let (ips, want, already) = {
        let mut p = state.ports.lock().map_err(|_| "ports poisoned")?;
        if p.given.is_empty() {
            p.given = std::fs::read_to_string(file(state)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        }
        (p.ips.clone(), p.given.get(id).copied(), p.open.contains(id))
    };
    if already { return want.ok_or_else(|| "no port".to_string()) }
    // The port it had, else the next free one above Prism's own.
    let base = state.ports.lock().map_err(|_| "ports poisoned")?.base;
    let taken: Vec<u16> = state.ports.lock().map_err(|_| "ports poisoned")?.given.values().copied().collect();
    let mut candidates: Vec<u16> = want.into_iter().collect();
    candidates.extend((base + 1..base + 200).filter(|p| !taken.contains(p)));
    for port in candidates {
        let mut listeners = Vec::new();
        let mut ok = true;
        for ip in &ips {
            match tokio::net::TcpListener::bind(SocketAddr::new(*ip, port)).await {
                Ok(l) => listeners.push(l),
                Err(_) => { ok = false; break }
            }
        }
        if !ok || listeners.is_empty() { continue }
        let app = Router::new()
            .fallback(crate::proxy::at_root)
            .layer(axum::Extension(crate::proxy::OwnPort(id.to_string())))
            .with_state(state.clone());
        for l in listeners {
            let app = app.clone();
            info!(facet = %id, port, "hosted app at its own port");
            tokio::spawn(async move {
                if let Err(e) = axum::serve(l, app.into_make_service_with_connect_info::<SocketAddr>()).await { warn!(error = %e, "app port ended") }
            });
        }
        let mut p = state.ports.lock().map_err(|_| "ports poisoned")?;
        p.given.insert(id.to_string(), port);
        p.open.insert(id.to_string());
        let _ = std::fs::write(file(state), serde_json::to_vec_pretty(&p.given).unwrap_or_default());
        return Ok(port);
    }
    Err("no free port for it".into())
}
