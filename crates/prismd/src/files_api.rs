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
    routing::{get, post},
};
use prism_core::auth::Sensitivity;
use prism_core::files::{list, path as fpath};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::AsyncReadExt as _;
use tracing::warn;

use crate::api::{AppState, require};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/files/roots", get(roots))
        .route("/api/files/list", get(listing))
        .route("/api/files/raw", get(raw))
        .route("/api/files/thumb", get(thumb))
        .route("/api/files/mkdir", post(mkdir))
        .route("/api/files/rename", post(rename))
        .route("/api/files/delete", post(delete))
        .route("/api/files/upload", post(upload))
}

/// Resolve a *writable* root, refusing when it is read-only.
///
/// Write access is per-root rather than global: the operator can expose their
/// whole home read-only and a scratch directory writable, and a mistake in one
/// cannot damage the other.
fn writable_root<'a>(state: &'a AppState, name: &str) -> Result<&'a fpath::Root, Response> {
    let root = fpath::find(&state.roots, name)
        .map_err(|e| err(StatusCode::NOT_FOUND, "no_root", e.public_message()))?;
    if !root.writable {
        return Err(err(
            StatusCode::FORBIDDEN,
            "read_only",
            format!("root `{name}` is read-only"),
        ));
    }
    Ok(root)
}

/// Resolve the *parent* of something that does not exist yet, then append one
/// validated component.
///
/// `resolve` canonicalises, which requires the path to exist — so creating a
/// file needs this two-step: confirm the directory is inside the root, then add
/// a name that cannot contain a separator or `..`.
fn resolve_new(
    state: &AppState,
    root_name: &str,
    parent: &str,
    name: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), Response> {
    let root = writable_root(state, root_name)?;
    let name = name.trim();
    if name.is_empty() || !fpath::is_safe_new_path(name) || name.contains('/') {
        return Err(err(StatusCode::BAD_REQUEST, "bad_name", "invalid name"));
    }
    let dir = fpath::resolve(root, parent)
        .map_err(|e| err(StatusCode::NOT_FOUND, "not_found", e.public_message()))?;
    if !dir.is_dir() {
        return Err(err(StatusCode::BAD_REQUEST, "not_a_directory", "not a directory"));
    }
    Ok((dir.join(name), dir))
}

#[derive(Deserialize)]
struct MkdirRequest {
    root: String,
    #[serde(default)]
    path: String,
    name: String,
}

async fn mkdir(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MkdirRequest>,
) -> Response {
    if let Some(d) = guard(&state, &headers) {
        return d;
    }
    let (target, _) = match resolve_new(&state, &body.root, &body.path, &body.name) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if target.exists() {
        return err(StatusCode::CONFLICT, "exists", "already exists");
    }
    match tokio::fs::create_dir(&target).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "mkdir_failed", e.to_string()),
    }
}

#[derive(Deserialize)]
struct RenameRequest {
    root: String,
    /// Existing path, relative to the root.
    path: String,
    /// New basename. Renaming, not moving — a move needs a destination path and
    /// its own confinement check.
    name: String,
}

async fn rename(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RenameRequest>,
) -> Response {
    if let Some(d) = guard(&state, &headers) {
        return d;
    }
    let root = match writable_root(&state, &body.root) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let from = match fpath::resolve(root, &body.path) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::NOT_FOUND, "not_found", e.public_message()),
    };
    let name = body.name.trim();
    if name.is_empty() || !fpath::is_safe_new_path(name) || name.contains('/') {
        return err(StatusCode::BAD_REQUEST, "bad_name", "invalid name");
    }
    let Some(parent) = from.parent() else {
        return err(StatusCode::BAD_REQUEST, "no_parent", "cannot rename this");
    };
    let to = parent.join(name);
    if to.exists() {
        return err(StatusCode::CONFLICT, "exists", "already exists");
    }
    match tokio::fs::rename(&from, &to).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "rename_failed", e.to_string()),
    }
}

#[derive(Deserialize)]
struct DeleteRequest {
    root: String,
    path: String,
    /// Required for a non-empty directory, so a stray click cannot remove a
    /// tree. There is no undo here and no trash.
    #[serde(default)]
    recursive: bool,
}

async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DeleteRequest>,
) -> Response {
    if let Some(d) = guard(&state, &headers) {
        return d;
    }
    let root = match writable_root(&state, &body.root) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let target = match fpath::resolve(root, &body.path) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::NOT_FOUND, "not_found", e.public_message()),
    };
    // Deleting the root itself would remove the thing the operator configured.
    if target == root.path {
        return err(StatusCode::FORBIDDEN, "is_root", "cannot delete a root");
    }

    let result = if target.is_dir() {
        if body.recursive {
            tokio::fs::remove_dir_all(&target).await
        } else {
            tokio::fs::remove_dir(&target).await
        }
    } else {
        tokio::fs::remove_file(&target).await
    };
    match result {
        Ok(()) => {
            warn!(path = %target.display(), "file deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::CONFLICT, "delete_failed", e.to_string()),
    }
}

/// Upload one file. Destination comes from the query, body is the raw bytes.
///
/// Streamed to disk rather than buffered: the operator moves model files, and
/// reading a 20 GB upload into memory would be the failure this daemon exists
/// to prevent.
async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<MkdirRequest>,
    body: axum::body::Body,
) -> Response {
    if let Some(d) = guard(&state, &headers) {
        return d;
    }
    let (target, _) = match resolve_new(&state, &q.root, &q.path, &q.name) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if target.exists() {
        return err(StatusCode::CONFLICT, "exists", "already exists");
    }

    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    // Written to a temporary name and renamed, so an interrupted upload never
    // leaves a truncated file that looks complete.
    let tmp = target.with_extension("prism-upload");
    let mut file = match tokio::fs::File::create(&tmp).await {
        Ok(f) => f,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "create_failed", e.to_string()),
    };

    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return err(StatusCode::BAD_REQUEST, "upload_failed", e.to_string());
            }
        };
        if let Err(e) = file.write_all(&chunk).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return err(StatusCode::INTERNAL_SERVER_ERROR, "write_failed", e.to_string());
        }
    }
    if let Err(e) = file.sync_all().await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return err(StatusCode::INTERNAL_SERVER_ERROR, "sync_failed", e.to_string());
    }
    drop(file);

    match tokio::fs::rename(&tmp, &target).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            err(StatusCode::INTERNAL_SERVER_ERROR, "rename_failed", e.to_string())
        }
    }
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

/// Files need an unlocked session, like everything else that is not public.
fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if state.roots.is_empty() {
        return Some(err(
            StatusCode::FORBIDDEN,
            "files_disabled",
            "no file roots are configured",
        ));
    }
    require(state, headers, Sensitivity::Session)
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

/// Serve a file's bytes, streamed, with HTTP range support.
///
/// Streaming rather than reading into memory is not an optimisation here: a
/// model file on this host is routinely 20 GB, and buffering one to serve it
/// would be a memory incident caused by the daemon built to prevent memory
/// incidents.
///
/// Range support is what makes video play at all. A browser will not seek —
/// and often refuses to start — without `Accept-Ranges: bytes` and a 206 for
/// partial requests. A 215 MB recording failed to load for exactly this reason.
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

    let meta = match tokio::fs::metadata(&full).await {
        Ok(m) => m,
        Err(e) => return err(StatusCode::NOT_FOUND, "read_failed", e.to_string()),
    };
    if meta.is_dir() {
        return err(StatusCode::BAD_REQUEST, "is_a_directory", "is a directory");
    }
    let len = meta.len();

    let name = full
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let mime = mime_for(&name);

    let mut file = match tokio::fs::File::open(&full).await {
        Ok(f) => f,
        Err(e) => return err(StatusCode::NOT_FOUND, "read_failed", e.to_string()),
    };

    // A byte range, if the client asked for one. Only the single-range form is
    // supported; multipart ranges are vanishingly rare and not worth the
    // complexity of a multipart body.
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, len));

    let (status, start, count) = match range {
        Some((s, e)) => (StatusCode::PARTIAL_CONTENT, s, e - s + 1),
        None => (StatusCode::OK, 0, len),
    };

    if start > 0 {
        use tokio::io::AsyncSeekExt;
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "seek_failed", "cannot seek");
        }
    }

    let stream = tokio_util::io::ReaderStream::new(file.take(count));
    let mut resp = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, count)
        .header(header::CACHE_CONTROL, "private, max-age=60");

    if status == StatusCode::PARTIAL_CONTENT {
        resp = resp.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, start + count - 1, len),
        );
    }
    if q.download.unwrap_or(false) {
        resp = resp.header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", name.replace('"', "")),
        );
    }

    resp.body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Parse a single `bytes=` range against a known length.
///
/// Returns an inclusive `(start, end)`. Unsatisfiable or malformed ranges yield
/// `None`, which the caller treats as "send the whole file" — more forgiving
/// than a 416, and a browser that sent a bad range still gets playable bytes.
fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header.strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None; // multipart ranges unsupported
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        // bytes=-500 — the final 500 bytes.
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            if n == 0 || len == 0 {
                return None;
            }
            (len.saturating_sub(n), len - 1)
        }
        // bytes=500- — from 500 to the end.
        (s, "") => (s.parse().ok()?, len.checked_sub(1)?),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(len.saturating_sub(1))),
    };
    if start > end || start >= len {
        return None;
    }
    Some((start, end))
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

    #[test]
    fn parses_a_normal_range() {
        assert_eq!(parse_range("bytes=0-499", 1000), Some((0, 499)));
        assert_eq!(parse_range("bytes=500-999", 1000), Some((500, 999)));
    }

    #[test]
    fn open_ended_range_runs_to_the_end() {
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
    }

    #[test]
    fn suffix_range_takes_the_tail() {
        assert_eq!(parse_range("bytes=-500", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-5000", 1000), Some((0, 999)));
    }

    #[test]
    fn an_end_past_the_file_is_clamped() {
        // Browsers routinely ask for more than exists at the tail of a video.
        assert_eq!(parse_range("bytes=900-9999", 1000), Some((900, 999)));
    }

    #[test]
    fn unsatisfiable_and_malformed_ranges_fall_back_to_the_whole_file() {
        for bad in ["bytes=2000-3000", "bytes=500-100", "nonsense", "bytes=", "bytes=abc-def", "bytes=0-1,5-6"] {
            assert_eq!(parse_range(bad, 1000), None, "{bad:?}");
        }
    }

    #[test]
    fn range_on_an_empty_file_is_none() {
        assert_eq!(parse_range("bytes=0-", 0), None);
        assert_eq!(parse_range("bytes=-10", 0), None);
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
