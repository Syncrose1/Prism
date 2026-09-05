//! Serving the embedded shell.
//!
//! `ui/` is compiled into the binary, so deploying Prism is copying one file.
//! There is no build step, no `node_modules`, and nothing fetched at first
//! paint — which matters because the tailnet may be the only route available
//! and the machine may already be struggling. See ADR 0001.

use axum::{
    body::Body,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../ui/"]
struct Assets;

/// The shell, at `/`.
pub async fn index() -> Response {
    serve("shell.html")
}

/// Anything else under `/ui/…`, chiefly the vendored terminal emulator.
pub async fn asset(Path(path): Path<String>) -> Response {
    serve(&path)
}

fn serve(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => {
            let mime = match path.rsplit_once('.').map(|(_, e)| e) {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "text/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("svg") => "image/svg+xml",
                Some("woff2") => "font/woff2",
                Some("png") => "image/png",
                _ => "application/octet-stream",
            };
            // The shell itself must not be cached: a redeployed binary should
            // take effect on reload rather than leaving the operator on a stale
            // build. Vendored assets are immutable and cached hard.
            let cache = if path.ends_with(".html") {
                "no-cache"
            } else {
                "public, max-age=604800, immutable"
            };
            Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, cache)
                .body(Body::from(file.data.into_owned()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_is_embedded() {
        assert!(
            Assets::get("shell.html").is_some(),
            "the shell must be compiled into the binary"
        );
    }

    #[test]
    fn the_terminal_emulator_is_vendored() {
        // Vendored rather than fetched: the CSP forbids a CDN, and the tailnet
        // may be the only route available.
        let js = Assets::get("vendor/xterm.js").expect("xterm.js embedded");
        assert!(js.data.len() > 100_000, "that is not the real xterm.js");
        assert!(Assets::get("vendor/xterm.css").is_some());
        assert!(Assets::get("vendor/xterm-addon-fit.js").is_some());
    }

    #[test]
    fn unknown_assets_are_not_found() {
        assert_eq!(serve("nope.js").status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn the_shell_is_served_uncached() {
        let r = serve("shell.html");
        assert_eq!(r.headers().get(header::CACHE_CONTROL).unwrap(), "no-cache");
    }
}
