//! **Share links** (`prism_core::links`): the owner makes, lists and takes
//! back links (`/api/links`, behind the session); the person on the other end
//! opens `/l/<token>`, which needs no account, no install and no script.
//!
//! The pages are plain HTML with plain forms: a phone from years ago, or a
//! browser with scripts switched off, uploads and downloads as well as any.
//! Everything that happens through a link (opened, sent, taken, removed,
//! refused) is in the sealed access log under the link's name.

use crate::api::{require, AppState};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use prism_core::access::{Access, Kind};
use prism_core::auth::{Sensitivity, totp};
use prism_core::files::path::{Root, resolve};
use prism_core::links::{Link, Perms, Refusal};
use serde::Deserialize;
use std::net::SocketAddr;
use tokio::io::AsyncWriteExt;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/links", get(list).post(make))
        .route("/api/links/{id}/revoke", post(revoke))
        .route("/l/{token}", get(page))
        .route("/l/{token}/raw", get(raw))
        .route("/l/{token}/thumb", get(thumb))
        .route("/l/{token}/upload", post(upload))
        .route("/l/{token}/remove", post(remove))
}

const DEVICE_COOKIE: &str = "prism_link_device";

fn random_hex(bytes: usize) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; bytes];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_err() {
        // No randomness, no link: an empty token matches nothing.
        return String::new();
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

// ── The owner's side ─────────────────────────────────────────────────────

async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = require(&state, &headers, Sensitivity::Session) { return r }
    let s = state.links.lock().expect("links poisoned");
    let now = totp::now_unix();
    let links: Vec<_> = s.links.iter().rev().map(|l| serde_json::json!({
        "id": l.id, "label": l.label, "root": l.root, "path": l.path, "perms": l.perms, "says": l.perms.say(),
        "created": l.created, "expires": l.expires, "once": l.once, "opened_elsewhere": l.bound.is_some(),
        "revoked": l.revoked, "live": !l.revoked && now < l.expires, "visits": l.visits, "received": l.mine.len(),
    })).collect();
    Json(serde_json::json!({ "links": links, "history": s.history })).into_response()
}

#[derive(Deserialize)]
struct MakeLink {
    root: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    label: Option<String>,
    perms: Perms,
    lasts_secs: u64,
    #[serde(default)]
    once: bool,
    /// A profile the person put together themselves: kept in the history.
    #[serde(default)]
    custom: bool,
}

async fn make(State(state): State<AppState>, headers: HeaderMap, Json(b): Json<MakeLink>) -> Response {
    if let Some(r) = require(&state, &headers, Sensitivity::Session) { return r }
    // The folder must exist inside a shared root, now.
    let Some(root) = state.roots.iter().find(|r| r.name == b.root) else { return bad("no_such_root", "that folder isn't shared") };
    let folder = match resolve(root, &b.path) { Ok(p) if p.is_dir() => p, _ => return bad("no_such_folder", "that folder doesn't exist") };
    if b.perms.writes() && !root.writable { return bad("read_only", "that folder can't be changed from PRISM, so a link can't upload into it") }
    if !b.perms.lists() && !b.perms.up { return bad("nothing_allowed", "the link would allow nothing") }
    let token = random_hex(16);
    if token.len() != 32 { return (StatusCode::SERVICE_UNAVAILABLE, "no randomness for a token").into_response() }
    let label = b.label.filter(|l| !l.trim().is_empty()).unwrap_or_else(|| folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| b.root.clone()));
    let link = {
        let mut s = state.links.lock().expect("links poisoned");
        let l = s.make(token, label, b.root, b.path, b.perms, b.lasts_secs, b.once, b.custom, totp::now_unix());
        if let Err(e) = s.save(&state.state_dir) { return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response() }
        l
    };
    Json(serde_json::json!({ "id": link.id, "url_path": format!("/l/{}", link.token), "expires": link.expires, "says": link.perms.say() })).into_response()
}

async fn revoke(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = require(&state, &headers, Sensitivity::Session) { return r }
    let mut s = state.links.lock().expect("links poisoned");
    if !s.revoke(&id) { return (StatusCode::NOT_FOUND, "no such link").into_response() }
    let _ = s.save(&state.state_dir);
    Json(serde_json::json!({ "ok": true })).into_response()
}

fn bad(error: &'static str, detail: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": error, "detail": detail }))).into_response()
}

// ── The other side ───────────────────────────────────────────────────────

/// The device this browser is, from its cookie, and whether it was just made.
fn device(headers: &HeaderMap) -> (String, bool) {
    let found = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).and_then(|c| c.split(';').find_map(|p| {
        let (k, v) = p.split_once('=')?; (k.trim() == DEVICE_COOKIE && v.trim().len() == 32).then(|| v.trim().to_string())
    }));
    match found { Some(d) => (d, false), None => (random_hex(16), true) }
}

fn log(state: &AppState, link: &Link, kind: Kind, peer: SocketAddr, what: String) {
    let a = Access { who: format!("link:{}", link.label), for_account: None, how: "link".into(), from: peer.ip().to_string(), what };
    let _ = state.access.append(kind, a, totp::now_unix());
}

/// Let this request in, or say plainly why not. The link's folder comes back
/// as its own root, so every path below it is confined by the same check that
/// guards Files.
fn admit(state: &AppState, token: &str, headers: &HeaderMap, peer: SocketAddr, count: bool) -> Result<(Link, Root, String, bool), Response> {
    let (dev, fresh) = device(headers);
    let now = totp::now_unix();
    let res = {
        let mut s = state.links.lock().expect("links poisoned");
        let r = if count { s.admit(token, &dev, now) } else {
            match s.find(token) { None => Err(Refusal::Unknown), Some(i) => s.links[i].check(Some(&dev), now).map(|_| s.links[i].clone()) }
        };
        if count && r.is_ok() { let _ = s.save(&state.state_dir); }
        r
    };
    match res {
        Ok(link) => {
            let base = state.roots.iter().find(|r| r.name == link.root).and_then(|r| resolve(r, &link.path).ok());
            let Some(root) = base.and_then(|p| Root::new(link.label.clone(), p, link.perms.writes())) else {
                return Err(refused(StatusCode::GONE, "The folder this link shared isn't there any more."));
            };
            Ok((link, root, dev, fresh))
        }
        Err(why) => {
            // Refusals are logged against the link when there is one to name.
            if why != Refusal::Unknown && let Some(l) = state.links.lock().ok().and_then(|s| s.find(token).map(|i| s.links[i].clone())) {
                log(state, &l, Kind::Refused, peer, format!("refused: {}", why.say()));
            }
            Err(refused(if why == Refusal::Unknown { StatusCode::NOT_FOUND } else { StatusCode::GONE }, why.say()))
        }
    }
}

fn refused(code: StatusCode, why: &str) -> Response {
    (code, Html(shell("Not available", &format!(r#"<div class="card"><h1>Not available</h1><p>{}</p></div>"#, esc(why))))).into_response()
}

fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }
fn qs(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[derive(Deserialize, Default)]
struct At {
    #[serde(default)]
    p: String,
    #[serde(default)]
    dl: Option<u8>,
    #[serde(default)]
    said: Option<String>,
}

async fn page(State(state): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Path(token): Path<String>, Query(at): Query<At>) -> Response {
    let (link, root, dev, fresh) = match admit(&state, &token, &headers, peer, true) { Ok(v) => v, Err(r) => return r };
    if fresh || at.p.is_empty() { log(&state, &link, Kind::SignedIn, peer, format!("opened \"{}\"", link.label)); }
    let here = at.p.trim_matches('/').to_string();
    if !link.shows(&here) { return refused(StatusCode::NOT_FOUND, "That isn't part of what was shared.") }
    let dir = match resolve(&root, &here) { Ok(d) if d.is_dir() => d, _ => return refused(StatusCode::NOT_FOUND, "That isn't part of what was shared.") };
    let mut rows = String::new();
    if link.perms.lists() {
        let listing = prism_core::files::list::list(&dir, 0, 2000, Default::default());
        let mut entries: Vec<_> = listing.map(|l| l.entries).unwrap_or_default().into_iter().filter(|e| !e.name.starts_with('.')).collect();
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        for e in entries {
            let rel = if here.is_empty() { e.name.clone() } else { format!("{here}/{}", e.name) };
            if !link.shows(&rel) { continue }
            let base = format!("/l/{token}");
            let thumb = if link.perms.stream && matches!(e.kind, prism_core::files::list::Kind::Image | prism_core::files::list::Kind::Video) {
                format!(r#"<img loading="lazy" alt="" src="{base}/thumb?p={}">"#, qs(&rel))
            } else { String::new() };
            let name = if e.is_dir { format!(r#"<a href="{base}?p={}">{}</a>"#, qs(&rel), esc(&e.name)) }
                else if link.perms.stream { format!(r#"<a href="{base}/raw?p={}">{}</a>"#, qs(&rel), esc(&e.name)) }
                else { esc(&e.name) };
            let size = if e.is_dir { "Folder".to_string() } else { e.size.map(human).unwrap_or_default() };
            let mut acts = String::new();
            if !e.is_dir && link.perms.down { acts += &format!(r#"<a class="btn" href="{base}/raw?p={}&dl=1">Download</a>"#, qs(&rel)) }
            if link.perms.edit { acts += &format!(r#"<form method="post" action="{base}/remove"><input type="hidden" name="p" value="{}"><button class="btn q">Remove</button></form>"#, esc(&rel)) }
            let glyph = match e.kind { prism_core::files::list::Kind::Dir => G_FOLDER, prism_core::files::list::Kind::Audio => G_MUSIC, prism_core::files::list::Kind::Image => G_PHOTO, prism_core::files::list::Kind::Video => G_FILM, _ => G_DOC };
            rows += &format!(r#"<li><span class="th">{glyph}{thumb}<span class="ic">{}</span></span><span class="nm">{name}<small>{size}</small></span><span class="acts">{acts}</span></li>"#, "");
        }
        if rows.is_empty() { rows = format!(r#"<li class="none">{}</li>"#, if link.perms.see { "Nothing here yet." } else { "Nothing sent yet. What you send will show here, and only to you." }) }
    }
    let crumbs = {
        let mut c = format!(r#"<a href="/l/{token}">{}</a>"#, esc(&link.label));
        let mut acc = String::new();
        for part in here.split('/').filter(|s| !s.is_empty()) { if !acc.is_empty() { acc.push('/') } acc += part; c += &format!(r#" › <a href="/l/{token}?p={}">{}</a>"#, qs(&acc), esc(part)) }
        c
    };
    let upload = if link.perms.up { format!(r#"<form class="drop" method="post" action="/l/{token}/upload?p={}" enctype="multipart/form-data"><label>Send files here<input type="file" name="file" multiple required></label><button class="btn">Send</button></form>"#, qs(&here)) } else { String::new() };
    let said = at.said.as_deref().map(|s| format!(r#"<p class="said">{}</p>"#, esc(s))).unwrap_or_default();
    let ends = if link.once { format!("Open on this device only, for another {}", until(link.expires)) } else { format!("Open for another {}", until(link.expires)) };
    let body = format!(r#"<header><div class="mark">{MARK}</div><div><h1>{}</h1><p>Shared with you through PRISM</p></div></header>
<div class="card"><p class="crumbs">{crumbs}</p><p class="can">You can {}. {ends}.</p>{said}{upload}{}</div>"#,
        esc(&link.label), esc(&link.perms.say()), if link.perms.lists() { format!("<ul>{rows}</ul>") } else { String::new() });
    let mut resp = Html(shell(&link.label, &body)).into_response();
    if fresh { if let Ok(v) = format!("{DEVICE_COOKIE}={dev}; Path=/l/; HttpOnly; SameSite=Lax; Max-Age=7776000").parse() { resp.headers_mut().append(header::SET_COOKIE, v); } }
    resp.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    resp.headers_mut().insert("referrer-policy", "no-referrer".parse().unwrap());
    resp
}

async fn raw(State(state): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Path(token): Path<String>, Query(at): Query<At>) -> Response {
    let (link, root, _, _) = match admit(&state, &token, &headers, peer, false) { Ok(v) => v, Err(r) => return r };
    let download = at.dl == Some(1);
    if (download && !link.perms.down) || (!download && !link.perms.stream && !link.perms.down) || !link.shows(&at.p) {
        return refused(StatusCode::FORBIDDEN, "This link doesn't allow that.");
    }
    let Ok(full) = resolve(&root, &at.p) else { return refused(StatusCode::NOT_FOUND, "That isn't part of what was shared.") };
    // Whole downloads are logged; a film's range requests aren't, one by one.
    if download || !headers.contains_key(header::RANGE) { log(&state, &link, Kind::SignedIn, peer, format!("{} \"{}\"", if download { "downloaded" } else { "opened" }, at.p)); }
    crate::files_api::send_file(&full, &headers, download && link.perms.down).await
}

async fn thumb(State(state): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Path(token): Path<String>, Query(at): Query<At>) -> Response {
    let (link, root, _, _) = match admit(&state, &token, &headers, peer, false) { Ok(v) => v, Err(r) => return r };
    if !link.perms.stream || !link.shows(&at.p) { return StatusCode::FORBIDDEN.into_response() }
    let Ok(full) = resolve(&root, &at.p) else { return StatusCode::NOT_FOUND.into_response() };
    match crate::files_api::render_thumb(&full, &state.thumb_dir).await {
        Some(bytes) => ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "private, max-age=600")], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Plain form uploads, streamed to disk a chunk at a time (a 20 GB file
/// never sits in memory), into the folder the page was showing.
async fn upload(State(state): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Path(token): Path<String>, Query(at): Query<At>, mut form: Multipart) -> Response {
    let (link, root, _, _) = match admit(&state, &token, &headers, peer, false) { Ok(v) => v, Err(r) => return r };
    if !link.perms.up { return refused(StatusCode::FORBIDDEN, "This link doesn't allow sending files.") }
    let here = at.p.trim_matches('/').to_string();
    if !link.shows(&here) { return refused(StatusCode::NOT_FOUND, "That isn't part of what was shared.") }
    let Ok(dir) = resolve(&root, &here) else { return refused(StatusCode::NOT_FOUND, "That isn't part of what was shared.") };
    let mut sent = Vec::new();
    while let Ok(Some(mut field)) = form.next_field().await {
        let Some(name) = field.file_name().map(|n| n.rsplit(['/', '\\']).next().unwrap_or(n).to_string()) else { continue };
        if name.is_empty() || name.starts_with('.') || !prism_core::files::path::is_safe_new_path(&name) { continue }
        // Never over something already there: a second copy gets a number.
        let mut target = dir.join(&name);
        let mut n = 2;
        while target.exists() { let (stem, ext) = name.rsplit_once('.').map(|(s, e)| (s.to_string(), format!(".{e}"))).unwrap_or((name.clone(), String::new())); target = dir.join(format!("{stem} ({n}){ext}")); n += 1; }
        let Ok(mut f) = tokio::fs::File::create(&target).await else { continue };
        let mut ok = true;
        while let Ok(Some(chunk)) = field.chunk().await { if f.write_all(&chunk).await.is_err() { ok = false; break } }
        if !ok { let _ = tokio::fs::remove_file(&target).await; continue }
        let _ = f.flush().await;
        let rel = prism_core::files::path::relative_to(&root, &target);
        state.links.lock().expect("links poisoned").note_mine(&token, &rel);
        log(&state, &link, Kind::Changed, peer, format!("sent \"{rel}\""));
        sent.push(target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    }
    let _ = state.links.lock().expect("links poisoned").save(&state.state_dir);
    let said = match sent.len() { 0 => "Nothing was sent.".to_string(), 1 => format!("Sent {}.", sent[0]), n => format!("Sent {n} files.") };
    Redirect::to(&format!("/l/{token}?p={}&said={}", qs(&here), qs(&said))).into_response()
}

#[derive(Deserialize)]
struct RemoveForm { p: String }

async fn remove(State(state): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Path(token): Path<String>, axum::extract::Form(f): axum::extract::Form<RemoveForm>) -> Response {
    let (link, root, _, _) = match admit(&state, &token, &headers, peer, false) { Ok(v) => v, Err(r) => return r };
    if !link.perms.edit || !link.shows(&f.p) || f.p.trim_matches('/').is_empty() { return refused(StatusCode::FORBIDDEN, "This link doesn't allow removing that.") }
    let Ok(full) = resolve(&root, &f.p) else { return refused(StatusCode::NOT_FOUND, "That isn't part of what was shared.") };
    let r = if full.is_dir() { tokio::fs::remove_dir(&full).await } else { tokio::fs::remove_file(&full).await };
    let parent = f.p.trim_matches('/').rsplit_once('/').map(|(a, _)| a.to_string()).unwrap_or_default();
    let said = match r { Ok(()) => { log(&state, &link, Kind::Changed, peer, format!("removed \"{}\"", f.p)); format!("Removed {}.", f.p.rsplit('/').next().unwrap_or(&f.p)) } Err(e) => format!("Couldn't remove it: {e}") };
    Redirect::to(&format!("/l/{token}?p={}&said={}", qs(&parent), qs(&said))).into_response()
}

fn human(b: u64) -> String {
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else if b >= 1 << 20 { format!("{:.1} MB", b as f64 / (1u64 << 20) as f64) } else if b >= 1024 { format!("{} KB", b / 1024) } else { format!("{b} B") }
}

/// When a link ends, plainly (the server's clock: the person may be anywhere).
fn until(unix: u64) -> String {
    let left = unix.saturating_sub(totp::now_unix());
    let n = |v: u64, one: &str, many: &str| if v == 1 { format!("1 {one}") } else { format!("{v} {many}") };
    if left < 3600 { n((left / 60).max(1), "minute", "minutes") } else if left < 86_400 { n(left / 3600, "hour", "hours") } else { n(left / 86_400, "day", "days") }
}

const G_FOLDER: &str = r##"<svg viewBox="0 0 24 24" width="26" height="26" fill="none" stroke="#E5A23A" stroke-width="2.4" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>"##;
const G_MUSIC: &str = r##"<svg viewBox="0 0 24 24" width="26" height="26" fill="none" stroke="#7C5CE0" stroke-width="2.4" stroke-linecap="round"><path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/></svg>"##;
const G_PHOTO: &str = r##"<svg viewBox="0 0 24 24" width="26" height="26" fill="none" stroke="#2E71C8" stroke-width="2.4" stroke-linejoin="round"><rect x="3" y="5" width="18" height="14" rx="3"/><path d="M4 17l5-5 4 4 3-3 4 4"/></svg>"##;
const G_FILM: &str = r##"<svg viewBox="0 0 24 24" width="26" height="26" fill="none" stroke="#C0485C" stroke-width="2.4" stroke-linejoin="round"><path d="M8 5l11 7-11 7z"/></svg>"##;
const G_DOC: &str = r##"<svg viewBox="0 0 24 24" width="26" height="26" fill="none" stroke="#9A978F" stroke-width="2.4" stroke-linejoin="round"><path d="M7 3h7l4 4v14H7z"/><path d="M14 3v4h4"/></svg>"##;

const MARK: &str = r##"<svg viewBox="0 0 100 100" aria-hidden="true"><path d="M18 86 L50 64 L82 86" fill="none" stroke="#D6D2C8" stroke-width="8" stroke-linecap="round" stroke-linejoin="round"/><path d="M18 68 L50 46 L82 68" fill="none" stroke="#2F3138" stroke-width="8" stroke-linecap="round" stroke-linejoin="round"/><path d="M50 33 L50 6" stroke="#33BFE2" stroke-width="7" stroke-linecap="round"/><path d="M50 33 L72 12" stroke="#E5A23A" stroke-width="7" stroke-linecap="round"/><path d="M50 33 L28 12" stroke="#7C5CE0" stroke-width="7" stroke-linecap="round"/></svg>"##;

/// The page around a link: Pearl, POLARIS's face, no scripts.
fn shell(title: &str, body: &str) -> String {
    format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex"><title>{}</title>
<style>
@font-face{{font-family:R;src:url(/ui/vendor/mplus-rounded-regular.woff2) format("woff2");font-weight:400}}
@font-face{{font-family:R;src:url(/ui/vendor/mplus-rounded-medium.woff2) format("woff2");font-weight:500}}
@font-face{{font-family:R;src:url(/ui/vendor/mplus-rounded-bold.woff2) format("woff2");font-weight:700}}
body{{margin:0;background:#E6E3DB;color:#2F3138;font:400 17px/1.55 R,"M PLUS Rounded 1c",system-ui,sans-serif;padding:28px 16px 48px}}
main{{max-width:760px;margin:0 auto;display:grid;gap:18px}}
header{{display:flex;gap:14px;align-items:center}}.mark svg{{width:56px;height:56px}}
h1{{margin:0;font-size:26px;font-weight:700}}header p{{margin:2px 0 0;color:#77756E;font-size:15px}}
.card{{background:#F6F4EF;border-radius:24px;padding:18px;box-shadow:0 12px 30px rgba(47,49,56,.12)}}
.crumbs{{margin:0 0 4px;font-weight:500}}.crumbs a{{color:#2F3138}}.can{{margin:0 0 12px;color:#77756E;font-size:15px}}
.said{{background:#DDF3EA;color:#23775A;border-radius:12px;padding:8px 12px;font-size:15px}}
ul{{list-style:none;margin:0;padding:0;display:grid;gap:8px}}
li{{display:grid;grid-template-columns:56px minmax(0,1fr) auto;gap:12px;align-items:center;background:#fff;border-radius:15px;padding:8px}}
li.none{{display:block;color:#77756E;font-size:15px;padding:14px}}
.th{{position:relative;width:56px;height:56px;border-radius:11px;background:#EEECE6;overflow:hidden;display:grid;place-items:center;color:#E5A23A;font-size:22px}}
.th img{{position:absolute;inset:0;width:100%;height:100%;object-fit:cover}}
.nm{{min-width:0;overflow-wrap:anywhere}}.nm a{{color:#2F3138;font-weight:500}}.nm small{{display:block;color:#77756E;font-size:15px}}
.acts{{display:flex;gap:6px;flex-wrap:wrap;justify-content:flex-end}}.acts form{{margin:0}}
.btn{{display:inline-block;border:0;border-radius:999px;padding:9px 14px;background:#33BFE2;color:#fff;font:500 15px/1 R,system-ui,sans-serif;text-decoration:none;cursor:pointer}}
.btn.q{{background:#EEECE6;color:#2F3138}}
.drop{{display:flex;gap:10px;align-items:center;flex-wrap:wrap;border:3px dashed #D2CFC6;border-radius:16px;padding:14px;margin:0 0 12px}}
.drop label{{flex:1;min-width:220px;display:grid;gap:6px;font-weight:500}}.drop input{{font:inherit;font-size:15px}}
@media (prefers-color-scheme:dark){{body{{background:#34373D;color:#F0F2F6}}.card{{background:#3E4148}}.crumbs a,h1{{color:#F0F2F6}}header p,.can{{color:#B4B8C0}}}}
</style></head><body><main>{}</main></body></html>"#, esc(title), body)
}
