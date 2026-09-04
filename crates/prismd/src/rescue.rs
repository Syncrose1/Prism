//! Critical Functions Mode.
//!
//! A single self-contained HTML page with **no JavaScript, no fetches, no
//! external assets**, served unconditionally at `/rescue` regardless of tier.
//! See ADR 0002.
//!
//! Three properties are load-bearing and should not be traded away:
//!
//! * **No shared code path with the rich shell.** A shell that fails to load
//!   must not be able to take this page with it.
//! * **No tier gate.** Automatic de-escalation is a convenience; the failure
//!   that cannot be anticipated is the one where the *detector* is wrong. This
//!   page is reachable when Prism believes everything is fine, because Prism's
//!   belief is exactly what is in question.
//! * **No JavaScript.** It has to render on a browser that is itself starved,
//!   on an old phone, on a bad connection — and in `w3m` over SSH, which during
//!   the 2026-09-04 incidents was the only surviving access path.
//!
//! Every control is a form `POST`. The session cookie is `SameSite=Strict`,
//! which is what makes these forms safe from cross-site submission without any
//! token plumbing.

use axum::{
    Form, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use prism_core::auth::Sensitivity;
use prism_core::safety::SafetyGuard;
use prism_core::sensors::{disk, memory, process};
use prism_core::supervisor::Supervisor;
use serde::Deserialize;
use std::fmt::Write as _;
use tracing::{info, warn};

use crate::api::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/rescue", get(page))
        .route("/rescue/login", post(login))
        .route("/rescue/kill", post(kill))
        .route("/rescue/facet/stop", post(stop_facet))
        .route("/rescue/remedy", post(remedy))
}

/// Shared page chrome. Duplicated rather than imported from the shell on
/// purpose: this page must not break when the shell does.
const HEAD: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Prism — Critical Functions</title>
<style>
:root{color-scheme:dark light}
body{margin:0;padding:1.25rem;font:15px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;
background:#0b0d10;color:#dfe4ea;max-width:60rem}
h1{font-size:1.1rem;margin:0 0 .25rem;letter-spacing:.08em;text-transform:uppercase}
h2{font-size:.85rem;margin:1.75rem 0 .5rem;letter-spacing:.1em;text-transform:uppercase;color:#8b95a3}
.sub{color:#8b95a3;margin:0 0 1.25rem;font-size:.85rem}
table{border-collapse:collapse;width:100%;font-size:.85rem}
td,th{text-align:left;padding:.35rem .5rem;border-bottom:1px solid #1d222b}
th{color:#8b95a3;font-weight:400}
td.n{text-align:right;font-variant-numeric:tabular-nums}
.cmd{color:#8b95a3;overflow-wrap:anywhere}
.tier{display:inline-block;padding:.15rem .6rem;border-radius:.2rem;font-weight:600}
.green{background:#12301c;color:#6ee7a0}
.amber{background:#3a2c0c;color:#f0c05a}
.red{background:#3d1414;color:#ff8a8a}
.black{background:#450a0a;color:#fff;outline:1px solid #ff5a5a}
button{font:inherit;font-size:.8rem;padding:.2rem .6rem;border:1px solid #3a4250;
background:#171b22;color:#dfe4ea;border-radius:.2rem;cursor:pointer}
button:hover{background:#222831;border-color:#5a6475}
input{font:inherit;padding:.4rem .6rem;background:#12151a;color:#dfe4ea;
border:1px solid #3a4250;border-radius:.2rem;letter-spacing:.3em;width:8em}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(11rem,1fr));gap:.75rem;margin:1rem 0}
.stat{background:#12151a;border:1px solid #1d222b;border-radius:.3rem;padding:.6rem .75rem}
.stat .k{color:#8b95a3;font-size:.7rem;letter-spacing:.08em;text-transform:uppercase}
.stat .v{font-size:1.25rem;font-variant-numeric:tabular-nums;margin-top:.15rem}
.note{color:#8b95a3;font-size:.8rem;margin-top:2rem;border-top:1px solid #1d222b;padding-top:.75rem}
form{display:inline}
</style></head><body>"#;

fn html_response(body: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

/// The zero-JS sign-in form.
///
/// Rescue carries its own login rather than reusing the API's JSON endpoint, so
/// that reaching a recoverable machine never depends on the rich shell's bundle,
/// its router, or JavaScript being available at all.
fn login_page(message: Option<&str>) -> Response {
    let mut html = String::from(HEAD);
    html.push_str(
        r#"<h1>Prism — Critical Functions</h1>
<p class="sub">Enter the current code from your authenticator.</p>"#,
    );
    if let Some(msg) = message {
        let _ = write!(html, r#"<p class="tier red">{}</p>"#, esc(msg));
    }
    html.push_str(
        r#"<form method="post" action="/rescue/login">
<input type="text" name="code" inputmode="numeric" pattern="[0-9]*" maxlength="6"
 autocomplete="one-time-code" autofocus placeholder="000000">
<button type="submit">Sign in</button></form>
<p class="note">This page needs no JavaScript and works in a text browser.</p>
</body></html>"#,
    );
    html_response(html)
}

#[derive(Deserialize)]
struct LoginForm {
    code: String,
}

async fn login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    let now = prism_core::auth::totp::now_unix();
    match state.auth.submit_code(&form.code, now) {
        (prism_core::auth::CodeOutcome::Accepted, Some(token)) => {
            info!("rescue: signed in");
            let cookie = format!(
                "prism_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
                state.auth.policy().session_ttl_secs
            );
            ([(header::SET_COOKIE, cookie)], Redirect::to("/rescue")).into_response()
        }
        (prism_core::auth::CodeOutcome::Replayed, _) => {
            login_page(Some("that code has already been used — wait for the next one"))
        }
        (prism_core::auth::CodeOutcome::LockedOut { retry_after_secs }, _) => login_page(Some(
            &format!("too many attempts; retry in {retry_after_secs}s"),
        )),
        _ => login_page(Some("incorrect code")),
    }
}

/// Minimal escaping for anything interpolated into the page.
///
/// Process command lines are attacker-influenceable — any user on the machine
/// can create a process named `<script>` — so nothing reaches the document
/// without passing through here.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn gib(mib: u64) -> String {
    format!("{:.1} GiB", mib as f64 / 1024.0)
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

/// Session tier, not Fresh.
///
/// Demanding a fresh authenticator code from someone whose machine is dying is
/// hostile, and the page is already tailnet-bound. Session is the balance: an
/// arbitrary device on the tailnet cannot kill processes, but the operator's
/// phone — already signed in — just works.
const RESCUE_TIER: Sensitivity = Sensitivity::Session;

/// True if the request may act. Rendered as a login form rather than JSON,
/// because the caller here is a browser, possibly a very unhappy one.
fn signed_in(state: &AppState, headers: &HeaderMap) -> bool {
    crate::api::require(state, headers, RESCUE_TIER).is_none()
}

async fn page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !signed_in(&state, &headers) {
        return login_page(None);
    }

    let vitals = state.vitals.read().expect("vitals lock poisoned").clone();
    let mut html = String::with_capacity(16 * 1024);
    html.push_str(HEAD);

    let tier = vitals.tier.as_str();
    let _ = write!(
        html,
        r#"<h1>Prism — Critical Functions</h1>
<p class="sub">Minimal recovery interface. No JavaScript. Always available at <code>/rescue</code>.</p>
<p>State: <span class="tier {t}">{t}</span></p>"#,
        t = esc(tier)
    );

    // --- vitals -----------------------------------------------------------
    html.push_str(r#"<div class="grid">"#);
    let _ = write!(
        html,
        r#"<div class="stat"><div class="k">Honest headroom</div><div class="v">{}</div></div>"#,
        gib(vitals.honest_headroom_mib)
    );
    let _ = write!(
        html,
        r#"<div class="stat"><div class="k">Phantom</div><div class="v">{}</div></div>"#,
        gib(vitals.phantom_headroom_mib)
    );
    let _ = write!(
        html,
        r#"<div class="stat"><div class="k">Memory stall</div><div class="v">{:.1}%</div></div>"#,
        vitals.stall_full * 100.0
    );
    if let Some(d) = &vitals.disk {
        let _ = write!(
            html,
            r#"<div class="stat"><div class="k">Disk free — {}</div><div class="v">{}</div></div>"#,
            esc(&d.path),
            gib(d.free_mib)
        );
    }
    html.push_str("</div>");

    // --- facets -----------------------------------------------------------
    let supervisor = Supervisor::new();
    if !state.facets.is_empty() {
        html.push_str("<h2>Facets</h2><table><tr><th>Facet</th><th>State</th><th class=\"n\">Memory</th><th></th></tr>");
        for facet in state.facets.iter() {
            let status = supervisor.status(&facet.id);
            let mem = supervisor
                .memory_current_kb(&facet.id)
                .map(|kb| gib(kb / 1024))
                .unwrap_or_else(|| "—".into());
            let _ = write!(
                html,
                r#"<tr><td>{}</td><td>{:?}</td><td class="n">{}</td><td>
<form method="post" action="/rescue/facet/stop"><input type="hidden" name="id" value="{}">
<button type="submit">Stop</button></form></td></tr>"#,
                esc(&facet.name),
                status,
                mem,
                esc(&facet.id)
            );
        }
        html.push_str("</table>");
    }

    // --- top consumers ----------------------------------------------------
    html.push_str("<h2>Largest processes</h2><table><tr><th class=\"n\">PID</th><th>Name</th><th class=\"n\">RSS</th><th></th></tr>");
    // Consult the same guard that would refuse the signal, so a process Prism
    // will not kill is shown as protected rather than offered as a button that
    // silently does nothing. The guard is the authority in both places.
    let guard = SafetyGuard::default();
    for p in process::top_by_rss(12) {
        let action = match guard.check(p.pid) {
            Ok(_) => format!(
                r#"<form method="post" action="/rescue/kill"><input type="hidden" name="pid" value="{}">
<button type="submit">Kill</button></form>"#,
                p.pid
            ),
            Err(refusal) => format!(
                r#"<span class="cmd" title="{}">protected</span>"#,
                esc(&refusal.reason())
            ),
        };
        let _ = write!(
            html,
            r#"<tr><td class="n">{}</td><td>{}<br><span class="cmd">{}</span></td><td class="n">{}</td><td>{}</td></tr>"#,
            p.pid,
            esc(&p.comm),
            esc(&truncate(&p.cmdline, 90)),
            gib(p.rss_kb / 1024),
            action
        );
    }
    html.push_str("</table>");

    // --- remedies ---------------------------------------------------------
    html.push_str("<h2>Remedies</h2>");
    html.push_str(
        r#"<form method="post" action="/rescue/remedy">
<input type="hidden" name="action" value="restart_shell">
<button type="submit">Restart desktop shell (quickshell)</button></form>"#,
    );

    // --- live figures, read fresh rather than from the published snapshot ---
    // If the monitor thread has died, the cached vitals above go stale silently.
    // Reading /proc directly here means the page still tells the truth.
    if let Ok(live) = memory::sample() {
        let disks = disk::sample(&disk::default_paths());
        let _ = write!(
            html,
            r#"<p class="note">Read live at request time: {} available, {} honest headroom{}.
Cached figures above come from the monitor thread; if they disagree with these, the monitor has stopped.</p>"#,
            gib(live.available_kb / 1024),
            gib(live.honest_headroom_kb / 1024),
            disk::tightest(&disks)
                .map(|d| format!(", {} free on {}", gib(d.available_mib()), d.path.display()))
                .unwrap_or_default()
        );
    }

    html.push_str("</body></html>");

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct KillForm {
    pid: u32,
}

async fn kill(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<KillForm>,
) -> Response {
    if !signed_in(&state, &headers) {
        return login_page(None);
    }
    // Goes through the same SafetyGuard as every other signal: the rescue page
    // cannot be used to kill init, sshd, tailscaled or the compositor, however
    // panicked the operator is. Those refusals are logged, not silent.
    let gone = crate::action::terminate(&[form.pid], std::time::Duration::from_secs(3));
    if gone.is_empty() {
        warn!(pid = form.pid, "rescue: kill did not take effect");
    } else {
        info!(pid = form.pid, "rescue: process terminated");
    }
    Redirect::to("/rescue").into_response()
}

#[derive(Deserialize)]
struct FacetForm {
    id: String,
}

async fn stop_facet(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<FacetForm>,
) -> Response {
    if !signed_in(&state, &headers) {
        return login_page(None);
    }
    // cgroup.kill: atomic across the whole tree, and cannot escape the cgroup.
    match Supervisor::new().kill(&form.id) {
        Ok(()) => info!(facet = %form.id, "rescue: facet killed"),
        Err(e) => warn!(facet = %form.id, error = %e, "rescue: facet kill failed"),
    }
    Redirect::to("/rescue").into_response()
}

#[derive(Deserialize)]
struct RemedyForm {
    action: String,
}

async fn remedy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RemedyForm>,
) -> Response {
    if !signed_in(&state, &headers) {
        return login_page(None);
    }
    match form.action.as_str() {
        "restart_shell" => {
            // The known-good remedy from architecture.md §1.2, issued through
            // the compositor so the replacement is session-owned rather than
            // dying with whatever spawned it.
            crate::action::spawn_detached(&[
                "hyprctl".into(),
                "dispatch".into(),
                "exec".into(),
                "killall ydotool qs quickshell; qs -c ii".into(),
            ]);
            info!("rescue: desktop shell restart dispatched");
        }
        other => warn!(action = %other, "rescue: unknown remedy"),
    }
    Redirect::to("/rescue").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_metacharacters() {
        assert_eq!(esc("<script>"), "&lt;script&gt;");
        assert_eq!(esc("a&b"), "a&amp;b");
        assert_eq!(esc("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(esc("it's"), "it&#39;s");
    }

    #[test]
    fn escapes_a_hostile_process_name() {
        // Any local user can name a process this; it must not reach the DOM.
        let hostile = r#"<img src=x onerror="alert(1)">"#;
        let escaped = esc(hostile);
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('>'));
        assert!(!escaped.contains('"'));
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(esc("/usr/bin/python3 -m comfy"), "/usr/bin/python3 -m comfy");
    }

    #[test]
    fn truncate_preserves_short_strings() {
        assert_eq!(truncate("short", 90), "short");
    }

    #[test]
    fn truncate_is_char_safe_on_multibyte() {
        // Byte slicing here would panic mid-codepoint.
        let s = "日本語".repeat(50);
        let t = truncate(&s, 10);
        assert_eq!(t.chars().count(), 11); // 10 + ellipsis
    }

    #[test]
    fn gib_formats_from_mib() {
        assert_eq!(gib(1024), "1.0 GiB");
        assert_eq!(gib(1536), "1.5 GiB");
        assert_eq!(gib(0), "0.0 GiB");
    }
}
