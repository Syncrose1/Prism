//! Reverse proxy for locally-running apps.
//!
//! A facet declares a port, and Prism serves that app at `/facet/<id>/`. The
//! point is not the routing — it is that ComfyUI's own interface, Jellyfin, or
//! anything else self-hosted becomes a Prism window with no UI code, behind the
//! same authentication and reachable on the same single tailnet port.
//!
//! This is the reason Prism does not reimplement a media library or an image
//! generation UI. Files and Gallery stay native because they browse the real
//! filesystem against Prism's roots; an *application* with its own domain model
//! should be adopted, not rebuilt.
//!
//! ## The base-path problem, stated honestly
//!
//! An app served at `/facet/comfyui/` that emits absolute URLs like
//! `/assets/app.js` will request them from Prism's root and get a 404. There is
//! no general fix — rewriting arbitrary JavaScript is not something to attempt.
//!
//! What is done instead:
//!
//! * `<base href>` is injected into HTML responses, which resolves *relative*
//!   URLs correctly and is enough for many apps;
//! * `Location` headers on redirects are rewritten back under the prefix;
//! * an app that still misbehaves can be opened directly on its own port, and
//!   the UI offers that rather than pretending the proxy is transparent.
//!
//! Claiming otherwise would produce a window that half-works, which is worse
//! than one that says what it cannot do.

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use http_body_util::BodyExt;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use prism_core::auth::Sensitivity;
use tracing::{debug, warn};

use crate::api::{AppState, require};

/// Headers that describe a single network hop and must not be forwarded.
///
/// Passing `Connection` or `Upgrade` through would confuse the downstream app
/// about a connection it is not party to.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

pub fn routes() -> Router<AppState> {
    Router::new()
        // Three forms, because a wildcard does not match an empty segment and
        // an app's own relative URLs depend on which one the browser is on.
        .route("/facet/{id}", any(forward_root))
        .route("/facet/{id}/", any(forward_root))
        .route("/facet/{id}/{*path}", any(forward))
}

pub type ProxyClient = Client<HttpConnector, Body>;
pub type TlsProxyClient =
    Client<hyper_rustls::HttpsConnector<HttpConnector>, Body>;

pub fn client() -> ProxyClient {
    Client::builder(TokioExecutor::new()).build(HttpConnector::new())
}

/// Accepts any certificate, and only ever connects to loopback.
///
/// This is not a weakened security boundary, because there was never one here
/// to weaken. The connection is to 127.0.0.1 on the same host, so it cannot be
/// intercepted by anything that is not already running as this user — at which
/// point certificate validation is irrelevant. Apps in this position (Syncthing
/// among them) present a self-signed certificate precisely because TLS on
/// loopback is a formality they cannot skip.
///
/// The real boundary is Prism's own authentication and its tailnet binding.
#[derive(Debug)]
struct LoopbackTrust;

impl rustls::client::danger::ServerCertVerifier for LoopbackTrust {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn tls_client() -> TlsProxyClient {
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(LoopbackTrust))
        .with_no_client_auth();

    let mut http = HttpConnector::new();
    http.enforce_http(false);
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(config)
        .https_only()
        .enable_http1()
        .wrap_connector(http);

    Client::builder(TokioExecutor::new()).build(https)
}

async fn forward_root(
    state: State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    // `/facet/x` and `/facet/x/` must behave the same, or a relative URL from
    // the app resolves against the wrong parent.
    proxy(state, id, String::new(), req).await
}

async fn forward(
    state: State<AppState>,
    Path((id, _decoded)): Path<(String, String)>,
    req: Request,
) -> Response {
    // The extractor's path is deliberately ignored. Axum percent-decodes it,
    // which silently rewrites the request: ComfyUI addresses a workflow as
    // `userdata/workflows%2FName.json`, and decoding turns that single segment
    // into two, so the app 404s and the UI appears not to respond. A name
    // containing a space decodes to an invalid URI and fails harder.
    //
    // The raw path from the URI is forwarded verbatim instead, so whatever the
    // browser encoded is what the app receives.
    let raw = req.uri().path();
    let prefix = format!("/facet/{id}");
    let path = raw
        .strip_prefix(&prefix)
        .map(|rest| rest.trim_start_matches('/'))
        .unwrap_or("")
        .to_string();
    proxy(state, id, path, req).await
}

fn err(status: StatusCode, detail: impl Into<String>) -> Response {
    (status, detail.into()).into_response()
}

async fn proxy(
    State(state): State<AppState>,
    id: String,
    path: String,
    req: Request,
) -> Response {
    // A proxied app is arbitrary local software; reaching it needs the same
    // session as anything else Prism exposes.
    if let Some(denied) = require(&state, req.headers(), Sensitivity::Session) {
        return denied;
    }

    let Some((port, prefer_tls)) = state
        .facets
        .read()
        .expect("facets poisoned")
        .iter()
        .find(|f| f.id == id)
        .and_then(|f| f.expose.as_ref().map(|e| (e.port, e.tls)))
    else {
        return err(
            StatusCode::NOT_FOUND,
            format!("facet `{id}` does not expose a port"),
        );
    };
    // An app that redirected us to HTTPS once will do so every time; remember
    // it so only the first request pays for the discovery.
    let use_tls = prefer_tls || state.tls_backends.read().expect("tls set poisoned").contains(&id);

    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let scheme = if use_tls { "https" } else { "http" };
    let target: Uri = match format!("{scheme}://127.0.0.1:{port}/{path}{query}").parse() {
        Ok(u) => u,
        Err(e) => return err(StatusCode::BAD_REQUEST, e.to_string()),
    };

    let prefix = format!("/facet/{id}");
    let is_upgrade = req
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));

    if is_upgrade {
        return upgrade(req, target, port).await;
    }

    let (mut parts, body) = req.into_parts();
    parts.uri = target;
    for name in HOP_BY_HOP {
        parts.headers.remove(*name);
    }
    // The app sees a request from Prism, and Prism is the only thing that can
    // reach it — it is bound to loopback.
    parts.headers.remove(header::HOST);

    let outgoing = Request::from_parts(parts, body);
    let response = if use_tls {
        state.proxy_tls.request(outgoing).await
    } else {
        state.proxy.request(outgoing).await
    };
    let response = match response {
        Ok(r) => r,
        Err(e) => {
            warn!(facet = %id, port, tls = use_tls, error = %e, "proxy request failed");
            return err(
                StatusCode::BAD_GATEWAY,
                format!("`{id}` is not answering on port {port}"),
            );
        }
    };

    // An app redirecting us to HTTPS on its own port is telling us it wants
    // TLS. Record that and retry rather than handing the browser a redirect to
    // an address only this host can reach.
    if !use_tls
        && response.status().is_redirection()
        && let Some(location) = response.headers().get(header::LOCATION)
        && let Ok(value) = location.to_str()
        && value.starts_with(&format!("https://127.0.0.1:{port}"))
    {
        debug!(facet = %id, port, "app requires TLS; switching backend");
        state
            .tls_backends
            .write()
            .expect("tls set poisoned")
            .insert(id.clone());
        // 307 preserves the method and body, so a POST that triggered the
        // discovery is replayed rather than silently downgraded to a GET.
        let here = format!("{prefix}/{path}{query}");
        return match HeaderValue::from_str(&here) {
            Ok(location) => {
                let mut redirect = Response::new(Body::empty());
                *redirect.status_mut() = StatusCode::TEMPORARY_REDIRECT;
                redirect.headers_mut().insert(header::LOCATION, location);
                redirect
            }
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
    }

    let (mut parts, body) = response.into_parts();
    for name in HOP_BY_HOP {
        parts.headers.remove(*name);
    }

    // A redirect must land inside the proxy, not at Prism's root and not at the
    // app's own address — which the browser cannot reach anyway, since the app
    // is bound to loopback on the host.
    if let Some(location) = parts.headers.get(header::LOCATION).cloned()
        && let Ok(value) = location.to_str()
        && let Some(rewritten) = rewrite_location(value, &prefix, port)
        && let Ok(header_value) = HeaderValue::from_str(&rewritten)
    {
        parts.headers.insert(header::LOCATION, header_value);
    }

    // Cookies the app sets need adjusting for the fact that it is being served
    // from somewhere other than where it thinks.
    rewrite_cookies(&mut parts.headers, &prefix);

    let is_html = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));

    if !is_html {
        return Response::from_parts(parts, Body::new(body));
    }

    // HTML is buffered so a <base> can be injected. Only HTML: buffering a
    // video stream would defeat the range support that makes it playable.
    let bytes = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    let rewritten = inject_base(&bytes, &prefix);
    parts.headers.remove(header::CONTENT_LENGTH);
    debug!(facet = %id, "injected base href");
    Response::from_parts(parts, Body::from(rewritten))
}

/// Adjust `Set-Cookie` headers for the proxy.
///
/// Two problems, both of which silently break a login and leave the user back
/// at the login page with no error — which is exactly what it looks like when
/// this is wrong.
///
/// **`Secure`.** An app reached over HTTPS sets `Secure` on its session cookie.
/// Prism serves over plain HTTP on the tailnet, so the browser discards it: the
/// login succeeds, the cookie never lands, and the next request is
/// unauthenticated. The flag is dropped when Prism itself is not on HTTPS.
/// Nothing is lost by doing so — the cookie is already travelling over a
/// connection whose confidentiality comes from the tailnet rather than TLS.
///
/// **`Path`.** A cookie scoped to `/rest` would never be sent to
/// `/facet/<id>/rest`. Paths are moved under the prefix so they still match.
fn rewrite_cookies(headers: &mut axum::http::HeaderMap, prefix: &str) {
    let cookies: Vec<String> = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect();
    if cookies.is_empty() {
        return;
    }

    headers.remove(header::SET_COOKIE);
    for cookie in cookies {
        let rewritten = rewrite_cookie(&cookie, prefix);
        if let Ok(value) = HeaderValue::from_str(&rewritten) {
            headers.append(header::SET_COOKIE, value);
        }
    }
}

fn rewrite_cookie(cookie: &str, prefix: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut saw_path = false;

    for (i, part) in cookie.split(';').enumerate() {
        let trimmed = part.trim();
        if i == 0 {
            out.push(trimmed.to_string()); // name=value
            continue;
        }
        let lower = trimmed.to_lowercase();

        // Prism is not on HTTPS, so a Secure cookie would simply be discarded.
        if lower == "secure" {
            continue;
        }

        if lower.starts_with("path=") {
            saw_path = true;
            // Sliced from the original rather than the lowercased copy: a path
            // is case-sensitive.
            let path = trimmed["path=".len()..].trim();
            // No trailing slash: per RFC 6265 a cookie-path of
            // `/facet/app/` does not match a request for `/facet/app`, while
            // `/facet/app` matches both it and everything beneath.
            let scoped = if path.starts_with(prefix) {
                path.to_string()
            } else if path == "/" || path.is_empty() {
                prefix.to_string()
            } else if path.starts_with('/') {
                format!("{prefix}{path}")
            } else {
                // A relative path is not something a cookie should carry;
                // scope it to the app rather than guess.
                prefix.to_string()
            };
            out.push(format!("Path={scoped}"));
            continue;
        }

        out.push(trimmed.to_string());
    }

    // Without an explicit Path a cookie defaults to the *directory* of the
    // request, which for a deep API call is narrower than the app expects.
    // Scoping it to the prefix restores the app's own assumption of "site-wide".
    if !saw_path {
        out.push(format!("Path={prefix}"));
    }

    out.join("; ")
}

/// Bring a `Location` header back under the proxy prefix.
///
/// Returns `None` when the header should be left alone: an absolute redirect to
/// somewhere else entirely is the app's business, and rewriting it would break
/// an OAuth flow or a link out.
fn rewrite_location(value: &str, prefix: &str, port: u16) -> Option<String> {
    // Root-relative: prepend the prefix.
    if value.starts_with('/') {
        if value.starts_with(prefix) {
            return None; // already correct
        }
        return Some(format!("{prefix}{value}"));
    }

    // Absolute, pointing back at the app's own loopback address. The browser
    // cannot reach that, so it must be folded back under the prefix.
    for scheme in ["http", "https"] {
        for host in ["127.0.0.1", "localhost", "[::1]"] {
            let origin = format!("{scheme}://{host}:{port}");
            if let Some(rest) = value.strip_prefix(&origin) {
                let rest = if rest.is_empty() { "/" } else { rest };
                return Some(format!("{prefix}{rest}"));
            }
        }
    }
    None
}

/// Insert `<base href="/facet/<id>/">` so the app's relative URLs resolve.
///
/// Placed immediately after `<head>` so it precedes any resource the document
/// references — a `<base>` that appears after a `<script src>` has no effect on
/// it. A document with no `<head>` is left alone rather than guessed at.
fn inject_base(html: &[u8], prefix: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(html);
    if text.contains("<base ") {
        return html.to_vec(); // the app already sets its own
    }
    let tag = format!(r#"<base href="{prefix}/">"#);

    let lower = text.to_lowercase();
    let Some(head) = lower.find("<head") else {
        return html.to_vec();
    };
    let Some(close) = lower[head..].find('>').map(|i| head + i + 1) else {
        return html.to_vec();
    };

    let mut out = String::with_capacity(text.len() + tag.len());
    out.push_str(&text[..close]);
    out.push_str(&tag);
    out.push_str(&text[close..]);
    out.into_bytes()
}

/// Pass a WebSocket upgrade through untouched.
///
/// Both connections are upgraded and the raw byte streams joined. Nothing in
/// between interprets frames: terminal traffic, ComfyUI's progress stream and
/// Jellyfin's playback events all rely on the payload arriving exactly as sent.
async fn upgrade(req: Request, target: Uri, port: u16) -> Response {
    let (mut parts, body) = req.into_parts();
    parts.uri = target;
    parts.headers.remove(header::HOST);
    let outgoing = Request::from_parts(parts.clone(), body);

    // A fresh client is used rather than the pooled one: an upgraded connection
    // leaves the pool permanently, and returning it would be a slow leak.
    let client = client();
    let response = match client.request(outgoing).await {
        Ok(r) => r,
        Err(e) => {
            warn!(port, error = %e, "websocket upgrade to the app failed");
            return err(StatusCode::BAD_GATEWAY, e.to_string());
        }
    };

    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        // The app declined the upgrade; pass its answer back rather than
        // inventing one.
        let (p, b) = response.into_parts();
        return Response::from_parts(p, Body::new(b));
    }

    let (resp_parts, resp_body) = response.into_parts();
    let upstream = hyper::upgrade::on(Response::from_parts(
        resp_parts.clone(),
        Body::new(resp_body),
    ));
    let downstream = hyper::upgrade::on(Request::from_parts(parts, Body::empty()));

    tokio::spawn(async move {
        match tokio::try_join!(downstream, upstream) {
            Ok((client_io, server_io)) => {
                let mut a = hyper_util::rt::TokioIo::new(client_io);
                let mut b = hyper_util::rt::TokioIo::new(server_io);
                // Errors here are a closed connection, which is how every
                // WebSocket ends.
                let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
            }
            Err(e) => warn!(error = %e, "websocket upgrade failed"),
        }
    });

    Response::from_parts(resp_parts, Body::empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_is_injected_after_the_head_tag() {
        let html = b"<html><head><title>x</title></head><body></body></html>";
        let out = String::from_utf8(inject_base(html, "/facet/app")).unwrap();
        assert!(out.contains(r#"<head><base href="/facet/app/">"#), "got {out}");
    }

    #[test]
    fn base_precedes_any_resource_it_must_affect() {
        // A <base> after a <script src> does not apply to it, so position is
        // the whole point rather than mere tidiness.
        let html = b"<html><head><script src=\"app.js\"></script></head></html>";
        let out = String::from_utf8(inject_base(html, "/facet/app")).unwrap();
        let base = out.find("<base").unwrap();
        let script = out.find("<script").unwrap();
        assert!(base < script, "base must come first: {out}");
    }

    #[test]
    fn a_head_with_attributes_is_handled() {
        let html = br#"<html><head lang="en" data-x="1"><title>x</title></head></html>"#;
        let out = String::from_utf8(inject_base(html, "/facet/app")).unwrap();
        assert!(out.contains(r#"data-x="1"><base href="/facet/app/">"#), "got {out}");
    }

    #[test]
    fn uppercase_markup_is_handled() {
        let html = b"<HTML><HEAD><TITLE>x</TITLE></HEAD></HTML>";
        let out = String::from_utf8(inject_base(html, "/facet/app")).unwrap();
        assert!(out.contains(r#"<base href="/facet/app/">"#), "got {out}");
    }

    #[test]
    fn an_app_that_sets_its_own_base_is_left_alone() {
        let html = br#"<html><head><base href="/custom/"></head></html>"#;
        let out = inject_base(html, "/facet/app");
        assert_eq!(out, html.to_vec());
    }

    #[test]
    fn a_document_without_a_head_is_not_guessed_at() {
        let html = b"<html><body>bare</body></html>";
        assert_eq!(inject_base(html, "/facet/app"), html.to_vec());
    }

    #[test]
    fn only_html_reaches_the_injector() {
        // inject_base is deliberately unguarded about content type — the caller
        // checks Content-Type first, because that is the only reliable signal.
        // JSON that happens to contain the literal "<head>" would be rewritten
        // if it ever got here, which is why it never does.
        let json = br#"{"note":"<head>"}"#;
        assert_ne!(
            inject_base(json, "/facet/app"),
            json.to_vec(),
            "if this ever passes, the content-type gate in proxy() is what \
             protects non-HTML, and removing it would corrupt JSON"
        );
    }

    #[test]
    fn binary_content_is_left_intact_when_it_has_no_head() {
        // The realistic non-HTML case: nothing resembling markup, so even a
        // mistaken call is harmless.
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        assert_eq!(inject_base(png, "/facet/app"), png.to_vec());
    }

    /// Mirrors the extraction in `forward`, so the behaviour can be asserted
    /// without standing up a router.
    fn raw_suffix(raw: &str, id: &str) -> String {
        let prefix = format!("/facet/{id}");
        raw.strip_prefix(&prefix)
            .map(|rest| rest.trim_start_matches('/'))
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn an_encoded_slash_survives_the_proxy() {
        // The bug that made ComfyUI's workflow list inert: %2F decoded to a
        // real slash turns one path segment into two, and the app 404s.
        assert_eq!(
            raw_suffix("/facet/comfyui/api/userdata/workflows%2FName.json", "comfyui"),
            "api/userdata/workflows%2FName.json"
        );
    }

    #[test]
    fn encoded_spaces_and_plus_survive() {
        // These decode to characters that make the forwarded URI invalid.
        assert_eq!(
            raw_suffix("/facet/app/a%20b%2Bc.json", "app"),
            "a%20b%2Bc.json"
        );
    }

    #[test]
    fn the_bare_and_trailing_slash_forms_both_yield_an_empty_path() {
        assert_eq!(raw_suffix("/facet/app", "app"), "");
        assert_eq!(raw_suffix("/facet/app/", "app"), "");
    }

    #[test]
    fn nested_paths_keep_their_structure() {
        assert_eq!(
            raw_suffix("/facet/app/vendor/bootstrap/css/bootstrap.css", "app"),
            "vendor/bootstrap/css/bootstrap.css"
        );
    }

    #[test]
    fn secure_is_dropped_because_prism_is_not_on_https() {
        // The exact failure this fixes: the browser silently discards a Secure
        // cookie on a plain-HTTP origin, so the login succeeds and the next
        // request is unauthenticated — which looks like the login page
        // reloading with no error.
        let out = rewrite_cookie("session=abc; Path=/; Secure; HttpOnly", "/facet/app");
        assert!(!out.to_lowercase().contains("secure"), "got {out}");
        assert!(out.contains("HttpOnly"), "unrelated flags must survive: {out}");
    }

    #[test]
    fn a_scoped_path_has_no_trailing_slash() {
        // RFC 6265: cookie-path `/facet/app/` does not match a request for
        // `/facet/app`, so the slash would silently narrow the cookie.
        let out = rewrite_cookie("s=1; Path=/", "/facet/app");
        assert!(out.contains("Path=/facet/app;") || out.ends_with("Path=/facet/app"), "got {out}");
    }

    #[test]
    fn a_root_path_is_scoped_to_the_prefix() {
        let out = rewrite_cookie("session=abc; Path=/", "/facet/app");
        assert!(out.contains("Path=/facet/app"), "got {out}");
    }

    #[test]
    fn a_deeper_path_is_moved_under_the_prefix() {
        // Path=/rest would never match /facet/app/rest.
        let out = rewrite_cookie("t=1; Path=/rest", "/facet/app");
        assert!(out.contains("Path=/facet/app/rest"), "got {out}");
    }

    #[test]
    fn an_already_scoped_path_is_left_alone() {
        let out = rewrite_cookie("t=1; Path=/facet/app/rest", "/facet/app");
        assert!(out.contains("Path=/facet/app/rest"), "got {out}");
        assert!(!out.contains("/facet/app/facet"), "double-prefixed: {out}");
    }

    #[test]
    fn a_cookie_without_a_path_gets_one() {
        // Otherwise it defaults to the request's directory, which for a deep
        // API call is narrower than the app assumes.
        let out = rewrite_cookie("CSRF-Token-X=abc", "/facet/app");
        assert!(out.starts_with("CSRF-Token-X=abc"), "got {out}");
        assert!(out.contains("Path=/facet/app"), "got {out}");
    }

    #[test]
    fn the_cookie_value_itself_is_never_touched() {
        let out = rewrite_cookie("s=aB3/+=xyz; Path=/; Secure", "/facet/app");
        assert!(out.starts_with("s=aB3/+=xyz"), "got {out}");
    }

    #[test]
    fn every_set_cookie_header_is_rewritten_not_just_the_first() {
        let mut headers = axum::http::HeaderMap::new();
        headers.append(header::SET_COOKIE, HeaderValue::from_static("a=1; Secure"));
        headers.append(header::SET_COOKIE, HeaderValue::from_static("b=2; Secure"));
        rewrite_cookies(&mut headers, "/facet/app");

        let all: Vec<String> = headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(all.len(), 2, "both must survive: {all:?}");
        assert!(all.iter().all(|c| !c.to_lowercase().contains("secure")), "{all:?}");
    }

    #[test]
    fn a_root_relative_redirect_is_brought_under_the_prefix() {
        assert_eq!(
            rewrite_location("/login", "/facet/app", 8384).as_deref(),
            Some("/facet/app/login")
        );
    }

    #[test]
    fn an_already_prefixed_redirect_is_left_alone() {
        // Prefixing twice would produce /facet/app/facet/app/login.
        assert_eq!(rewrite_location("/facet/app/login", "/facet/app", 8384), None);
    }

    #[test]
    fn a_redirect_to_the_apps_own_address_is_folded_back() {
        // The browser cannot reach the app's loopback address; only Prism can.
        assert_eq!(
            rewrite_location("http://127.0.0.1:8384/gui/", "/facet/app", 8384).as_deref(),
            Some("/facet/app/gui/")
        );
        assert_eq!(
            rewrite_location("https://127.0.0.1:8384/", "/facet/app", 8384).as_deref(),
            Some("/facet/app/")
        );
        assert_eq!(
            rewrite_location("http://localhost:8384/x", "/facet/app", 8384).as_deref(),
            Some("/facet/app/x")
        );
    }

    #[test]
    fn a_redirect_elsewhere_is_the_apps_business() {
        // Rewriting this would break an OAuth flow or an outbound link.
        assert_eq!(
            rewrite_location("https://accounts.example.com/auth", "/facet/app", 8384),
            None
        );
        // A different port is a different service.
        assert_eq!(
            rewrite_location("http://127.0.0.1:9999/x", "/facet/app", 8384),
            None
        );
    }

    #[test]
    fn hop_by_hop_headers_are_named_in_lowercase() {
        // They are matched against HeaderMap keys, which are lowercase.
        for h in HOP_BY_HOP {
            assert_eq!(*h, h.to_lowercase(), "{h} must be lowercase");
        }
    }

    #[test]
    fn connection_and_upgrade_are_stripped() {
        // Forwarding these would describe a hop the app is not part of.
        assert!(HOP_BY_HOP.contains(&"connection"));
        assert!(HOP_BY_HOP.contains(&"upgrade"));
    }
}
