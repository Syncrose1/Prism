//! The Prism daemon.
//!
//! Two independent halves share one process:
//!
//! * a **synchronous monitor thread** that senses and intervenes, and
//! * an **async HTTP server** exposing the tailnet API.
//!
//! The monitor is deliberately not async. It has to keep working during the
//! memory exhaustion it exists to resolve, and at that moment an executor
//! competing for the same starved runtime is a liability. It runs on its own
//! thread, publishes readings through a lock, and never awaits anything.

mod action;
mod api;
mod bind;
mod monitor;
mod enrol;
mod facets_api;
mod files_api;
mod rescue;
mod term_api;
mod ui;

use anyhow::Context as _;
use prism_core::auth::{AuthPolicy, Authenticator, session::SessionKey};
use prism_core::config::{self, HostConfig, Profile};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

fn main() -> anyhow::Result<()> {
    // Subcommands run and exit; only the bare invocation starts the daemon.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("enrol") | Some("enroll") => {
            let reset = args.iter().any(|a| a == "--reset");
            return enrol::command(&config::state_dir(), reset);
        }
        Some("--help") | Some("-h") => {
            println!("prismd — the Prism daemon\n");
            println!("  prismd                 run the daemon");
            println!("  prismd enrol           show enrolment status");
            println!("  prismd enrol --reset   revoke and replace the authenticator secret");
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown command `{other}`; try --help"),
        None => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PRISM_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let dir = config::config_dir();
    let state_dir = config::state_dir();
    let host: HostConfig =
        config::load_or_default(&dir.join("prism.toml")).context("loading host config")?;
    let profile: Profile =
        config::load_or_default(&dir.join("profile.toml")).context("loading profile")?;

    info!(
        profile = %profile.name,
        config = %dir.display(),
        state = %state_dir.display(),
        "prism starting"
    );

    lock_memory();

    let secret = enrol::load_or_enrol(&state_dir)?;
    let session_key = SessionKey::load_or_create(&state_dir.join("session.key"))
        .context("loading session key")?;
    let auth = Arc::new(Authenticator::new(
        session_key,
        secret,
        AuthPolicy::default(),
    ));

    let vitals: api::SharedVitals = Arc::new(RwLock::new(api::Vitals::default()));
    let facets = Arc::new(profile.facet.clone());
    let terminals = Arc::new(prism_core::term::session::SessionManager::new(
        term_api::manager_from(&host.terminal),
    ));

    // Roots are canonicalised once at startup, so every later containment check
    // compares two already-real paths. A configured root that does not exist is
    // dropped with a warning rather than aborting the daemon.
    let roots: Vec<prism_core::files::path::Root> = host
        .files
        .roots
        .iter()
        .filter_map(|r| {
            match prism_core::files::path::Root::new(&r.name, &r.path, r.writable) {
                Some(root) => {
                    info!(name = %root.name, path = %root.path.display(), writable = root.writable, "file root");
                    Some(root)
                }
                None => {
                    warn!(name = %r.name, path = %r.path.display(), "file root does not exist; skipping");
                    None
                }
            }
        })
        .collect();
    if roots.is_empty() {
        info!("no file roots configured; the Files app is disabled");
    }
    let roots = Arc::new(roots);
    let thumb_dir = Arc::new(state_dir.join("thumbs"));
    if host.terminal.enabled {
        info!(
            max_sessions = host.terminal.max_sessions,
            scoped = host.terminal.use_scope,
            "terminal sessions enabled"
        );
    } else {
        info!("terminal sessions disabled by configuration");
    }

    // The monitor owns its own thread so that neither half can stall the other.
    let monitor_vitals = Arc::clone(&vitals);
    let monitor_terms = Arc::clone(&terminals);
    std::thread::Builder::new()
        .name("prism-monitor".into())
        .spawn(move || {
            let mut monitor = monitor::Monitor::new(profile, monitor_vitals, monitor_terms);
            if let Err(e) = monitor.run() {
                tracing::error!(error = %e, "monitor loop exited");
            }
        })
        .context("spawning monitor thread")?;

    serve(host, auth, vitals, facets, terminals, roots, thumb_dir)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn serve(
    host: HostConfig,
    auth: Arc<Authenticator>,
    vitals: api::SharedVitals,
    facets: Arc<Vec<prism_core::config::Facet>>,
    terminals: Arc<prism_core::term::session::SessionManager>,
    roots: Arc<Vec<prism_core::files::path::Root>>,
    thumb_dir: Arc<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let addr = bind::resolve(&host.server.bind, host.server.port)?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    info!(%addr, "prism os listening at http://{addr}/");
    let app = api::router(api::AppState {
        auth,
        vitals,
        facets,
        terminals,
        roots,
        thumb_dir,
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving api")
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown requested");
}

/// Pin the daemon's pages so it cannot be swapped out.
///
/// Without this, Prism is among the first things evicted under the pressure it
/// is supposed to resolve — paged out precisely when it needs to act, which is
/// how a watchdog becomes another casualty rather than a rescue.
///
/// Failure is not fatal: the daemon is still useful unlocked, and on a host with
/// a restrictive `RLIMIT_MEMLOCK` refusing to start would be worse than running
/// slightly less reliably.
fn lock_memory() {
    // SAFETY: `mlockall` takes only a flags bitmask and touches no caller
    // memory. The call is total; it either succeeds or reports errno.
    let rc = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
    if rc == 0 {
        info!("memory locked (mlockall): daemon is unswappable");
    } else {
        let err = std::io::Error::last_os_error();
        warn!(
            %err,
            "could not lock memory; prism may be paged out under pressure. \
             Raise LimitMEMLOCK in the service unit to fix."
        );
    }
}
