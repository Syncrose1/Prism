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
        .route("/api/facets", get(list))
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

fn find<'a>(state: &'a AppState, id: &str) -> Option<&'a Facet> {
    state.facets.iter().find(|f| f.id == id)
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
    let facets: Vec<FacetView> = state.facets.iter().map(|f| view(f, &sup)).collect();
    Json(facets).into_response()
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

    match sup.start(facet) {
        Ok(()) => {
            info!(facet = %id, "facet started");
            Json(view(facet, &sup)).into_response()
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
