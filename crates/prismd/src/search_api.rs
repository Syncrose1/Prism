//! **Finding a file by name, anywhere Prism shares** (Ctrl+K in Prism OS).
//!
//! Instant where the machine keeps a file index (`plocate`, updated daily by
//! most distributions), with a short live walk near the top of each shared
//! folder for what's newer than the index; the walk alone where there's no
//! index. Every hit is confined to a shared folder (and, for a guest, to the
//! folders opened to guests), hidden paths are left out, and the whole thing
//! stays inside a time budget so a huge home never holds the box up.

use crate::api::{AppState, require};
use axum::{Json, Router, extract::{Query, State}, http::HeaderMap, response::{IntoResponse, Response}, routing::get};
use prism_core::auth::Sensitivity;
use prism_core::files::list::Kind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn routes() -> Router<AppState> { Router::new().route("/api/files/search", get(search)) }

#[derive(Deserialize)]
struct Q { q: String, #[serde(default)] limit: Option<usize> }

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Hit { root: String, path: String, name: String, is_dir: bool, kind: Kind, modified: Option<u64> }

/// The words of a query, lowercased; every one must be in the name.
fn words(q: &str) -> Vec<String> { q.split_whitespace().map(|w| w.to_lowercase()).filter(|w| !w.is_empty()).collect() }
pub fn matches(name: &str, words: &[String]) -> bool { let n = name.to_lowercase(); !words.is_empty() && words.iter().all(|w| n.contains(w)) }

/// Hidden anywhere on its way down from the root: left out.
fn hidden(rel: &Path) -> bool { rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) }

/// Folders a person never means when searching by name.
const SKIP: &[&str] = &["node_modules", "target", "__pycache__", "site-packages", "venv", ".venv", "build", "dist"];

/// A bounded breadth-first walk: shallow first, until the budget runs out.
fn walk(root: &Path, words: &[String], until: Instant, out: &mut Vec<PathBuf>, cap: usize) {
    let mut queue = std::collections::VecDeque::from([(root.to_path_buf(), 0u32)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if Instant::now() > until || out.len() >= cap { return }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') { continue }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() { continue }
            if matches(&name, words) { out.push(e.path()) }
            if ft.is_dir() && depth < 6 && !SKIP.contains(&name.as_str()) { queue.push_back((e.path(), depth + 1)) }
        }
    }
}

/// The machine's own index, if it keeps one: basenames holding every word.
fn indexed(words: &[String], cap: usize) -> Option<Vec<PathBuf>> {
    let mut cmd = std::process::Command::new("plocate");
    cmd.args(["-i", "-b", "-l", &cap.to_string(), "--"]).args(words);
    let out = cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() && out.stdout.is_empty() { return None }
    Some(String::from_utf8_lossy(&out.stdout).lines().map(PathBuf::from).collect())
}

async fn search(State(state): State<AppState>, headers: HeaderMap, Query(q): Query<Q>) -> Response {
    if let Some(d) = require(&state, &headers, Sensitivity::Session) { return d }
    let guest = state.auth.session_kind(crate::api::session_token(&headers).as_deref(), prism_core::auth::totp::now_unix()) == Some(prism_core::auth::session::TokenKind::Guest);
    let roots: Vec<(String, PathBuf)> = state.roots.iter().filter(|r| !guest || state.guest_roots.contains(&r.name)).map(|r| (r.name.clone(), r.path.clone())).collect();
    let w = words(&q.q);
    let limit = q.limit.unwrap_or(40).min(200);
    if w.is_empty() || w.iter().map(|x| x.chars().count()).sum::<usize>() < 2 || roots.is_empty() { return Json(Vec::<Hit>::new()).into_response() }
    let hits = tokio::task::spawn_blocking(move || {
        let until = Instant::now() + Duration::from_millis(900);
        let mut found: Vec<PathBuf> = indexed(&w, 5000).unwrap_or_default();
        // What's newer than the index, near the top of each shared folder.
        let each = until.saturating_duration_since(Instant::now()) / roots.len().max(1) as u32;
        for (_, p) in &roots { walk(p, &w, Instant::now() + each, &mut found, 4000) }
        let mut seen = std::collections::HashSet::new();
        let mut hits: Vec<(Hit, usize)> = Vec::new();
        for f in found {
            if !seen.insert(f.clone()) { continue }
            // The shared folder it's in (the deepest, if they nest).
            let Some((root, rp)) = roots.iter().filter(|(_, rp)| f.starts_with(rp)).max_by_key(|(_, rp)| rp.as_os_str().len()) else { continue };
            let Ok(rel) = f.strip_prefix(rp) else { continue };
            if rel.as_os_str().is_empty() || hidden(rel) || rel.components().any(|c| { let c = c.as_os_str().to_string_lossy(); SKIP.contains(&c.as_ref()) || c.starts_with("target-") }) { continue }
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !matches(&name, &w) { continue }
            let Ok(meta) = std::fs::symlink_metadata(&f) else { continue };
            if meta.file_type().is_symlink() { continue }
            let depth = rel.components().count();
            let kind = if meta.is_dir() { Kind::Dir } else { Kind::from_extension(&name) };
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
            hits.push((Hit { root: root.clone(), path: rel.to_string_lossy().into_owned(), name, is_dir: meta.is_dir(), kind, modified }, depth));
        }
        // A name that starts with what was typed first; then shallower; then newer.
        let first = w[0].clone();
        hits.sort_by(|(a, da), (b, db)| (!a.name.to_lowercase().starts_with(&first)).cmp(&!b.name.to_lowercase().starts_with(&first)).then(da.cmp(db)).then(b.modified.cmp(&a.modified)));
        hits.into_iter().take(limit).map(|(h, _)| h).collect::<Vec<_>>()
    }).await.unwrap_or_default();
    Json(hits).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_must_be_in_the_name_and_hidden_paths_never_show() {
        let w = words("Tax 2025");
        assert!(matches("tax-return-2025.pdf", &w));
        assert!(!matches("tax-return-2024.pdf", &w));
        assert!(hidden(Path::new("a/.git/config")) && !hidden(Path::new("a/b/c.txt")));
        let dir = std::env::temp_dir().join(format!("prism-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Docs/.secret")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("Docs/tax-2025.pdf"), b"").unwrap();
        std::fs::write(dir.join("Docs/.secret/tax-2025-keys.txt"), b"").unwrap();
        std::fs::write(dir.join("node_modules/tax-2025.js"), b"").unwrap();
        let mut out = Vec::new();
        walk(&dir, &w, Instant::now() + Duration::from_secs(2), &mut out, 100);
        assert_eq!(out, vec![dir.join("Docs/tax-2025.pdf")], "hidden folders and node_modules left out");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
