//! Terminal HTTP and WebSocket surface.
//!
//! Everything here needs an unlocked session. A shell is unrestricted access
//! to the machine, so it sits at the same tier as files rather than a lower one.
//! See ADR 0003 §4 and `auth` for why the timed tier was removed.
//!
//! The WebSocket carries raw bytes in both directions — terminal traffic is
//! escape sequences, not text, and any framing or transcoding on the way through
//! would corrupt cursor addressing and colour.

use axum::{
    Json, Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use prism_core::auth::Sensitivity;
use prism_core::governor::Tier;
use prism_core::term::pty::WinSize;
use prism_core::term::session::{SpawnError, TermConfig};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::api::{AppState, require};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/term", get(list).post(create))
        .route("/api/term/{id}/attach", get(attach))
        .route("/api/term/{id}/resize", post(resize))
        .route("/api/term/{id}/kill", post(kill))
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    detail: String,
}

fn err(status: StatusCode, error: &'static str, detail: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorBody {
            error,
            detail: detail.into(),
        }),
    )
        .into_response()
}

/// Terminals are refused above Amber.
///
/// Opening a shell on a machine that is already failing adds load at the worst
/// possible moment. Existing sessions stay attached — the operator may well be
/// using one to fix the problem — and `/rescue` remains available regardless.
fn tier_allows_new_session(state: &AppState) -> bool {
    let tier = state.vitals.read().expect("vitals poisoned").tier.clone();
    !matches!(tier.as_str(), "red" | "black")
}

fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if !state.terminals.config().enabled {
        return Some(err(
            StatusCode::FORBIDDEN,
            "terminal_disabled",
            "terminal sessions are disabled on this host",
        ));
    }
    require(state, headers, Sensitivity::Session)
}

// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ListResponse {
    sessions: Vec<prism_core::term::session::SessionInfo>,
    max_sessions: usize,
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(denied) = guard(&state, &headers) {
        return denied;
    }
    state.terminals.reap();
    Json(ListResponse {
        sessions: state.terminals.list(),
        max_sessions: state.terminals.config().max_sessions,
    })
    .into_response()
}

#[derive(Deserialize, Default)]
struct CreateRequest {
    /// Empty runs the configured login shell.
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    rows: Option<u16>,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    title: Option<String>,
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<CreateRequest>>,
) -> Response {
    if let Some(denied) = guard(&state, &headers) {
        return denied;
    }
    if !tier_allows_new_session(&state) {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "degraded",
            "the machine is under pressure; new terminals are refused. \
             Use /rescue to recover.",
        );
    }

    let req = body.map(|Json(b)| b).unwrap_or_default();
    let size = WinSize {
        rows: req.rows.unwrap_or(24).clamp(1, 500),
        cols: req.cols.unwrap_or(80).clamp(1, 1000),
    };
    let title = req.title.unwrap_or_else(|| "Terminal".into());

    match state
        .terminals
        .create(&req.command, req.cwd.as_deref(), size, &title)
    {
        Ok(session) => {
            info!(id = %session.id, pid = session.pid(), "terminal session created");
            Json(session.info()).into_response()
        }
        Err(SpawnError::Disabled) => err(
            StatusCode::FORBIDDEN,
            "terminal_disabled",
            "terminal sessions are disabled",
        ),
        Err(SpawnError::TooMany) => err(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_sessions",
            "close or kill an existing session first",
        ),
        Err(SpawnError::Io(e)) => {
            warn!(error = %e, "terminal spawn failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "spawn_failed", e.to_string())
        }
    }
}

#[derive(Deserialize)]
struct ResizeRequest {
    rows: u16,
    cols: u16,
}

async fn resize(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ResizeRequest>,
) -> Response {
    if let Some(denied) = guard(&state, &headers) {
        return denied;
    }
    let Some(session) = state.terminals.get(&id) else {
        return err(StatusCode::NOT_FOUND, "no_session", "no such session");
    };
    // Without this a resized browser window leaves full-screen programs drawing
    // to the old geometry — the classic "vim thinks the screen is 80 columns".
    match session.resize(WinSize {
        rows: body.rows.clamp(1, 500),
        cols: body.cols.clamp(1, 1000),
    }) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "resize_failed", e.to_string()),
    }
}

async fn kill(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = guard(&state, &headers) {
        return denied;
    }
    // The destructive operation, deliberately distinct from closing a window.
    if state.terminals.kill(&id) {
        info!(%id, "terminal session killed");
        StatusCode::NO_CONTENT.into_response()
    } else {
        err(StatusCode::NOT_FOUND, "no_session", "no such session")
    }
}

// ---------------------------------------------------------------------------
// Attach
// ---------------------------------------------------------------------------

async fn attach(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // Authorised before the upgrade: a socket must never be established for a
    // caller who could not have reached the endpoint over plain HTTP.
    if let Some(denied) = guard(&state, &headers) {
        return denied;
    }
    let Some(session) = state.terminals.get(&id) else {
        return err(StatusCode::NOT_FOUND, "no_session", "no such session");
    };
    ws.on_upgrade(move |socket| pump(socket, session))
}

/// How often to ping an idle socket.
///
/// A terminal nobody is typing into sends nothing at all, and an idle
/// WebSocket gets dropped by browsers, proxies and NAT tables somewhere around
/// 30-60 seconds. That is the "it detached on its own" the operator saw. A ping
/// well inside that window keeps the connection accounted for as live.
const PING_EVERY: Duration = Duration::from_secs(20);

/// Bridge a WebSocket to a session for as long as both live.
///
/// Dropping out of this function is a *detach*, not a kill. The session, its
/// scrollback and everything it launched carry on without a listener — which is
/// what makes the client's automatic reconnect safe.
async fn pump(socket: WebSocket, session: Arc<prism_core::term::session::Session>) {
    use futures_util::{SinkExt, StreamExt};

    let (mut sink, mut stream) = socket.split();
    let (history, mut rx) = session.attach();

    // Replay scrollback first, so a reattached window shows what happened while
    // it was away rather than an empty screen.
    if !history.is_empty() && sink.send(Message::Binary(history.into())).await.is_err() {
        return;
    }

    let id = session.id.clone();
    let out = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await; // the first tick is immediate
        loop {
            tokio::select! {
                received = rx.recv() => match received {
                    Ok(chunk) => {
                        if sink
                            .send(Message::Binary(chunk.as_slice().to_vec().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    // Lagged means this client could not keep up and the channel
                    // dropped chunks for it. Continuing with a gap is better than
                    // disconnecting the operator mid-session.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(%id, dropped = n, "terminal client lagged");
                    }
                    Err(_) => break,
                },
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Keystrokes travel as raw bytes; text frames are accepted so a trivial
    // client can send without constructing binary frames.
    while let Some(message) = stream.next().await {
        let bytes = match message {
            Ok(Message::Binary(b)) => b.to_vec(),
            Ok(Message::Text(t)) => t.as_bytes().to_vec(),
            Ok(Message::Close(_)) => break,
            // Pong and Ping are keepalive traffic, not input.
            Ok(_) => continue,
            // A read error ends this attachment, but the session is untouched
            // and the client will reconnect.
            Err(_) => break,
        };
        if session.write(&bytes).is_err() {
            break;
        }
    }

    out.abort();
}

/// Build a session manager from host configuration.
pub fn manager_from(cfg: &prism_core::config::TerminalConfig) -> TermConfig {
    TermConfig {
        enabled: cfg.enabled,
        shell: cfg
            .shell
            .clone()
            .unwrap_or_else(prism_core::term::session::login_shell),
        scrollback_bytes: cfg.scrollback_bytes,
        max_sessions: cfg.max_sessions,
        use_scope: cfg.use_scope,
    }
}

/// Tier-driven de-escalation of terminal resources.
pub fn apply_tier(mgr: &prism_core::term::session::SessionManager, tier: Tier) {
    let cap = match tier {
        Tier::Green | Tier::Amber => mgr.config().scrollback_bytes,
        // Retaining a quarter of a megabyte per session is a luxury a failing
        // machine cannot afford.
        Tier::Red => 32 * 1024,
        Tier::Black => 4 * 1024,
    };
    mgr.shrink_all_scrollback(cap);
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::config::TerminalConfig;

    #[test]
    fn config_maps_across() {
        let cfg = TerminalConfig {
            enabled: true,
            shell: Some("/bin/zsh".into()),
            scrollback_bytes: 4096,
            max_sessions: 3,
            use_scope: false,
        };
        let t = manager_from(&cfg);
        assert_eq!(t.shell, "/bin/zsh");
        assert_eq!(t.scrollback_bytes, 4096);
        assert_eq!(t.max_sessions, 3);
        assert!(!t.use_scope);
    }

    #[test]
    fn shell_falls_back_when_unset() {
        let cfg = TerminalConfig {
            shell: None,
            ..Default::default()
        };
        assert!(!manager_from(&cfg).shell.is_empty());
    }

    #[test]
    fn degraded_tiers_shrink_scrollback() {
        let mgr = prism_core::term::session::SessionManager::new(TermConfig {
            scrollback_bytes: 256 * 1024,
            use_scope: false,
            ..Default::default()
        });
        // No sessions: the call must still be safe.
        apply_tier(&mgr, Tier::Black);
        apply_tier(&mgr, Tier::Green);
    }
}
