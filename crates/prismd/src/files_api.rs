//! File browsing, download and preview.
//!
//! Every path reaching the filesystem has come back from
//! [`prism_core::files::path::resolve`], which canonicalises and then verifies
//! containment. Nothing here constructs a path any other way — that is the
//! security boundary, and it is one function rather than a convention.
//!
//! Thumbnails shell out to tools already installed on the host (`vips`,
//! `ffmpeg`, `pdftoppm`) rather than linking image codecs into the daemon. See
//! ADR 0001: Prism orchestrates a pipeline, it does not reimplement one.

use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use prism_core::auth::Sensitivity;
use prism_core::files::{list, path as fpath};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

use crate::api::{AppState, require};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/files/roots", get(roots))
        .route("/api/files/list", get(listing))
        .route("/api/files/raw", get(raw))
        .route("/api/files/thumb", get(thumb))
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

/// Files are [`Sensitivity::Fresh`]: reading the filesystem is the most
/// sensitive read Prism offers, and a long-lived session on a phone is not the
/// same assurance as proving possession of it now.
fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if state.roots.is_empty() {
        return Some(err(
            StatusCode::FORBIDDEN,
            "files_disabled",
            "no file roots are configured",
        ));
    }
    require(state, headers, Sensitivity::Fresh)
}

/// Resolve `(root, path)` from a query into a real, contained filesystem path.
fn resolve(state: &AppState, root: &str, rel: &str) -> Result<(fpath::Root, PathBuf), Response> {
    let root = fpath::find(&state.roots, root)
        .map_err(|e| err(StatusCode::NOT_FOUND, "no_root", e.public_message()))?;
    let full = fpath::resolve(root, rel).map_err(|e| {
        // Escapes and NotFound deliberately look identical to the caller —
        // distinguishing them would reveal what exists outside the root.
        if matches!(e, fpath::PathError::Escapes) {
            warn!(root = %root.name, path = %rel, "refused a path that escapes its root");
        }
        err(StatusCode::NOT_FOUND, "not_found", e.public_message())
    })?;
    Ok((root.clone(), full))
}

// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct RootInfo {
    name: String,
    writable: bool,
}

async fn roots(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(d) = guard(&state, &headers) {
        return d;
    }
    let roots: Vec<RootInfo> = state
        .roots
        .iter()
        .map(|r| RootInfo {
            name: r.name.clone(),
            writable: r.writable,
        })
        .collect();
    Json(roots).into_response()
}

#[derive(Deserialize)]
struct ListQuery {
    root: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    sort: Option<String>,
}

#[derive(Serialize)]
struct ListResponse {
    root: String,
    path: String,
    #[serde(flatten)]
    listing: list::Listing,
}

async fn listing(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    if let Some(d) = guard(&state, &headers) {
        return d;
    }
    let (root, full) = match resolve(&state, &q.root, &q.path) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !full.is_dir() {
        return err(StatusCode::BAD_REQUEST, "not_a_directory", "not a directory");
    }

    let sort = match q.sort.as_deref() {
        Some("size") => list::Sort::Size,
        Some("modified") => list::Sort::Modified,
        _ => list::Sort::Name,
    };
    // Capped so a client cannot ask for a 100k-entry page and force the server
    // to stat the lot — the N+1 avoidance from ADR 0001 only holds if the page
    // stays small.
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);

    match list::list(&full, q.offset.unwrap_or(0), limit, sort) {
        Ok(listing) => Json(ListResponse {
            root: root.name.clone(),
            path: fpath::relative_to(&root, &full),
            listing,
        })
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "read_failed", e.to_string()),
    }
}

#[derive(Deserialize)]
struct FileQuery {
    root: String,
    path: String,
    #[serde(default)]
    download: Option<bool>,
}

/// Serve a file's bytes.
///
/// `Content-Disposition: attachment` only when explicitly requested, so images
/// and video can render inline while a click on "download" still saves.
async fn raw(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FileQuery>,
) -> Response {
    if let Some(d) = guard(&state, &headers) {
        return d;
    }
    let (_root, full) = match resolve(&state, &q.root, &q.path) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if full.is_dir() {
        return err(StatusCode::BAD_REQUEST, "is_a_directory", "is a directory");
    }

    let name = full
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let mime = mime_for(&name);

    match tokio::fs::read(&full).await {
        Ok(bytes) => {
            let mut resp = Response::builder()
                .header(header::CONTENT_TYPE, mime)
                // Private: this is the operator's filesystem over a shared
                // proxy-free path, but caching it in an intermediary is still
                // not something to invite.
                .header(header::CACHE_CONTROL, "private, max-age=60");
            if q.download.unwrap_or(false) {
                resp = resp.header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", name.replace('"', "")),
                );
            }
            resp.body(Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => err(StatusCode::NOT_FOUND, "read_failed", e.to_string()),
    }
}

/// Generate (or serve a cached) thumbnail.
///
/// Suppressed at Red and above: spawning image and video decoders on a machine
/// that is already short of memory is precisely the wrong thing to do, and the
/// UI degrades to an icon.
async fn thumb(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FileQuery>,
) -> Response {
    if let Some(d) = guard(&state, &headers) {
        return d;
    }
    let tier = state.vitals.read().expect("vitals poisoned").tier.clone();
    if matches!(tier.as_str(), "red" | "black") {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "degraded",
            "thumbnails are suspended while the machine is under pressure",
        );
    }

    let (_root, full) = match resolve(&state, &q.root, &q.path) {
        Ok(v) => v,
        Err(r) => return r,
    };

    match render_thumb(&full, &state.thumb_dir).await {
        Some(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "private, max-age=86400")
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        None => err(StatusCode::NOT_FOUND, "no_thumbnail", "cannot preview this file"),
    }
}

/// Produce a thumbnail, caching by `(path, mtime, size)`.
///
/// Keyed on mtime and size so an edited file re-renders without any
/// invalidation logic, and identical requests are free after the first.
async fn render_thumb(src: &std::path::Path, cache_dir: &std::path::Path) -> Option<Vec<u8>> {
    let meta = tokio::fs::metadata(src).await.ok()?;
    if meta.is_dir() {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    // A hash of the identity, not the contents: cheap, and enough to key on.
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        src.hash(&mut h);
        mtime.hash(&mut h);
        meta.len().hash(&mut h);
        format!("{:016x}.jpg", h.finish())
    };
    let cached = cache_dir.join(&key);
    if let Ok(bytes) = tokio::fs::read(&cached).await {
        return Some(bytes);
    }
    tokio::fs::create_dir_all(cache_dir).await.ok()?;

    let name = src.file_name()?.to_string_lossy().to_lowercase();
    let kind = list::Kind::from_extension(&name);
    let src_s = src.to_string_lossy().to_string();
    let out_s = cached.to_string_lossy().to_string();

    let ok = match kind {
        list::Kind::Image => run(
            "vips",
            &["thumbnail", &src_s, &format!("{out_s}[Q=82]"), "512"],
        )
        .await,
        list::Kind::Video => {
            // Seek *before* decoding: `-ss` ahead of `-i` means ffmpeg jumps to
            // the keyframe rather than decoding from the start, which is the
            // difference between instant and unusable on a 200 MB file.
            run(
                "ffmpeg",
                &[
                    "-ss", "00:00:03", "-i", &src_s, "-frames:v", "1",
                    "-vf", "scale=512:-1", "-y", "-loglevel", "error", &out_s,
                ],
            )
            .await
        }
        list::Kind::Pdf => {
            let stem = out_s.trim_end_matches(".jpg").to_string();
            let ok = run(
                "pdftoppm",
                &["-jpeg", "-f", "1", "-l", "1", "-scale-to", "512", &src_s, &stem],
            )
            .await;
            // pdftoppm appends a page suffix; normalise it to the cache key.
            if ok {
                for suffix in ["-1.jpg", "-01.jpg", "-001.jpg"] {
                    let produced = format!("{stem}{suffix}");
                    if tokio::fs::rename(&produced, &out_s).await.is_ok() {
                        break;
                    }
                }
            }
            ok
        }
        _ => false,
    };

    if !ok {
        return None;
    }
    tokio::fs::read(&cached).await.ok()
}

async fn run(program: &str, args: &[&str]) -> bool {
    tokio::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

fn mime_for(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "md" | "txt" | "log" | "conf" | "toml" | "yaml" | "yml" | "csv" | "rs" | "py" | "js"
        | "ts" | "sh" | "c" | "h" | "cpp" | "go" | "lua" | "html" | "css" | "xml" => {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_types() {
        assert_eq!(mime_for("a.png"), "image/png");
        assert_eq!(mime_for("clip.mp4"), "video/mp4");
        assert_eq!(mime_for("doc.pdf"), "application/pdf");
        assert_eq!(mime_for("notes.md"), "text/plain; charset=utf-8");
        assert_eq!(mime_for("model.safetensors"), "application/octet-stream");
        assert_eq!(mime_for("noext"), "application/octet-stream");
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(mime_for("PHOTO.JPG"), "image/jpeg");
    }

    #[tokio::test]
    async fn thumbnail_of_a_directory_is_none() {
        assert!(render_thumb(std::path::Path::new("/tmp"), std::path::Path::new("/tmp")).await.is_none());
    }

    #[tokio::test]
    async fn thumbnail_of_an_unsupported_type_is_none() {
        let dir = std::env::temp_dir().join(format!("prism-thumb-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("notes.md");
        std::fs::write(&f, b"# hello").unwrap();
        assert!(render_thumb(&f, &dir).await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
