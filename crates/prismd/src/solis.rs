//! **Solis, through Prism.** POLARIS's agent listens on its gateway
//! (`/api/realtime`, loopback, port 8788 unless POLARIS was told otherwise).
//! Prism passes a signed-in owner through to it, so a device that can't
//! install POLARIS can still talk to Solis in a browser. Nothing here speaks
//! the protocol: the panel does, and the frames pass untouched.
//!
//! No POLARIS on this PC, or its gateway off: `/api/solis` says so and the
//! panel says how to turn it on.

use crate::api::{AppState, require};
use axum::{Json, Router, extract::{Request, State}, http::{StatusCode, Uri}, response::{IntoResponse, Response}, routing::get};
use prism_core::auth::Sensitivity;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/solis", get(here))
        .route("/api/solis/realtime", get(realtime))
}

/// Where POLARIS's gateway listens: `PRISM_SOLIS_PORT`, else POLARIS's default.
fn port() -> u16 {
    std::env::var("PRISM_SOLIS_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8788)
}

async fn here(State(state): State<AppState>, req: Request) -> Response {
    if let Some(r) = require(&state, req.headers(), Sensitivity::Session) { return r }
    let up = tokio::time::timeout(std::time::Duration::from_millis(600), tokio::net::TcpStream::connect(("127.0.0.1", port()))).await.is_ok_and(|r| r.is_ok());
    Json(serde_json::json!({ "up": up, "port": port() })).into_response()
}

async fn realtime(State(state): State<AppState>, req: Request) -> Response {
    if let Some(r) = require(&state, req.headers(), Sensitivity::Session) { return r }
    let target: Uri = match format!("http://127.0.0.1:{}/api/realtime", port()).parse() {
        Ok(u) => u,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    crate::proxy::upgrade(req, target, port()).await
}
