//! Workspace persistence.
//!
//! *Operator requirement: "there is no state persistence of Prism OS. The state
//! should be saved on the Prism OS PC itself, so that the same cloud PC is
//! experienced across any and all devices when the target is the same target."*
//!
//! So layout lives on the host, not in the browser. `localStorage` would give
//! each device its own private desktop, which is the opposite of a cloud PC —
//! open a window on the laptop and it should be there on the tablet.
//!
//! The state is deliberately opaque to the server: it stores and returns a JSON
//! blob the shell defines. The alternative — typing every window and app here —
//! would mean a schema migration in Rust every time the UI gains a field, for no
//! benefit, since nothing server-side ever inspects it.
//!
//! Writes are atomic (temp file, then rename) because a half-written layout that
//! fails to parse would leave the operator with a desktop that will not load,
//! and the recovery for that is worse than losing a window position.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use prism_core::auth::Sensitivity;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::api::{AppState, require};

/// A ceiling on stored state. Layout is small; anything approaching this is a
/// bug or an attempt to use Prism as a database, and neither should be able to
/// fill the operator's disk — which is already at 94%.
const MAX_BYTES: usize = 256 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/workspace", get(load).put(save))
}

pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join("workspace.json")
}

async fn load(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Layout is not sensitive, but it does describe the machine's contents, so
    // it sits behind the same session as vitals.
    if let Some(denied) = require(&state, &headers, Sensitivity::Session) {
        return denied;
    }
    match tokio::fs::read_to_string(path(&state.state_dir)).await {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => Json(value).into_response(),
            Err(e) => {
                // A corrupt file must not wedge the desktop: report an empty
                // workspace and let the next save overwrite it.
                warn!(error = %e, "workspace state is not valid JSON; ignoring");
                Json(Value::Null).into_response()
            }
        },
        // No file yet is a first run, not an error.
        Err(_) => Json(Value::Null).into_response(),
    }
}

async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Some(denied) = require(&state, &headers, Sensitivity::Session) {
        return denied;
    }
    if body.len() > MAX_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("workspace state exceeds {MAX_BYTES} bytes"),
        )
            .into_response();
    }
    // Validate before writing. Storing something unparseable would only fail
    // later, on load, when it is least convenient.
    if serde_json::from_str::<Value>(&body).is_err() {
        return (StatusCode::BAD_REQUEST, "not valid JSON").into_response();
    }

    match write_atomic(&path(&state.state_dir), body.as_bytes()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            warn!(error = %e, "could not persist workspace");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

/// Write via a temporary file and rename, so a reader never sees a partial file.
async fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = target.with_extension("json.tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "prism-ws-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn state_lives_beside_the_other_host_state() {
        assert!(path(Path::new("/var/lib/prism")).ends_with("workspace.json"));
    }

    #[tokio::test]
    async fn writes_are_atomic_and_leave_no_temp_file() {
        let d = tmpdir("atomic");
        let target = d.join("workspace.json");
        write_atomic(&target, br#"{"windows":[]}"#).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            r#"{"windows":[]}"#
        );
        assert!(
            !d.join("workspace.json.tmp").exists(),
            "the temp file must be renamed, not left behind"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_later_write_replaces_the_earlier_one_wholesale() {
        let d = tmpdir("replace");
        let target = d.join("workspace.json");
        write_atomic(&target, br#"{"a":1}"#).await.unwrap();
        write_atomic(&target, br#"{"b":2}"#).await.unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), r#"{"b":2}"#);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn creates_the_state_directory_if_it_is_missing() {
        let d = tmpdir("mkdir");
        let target = d.join("nested/deeper/workspace.json");
        write_atomic(&target, b"{}").await.unwrap();
        assert!(target.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_size_ceiling_is_generous_for_layout_but_not_a_database() {
        // Layout is a few kilobytes; this is room to spare without letting the
        // endpoint fill a disk that is already 94% full.
        assert!(MAX_BYTES >= 64 * 1024);
        assert!(MAX_BYTES <= 1024 * 1024);
    }
}
