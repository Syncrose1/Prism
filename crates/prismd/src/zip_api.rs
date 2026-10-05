//! **A folder, or several things, as one download.** Zipped as it's sent:
//! nothing is gathered on disk or in memory first (a folder here can be
//! hundreds of gigabytes), and stored rather than compressed, since what
//! people send each other (pictures, films, music) is compressed already and
//! squeezing it again only costs the PC time.

use crate::api::AppState;
use axum::{body::Body, extract::{Query, State}, http::{HeaderMap, StatusCode, header}, response::{IntoResponse, Response}};
use serde::Deserialize;
use std::io::Write;

#[derive(Deserialize)]
pub struct ZipQuery {
    root: String,
    /// The folder the names are in, relative to the root.
    #[serde(default)]
    dir: String,
    /// A JSON list of names in `dir`; empty means `dir` itself.
    #[serde(default)]
    names: String,
    /// What to call the download.
    #[serde(default)]
    name: String,
}

/// A `Write` that hands each piece to the response as it's made.
struct Pipe(tokio::sync::mpsc::Sender<Result<Vec<u8>, std::io::Error>>);
impl Write for Pipe {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.blocking_send(Ok(b.to_vec())).map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the download was cancelled"))?;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

pub async fn zip(State(state): State<AppState>, headers: HeaderMap, Query(q): Query<ZipQuery>) -> Response {
    if let Some(d) = crate::files_api::guard_pub(&state, &headers) { return d }
    let (_root, base) = match crate::files_api::resolve_pub(&state, &q.root, &q.dir) { Ok(v) => v, Err(r) => return r };
    let names: Vec<String> = if q.names.trim().is_empty() { vec![] } else {
        match serde_json::from_str(&q.names) { Ok(v) => v, Err(_) => return (StatusCode::BAD_REQUEST, "names must be a JSON list").into_response() }
    };
    // Each named thing is resolved through the root, so nothing outside it
    // can be named in.
    let mut items = Vec::new();
    if names.is_empty() { items.push((base.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| q.root.clone()), base.clone())) }
    for n in &names {
        let rel = if q.dir.is_empty() { n.clone() } else { format!("{}/{n}", q.dir) };
        match crate::files_api::resolve_pub(&state, &q.root, &rel) { Ok((_, p)) => items.push((n.clone(), p)), Err(r) => return r }
    }
    let file = {
        let n = if !q.name.trim().is_empty() { q.name.clone() } else if items.len() == 1 { items[0].0.clone() } else { "files".into() };
        let n: String = n.chars().filter(|c| !"\"\\/\r\n".contains(*c)).collect();
        format!("{n}.zip")
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(16);
    tokio::task::spawn_blocking(move || {
        let mut z = zip::ZipWriter::new_stream(Pipe(tx.clone()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
        let add = |z: &mut zip::ZipWriter<_>, name: &str, path: &std::path::Path| -> std::io::Result<()> {
            z.start_file(name, opts)?;
            let mut f = std::fs::File::open(path)?;
            std::io::copy(&mut f, z)?;
            Ok(())
        };
        let r: std::io::Result<()> = (|| {
            for (name, path) in &items {
                if path.is_dir() {
                    let mut stack = vec![(name.clone(), path.clone())];
                    while let Some((prefix, dir)) = stack.pop() {
                        z.add_directory(format!("{prefix}/"), opts)?;
                        let mut kids: Vec<_> = std::fs::read_dir(&dir)?.flatten().collect();
                        kids.sort_by_key(|e| e.file_name());
                        for e in kids {
                            let ft = match e.file_type() { Ok(t) => t, Err(_) => continue };
                            // Links are left out: one could point outside the root.
                            if ft.is_symlink() { continue }
                            let n = format!("{prefix}/{}", e.file_name().to_string_lossy());
                            if ft.is_dir() { stack.push((n, e.path())) } else if ft.is_file() { add(&mut z, &n, &e.path())? }
                        }
                    }
                } else {
                    add(&mut z, name, path)?;
                }
            }
            z.finish().map(|_| ()).map_err(std::io::Error::other)
        })();
        if let Err(e) = r { let _ = tx.blocking_send(Err(e)); }
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|c| (c, rx)) });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{file}\""))
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
