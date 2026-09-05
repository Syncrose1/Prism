//! Facet control.
//!
//! Starting a facet runs an operator-configured command, which is close enough
//! to arbitrary execution that it sits at [`Sensitivity::Fresh`] alongside files
//! and terminals. Reading their state is only `Session` — glancing at whether
//! ComfyUI is up should not demand a code.
//!
//! Stopping prefers `cgroup.kill`: atomic across the whole tree, so a workload
//! that forked CUDA workers dies in one operation with nothing orphaned. That is
//! also why facets exist rather than Prism tracking pids.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use prism_core::auth::Sensitivity;
use prism_core::config::{Facet, FacetLimits};
use prism_core::supervisor::{FacetStatus, Supervisor};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::api::{AppState, require};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/facets", get(list).post(create))
        .route("/api/facets/{id}", axum::routing::delete(remove))
        .route("/api/facets/{id}/start", post(start))
        .route("/api/facets/{id}/stop", post(stop))
        .route("/api/facets/{id}/kill", post(kill))
        .route("/api/facets/{id}/limits", post(set_limits))
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    detail: String,
}

fn err(status: StatusCode, error: &'static str, detail: impl Into<String>) -> Response {
    (status, Json(ErrorBody { error, detail: detail.into() })).into_response()
}

fn find(state: &AppState, id: &str) -> Option<Facet> {
    state.facets.read().expect("facets poisoned").iter().find(|f| f.id == id).cloned()
}

/// Persist the facet list back to the profile the daemon loaded.
///
/// Rewriting the operator's own `profile.toml` keeps one source of truth:
/// a facet added from the UI is a facet they can also read, edit and copy to
/// another machine, rather than living in a database only Prism understands.
fn persist(state: &AppState) -> anyhow::Result<()> {
    let facets = state.facets.read().expect("facets poisoned").clone();
    let mut profile: prism_core::config::Profile =
        prism_core::config::load_or_default(&state.profile_path)?;
    profile.facet = facets;
    prism_core::config::save(&state.profile_path, &profile)
}

#[derive(Serialize)]
struct FacetView {
    id: String,
    name: String,
    /// "running" | "stopped" | "failed"
    state: &'static str,
    detail: Option<String>,
    command: String,
    memory_mib: Option<u64>,
    swap_mib: Option<u64>,
    limits: LimitsView,
    /// True when starting this facet opens a Terminal window rather than
    /// running headless.
    pty: bool,
    /// False when the facet's capability gate is unmet on this host — a profile
    /// authored elsewhere may name workloads that do not exist here.
    available: bool,
    unavailable_because: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct LimitsView {
    memory_high: Option<String>,
    memory_max: Option<String>,
    swap_max: Option<String>,
}

impl From<&FacetLimits> for LimitsView {
    fn from(l: &FacetLimits) -> Self {
        Self {
            memory_high: l.memory_high.clone(),
            memory_max: l.memory_max.clone(),
            swap_max: l.swap_max.clone(),
        }
    }
}

fn view(facet: &Facet, sup: &Supervisor) -> FacetView {
    let status = sup.status(&facet.id);
    let (state, detail) = match &status {
        FacetStatus::Running => ("running", None),
        FacetStatus::Stopped => ("stopped", None),
        FacetStatus::Failed(why) => ("failed", Some(why.clone())),
    };
    let running = matches!(status, FacetStatus::Running);
    let gate = facet.enabled_if.evaluate();

    FacetView {
        id: facet.id.clone(),
        name: facet.name.clone(),
        state,
        detail,
        command: facet.command.join(" "),
        // Only meaningful while running; a stopped facet has no cgroup.
        memory_mib: running.then(|| sup.memory_current_kb(&facet.id).map(|kb| kb / 1024)).flatten(),
        swap_mib: running.then(|| sup.memory_swap_kb(&facet.id).map(|kb| kb / 1024)).flatten(),
        limits: LimitsView::from(&facet.limits),
        pty: facet.pty,
        available: gate.is_satisfied(),
        unavailable_because: match gate {
            prism_core::gate::GateOutcome::Blocked(why) => Some(why),
            _ => None,
        },
    }
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Reading state is not a privileged action; acting on it is.
    if let Some(d) = require(&state, &headers, Sensitivity::Session) {
        return d;
    }
    let sup = Supervisor::new();
    let facets: Vec<FacetView> = state
        .facets
        .read()
        .expect("facets poisoned")
        .iter()
        .map(|f| view(f, &sup))
        .collect();
    Json(facets).into_response()
}

#[derive(Deserialize)]
struct CreateRequest {
    name: String,
    /// The command line, as typed. Split with shell-like quoting.
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    pty: bool,
    #[serde(default)]
    limits: LimitsView,
}

/// Split a command line on whitespace, honouring single and double quotes.
///
/// Deliberately not a shell: no globbing, no substitution, no pipes. The
/// operator asked to add scripts, and a script path with a space in it should
/// work — but a facet definition is not a place to smuggle `; rm -rf`.
pub fn split_command(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;

    for ch in input.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => cur.push(ch),
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                any = true;
            }
            None if ch.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            None => cur.push(ch),
        }
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    out
}

/// Derive a stable, filesystem- and unit-safe id from a name.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    // Collapse runs, so "My  Script!!" does not become "my--script--".
    let mut out = String::new();
    let mut last_dash = false;
    for c in s.chars() {
        if c == '-' {
            if !last_dash {
                out.push(c);
            }
            last_dash = true;
        } else {
            out.push(c);
            last_dash = false;
        }
    }
    if out.is_empty() { "facet".into() } else { out }
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateRequest>,
) -> Response {
    // Adding a facet defines a command Prism will later run, so it is as
    // sensitive as running one.
    if let Some(d) = require(&state, &headers, Sensitivity::Fresh) {
        return d;
    }

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "no_name", "a name is required");
    }
    let command = split_command(&body.command);
    if command.is_empty() {
        return err(StatusCode::BAD_REQUEST, "no_command", "a command is required");
    }

    let mut id = slug(&name);
    {
        let facets = state.facets.read().expect("facets poisoned");
        if facets.iter().any(|f| f.id == id) {
            // Names collide; disambiguate rather than refusing.
            let mut n = 2;
            while facets.iter().any(|f| f.id == format!("{id}-{n}")) {
                n += 1;
            }
            id = format!("{id}-{n}");
        }
    }

    let facet = Facet {
        id: id.clone(),
        name,
        command,
        cwd: body.cwd.filter(|c| !c.trim().is_empty()).map(std::path::PathBuf::from),
        limits: FacetLimits {
            memory_high: body.limits.memory_high,
            memory_max: body.limits.memory_max,
            swap_max: body.limits.swap_max,
        },
        enabled_if: Default::default(),
        pty: body.pty,
    };

    state.facets.write().expect("facets poisoned").push(facet.clone());
    if let Err(e) = persist(&state) {
        // Roll back rather than leaving memory and disk disagreeing.
        state.facets.write().expect("facets poisoned").retain(|f| f.id != id);
        warn!(error = %e, "could not persist new facet");
        return err(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string());
    }

    info!(facet = %id, "facet added");
    Json(view(&facet, &Supervisor::new())).into_response()
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(d) = require(&state, &headers, Sensitivity::Fresh) {
        return d;
    }
    let Some(removed) = find(&state, &id) else {
        return err(StatusCode::NOT_FOUND, "no_facet", "no such facet");
    };
    // A running workload must be stopped deliberately, not removed out from
    // under itself leaving an orphaned scope nothing knows about.
    if matches!(Supervisor::new().status(&id), FacetStatus::Running) {
        return err(StatusCode::CONFLICT, "still_running", "stop it first");
    }

    state.facets.write().expect("facets poisoned").retain(|f| f.id != id);
    if let Err(e) = persist(&state) {
        state.facets.write().expect("facets poisoned").push(removed);
        return err(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string());
    }
    info!(facet = %id, "facet removed");
    StatusCode::NO_CONTENT.into_response()
}

async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(d) = require(&state, &headers, Sensitivity::Fresh) {
        return d;
    }
    let Some(facet) = find(&state, &id) else {
        return err(StatusCode::NOT_FOUND, "no_facet", "no such facet");
    };

    if let prism_core::gate::GateOutcome::Blocked(why) = facet.enabled_if.evaluate() {
        return err(StatusCode::PRECONDITION_FAILED, "unavailable", why);
    }

    // Starting a heavy workload on a machine that is already failing is how the
    // operator lost three weekends. Refuse, and say why.
    let tier = state.vitals.read().expect("vitals poisoned").tier.clone();
    if matches!(tier.as_str(), "red" | "black") {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "degraded",
            "the machine is under memory pressure; refusing to start a workload",
        );
    }

    // An interactive launcher becomes a terminal session rather than a headless
    // scope, so its prompts can actually be answered. Same containment either
    // way — a session is a scope too.
    if facet.pty {
        let title = facet.name.clone();
        let cwd = facet.cwd.as_ref().map(|p| p.display().to_string());
        return match state.terminals.create(
            &facet.command,
            cwd.as_deref(),
            prism_core::term::pty::WinSize { rows: 30, cols: 100 },
            &title,
        ) {
            Ok(session) => {
                info!(facet = %id, session = %session.id, "facet started with a pty");
                Json(serde_json::json!({
                    "pty": true,
                    "session": session.info(),
                }))
                .into_response()
            }
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "start_failed", e.to_string()),
        };
    }

    let sup = Supervisor::new();
    if matches!(sup.status(&id), FacetStatus::Running) {
        return err(StatusCode::CONFLICT, "already_running", "already running");
    }

    match sup.start(&facet) {
        Ok(()) => {
            info!(facet = %id, "facet started");
            Json(view(&facet, &sup)).into_response()
        }
        Err(e) => {
            warn!(facet = %id, error = %e, "facet start failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "start_failed", e.to_string())
        }
    }
}

/// Ask a facet to stop, letting it run its own shutdown.
async fn stop(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    if let Some(d) = require(&state, &headers, Sensitivity::Fresh) {
        return d;
    }
    if find(&state, &id).is_none() {
        return err(StatusCode::NOT_FOUND, "no_facet", "no such facet");
    }
    match Supervisor::new().stop(&id) {
        Ok(()) => {
            info!(facet = %id, "facet stopped");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "stop_failed", e.to_string()),
    }
}

/// Terminate the whole tree immediately, for a workload that will not stop.
async fn kill(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    if let Some(d) = require(&state, &headers, Sensitivity::Fresh) {
        return d;
    }
    if find(&state, &id).is_none() {
        return err(StatusCode::NOT_FOUND, "no_facet", "no such facet");
    }
    match Supervisor::new().kill(&id) {
        Ok(()) => {
            info!(facet = %id, "facet killed via cgroup.kill");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::CONFLICT, "kill_failed", e.to_string()),
    }
}

/// Adjust limits on a *running* facet, without restarting it.
///
/// This is the "drag a slider from London" requirement: the operator can throttle
/// a workload that is misbehaving rather than having to kill and relaunch it.
async fn set_limits(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<LimitsView>,
) -> Response {
    if let Some(d) = require(&state, &headers, Sensitivity::Fresh) {
        return d;
    }
    if find(&state, &id).is_none() {
        return err(StatusCode::NOT_FOUND, "no_facet", "no such facet");
    }
    let limits = FacetLimits {
        memory_high: body.memory_high,
        memory_max: body.memory_max,
        swap_max: body.swap_max,
    };
    match Supervisor::new().set_limits(&id, &limits) {
        Ok(()) => {
            info!(facet = %id, "facet limits adjusted");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::CONFLICT, "limits_failed", e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::gate::Gate;

    fn facet(id: &str) -> Facet {
        Facet {
            id: id.into(),
            name: "Test".into(),
            command: vec!["sleep".into(), "60".into()],
            cwd: None,
            limits: FacetLimits::default(),
            enabled_if: Gate::default(),
            pty: false,
        }
    }

    #[test]
    fn a_stopped_facet_reports_no_memory() {
        // A stopped facet has no cgroup; reporting 0 would look like a running
        // workload using nothing.
        let v = view(&facet("nonexistent-xyzzy"), &Supervisor::new());
        assert_eq!(v.state, "stopped");
        assert!(v.memory_mib.is_none());
        assert!(v.swap_mib.is_none());
    }

    #[test]
    fn an_unmet_gate_marks_the_facet_unavailable_with_a_reason() {
        let mut f = facet("gated");
        f.enabled_if = Gate {
            binary: Some("definitely-not-real-xyzzy".into()),
            ..Default::default()
        };
        let v = view(&f, &Supervisor::new());
        assert!(!v.available);
        assert!(v.unavailable_because.unwrap().contains("not on PATH"));
    }

    #[test]
    fn an_ungated_facet_is_available() {
        assert!(view(&facet("plain"), &Supervisor::new()).available);
    }

    #[test]
    fn limits_round_trip_through_the_view() {
        let l = FacetLimits {
            memory_high: Some("22G".into()),
            memory_max: None,
            swap_max: Some("6G".into()),
        };
        let v = LimitsView::from(&l);
        assert_eq!(v.memory_high.as_deref(), Some("22G"));
        assert_eq!(v.memory_max, None);
        assert_eq!(v.swap_max.as_deref(), Some("6G"));
    }

    #[test]
    fn command_splitting_handles_quoted_paths() {
        // A script path with a space in it is the whole reason this exists.
        assert_eq!(split_command("./run.sh"), vec!["./run.sh"]);
        assert_eq!(split_command("python  -m  comfy"), vec!["python", "-m", "comfy"]);
        assert_eq!(
            split_command(r#""/home/a b/run.sh" --flag"#),
            vec!["/home/a b/run.sh", "--flag"]
        );
        assert_eq!(split_command("'single quoted arg'"), vec!["single quoted arg"]);
    }

    #[test]
    fn command_splitting_is_not_a_shell() {
        // No globbing, no substitution, no operators — a facet definition is
        // not a place to smuggle a second command.
        assert_eq!(
            split_command("echo hi; rm -rf /"),
            vec!["echo", "hi;", "rm", "-rf", "/"]
        );
        assert_eq!(split_command("echo $HOME"), vec!["echo", "$HOME"]);
    }

    #[test]
    fn empty_command_yields_nothing_to_run() {
        assert!(split_command("").is_empty());
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn an_empty_quoted_argument_survives() {
        assert_eq!(split_command(r#"cmd "" x"#), vec!["cmd", "", "x"]);
    }

    #[test]
    fn slugs_are_stable_and_unit_safe() {
        assert_eq!(slug("ComfyUI"), "comfyui");
        assert_eq!(slug("My  Script!!"), "my-script");
        assert_eq!(slug("llama.cpp server"), "llama-cpp-server");
        assert_eq!(slug("!!!"), "facet");
        assert_eq!(slug(""), "facet");
    }

    #[test]
    fn a_pty_facet_is_marked_as_such_so_the_ui_can_open_a_terminal() {
        let mut f = facet("interactive");
        f.pty = true;
        assert!(view(&f, &Supervisor::new()).pty);
        assert!(!view(&facet("headless"), &Supervisor::new()).pty);
    }

    #[test]
    fn the_command_is_shown_so_the_operator_knows_what_will_run() {
        assert_eq!(view(&facet("x"), &Supervisor::new()).command, "sleep 60");
    }
}
