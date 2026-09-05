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
mod setup;
mod term_api;
mod ui;
mod workspace;

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
        Some("setup") => {
            return setup::command(&config::config_dir());
        }
        Some("passwd") | Some("password") => {
            return enrol::passwd(&config::state_dir());
        }
        Some("--help") | Some("-h") => {
            println!("prismd — the Prism daemon\n");
            println!("  prismd                 run the daemon");
            println!("  prismd enrol           show enrolment status");
            println!("  prismd enrol --reset   revoke and replace the authenticator secret");
            println!("  prismd passwd          set the quick-unlock password");
            println!("  prismd setup           detect this machine and write config");
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

    // First run: write a configuration that reflects this machine, rather than
    // starting with no file roots and no workloads and leaving the operator to
    // discover why everything is empty. Never overwrites.
    match setup::write_if_absent(&dir) {
        Ok(true) => info!(config = %dir.display(), "first run: wrote a detected configuration"),
        Ok(false) => {}
        Err(e) => warn!(error = %e, "could not write initial configuration"),
    }
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
    let password_hash = enrol::load_password_hash(&state_dir);
    if password_hash.is_none() {
        info!("no unlock password set; every sign-in will need an authenticator code (`prismd passwd`)");
    }
    let auth = Arc::new(
        Authenticator::new(session_key, secret, AuthPolicy::default())
            .with_password_hash(password_hash),
    );

    let vitals: api::SharedVitals = Arc::new(RwLock::new(api::Vitals::default()));
    let facets = Arc::new(RwLock::new(profile.facet.clone()));
    let profile_path = Arc::new(dir.join("profile.toml"));
    let events = Arc::new(prism_core::events::EventLog::new());
    events.push(prism_core::events::Level::Info, "prism", "daemon started");
    let state_dir_arc = Arc::new(state_dir.clone());
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
    let monitor_events = Arc::clone(&events);
    std::thread::Builder::new()
        .name("prism-monitor".into())
        .spawn(move || {
            let mut monitor =
                monitor::Monitor::new(profile, monitor_vitals, monitor_terms, monitor_events);
            if let Err(e) = monitor.run() {
                tracing::error!(error = %e, "monitor loop exited");
            }
        })
        .context("spawning monitor thread")?;

    serve(host, auth, vitals, facets, terminals, roots, thumb_dir, profile_path, state_dir_arc, events)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn serve(
    host: HostConfig,
    auth: Arc<Authenticator>,
    vitals: api::SharedVitals,
    facets: Arc<RwLock<Vec<prism_core::config::Facet>>>,
    terminals: Arc<prism_core::term::session::SessionManager>,
    roots: Arc<Vec<prism_core::files::path::Root>>,
    thumb_dir: Arc<std::path::PathBuf>,
    profile_path: Arc<std::path::PathBuf>,
    state_dir: Arc<std::path::PathBuf>,
    events: Arc<prism_core::events::EventLog>,
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
        profile_path,
        state_dir,
        events,
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
    // MCL_ONFAULT first. Plain MCL_CURRENT|MCL_FUTURE locks the whole *address
    // space*, which for a Rust binary with its allocator arenas is far larger
    // than what is actually resident — so it fails against a modest
    // RLIMIT_MEMLOCK even though the daemon itself is only a few MiB. ONFAULT
    // locks pages as they are faulted in, so the limit applies to real usage
    // and an unprivileged user service can succeed without any system change.
    //
    // SAFETY: `mlockall` takes only a flags bitmask and touches no caller
    // memory. The call is total; it either succeeds or reports errno.
    let onfault = unsafe {
        libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE | libc::MCL_ONFAULT)
    };
    if onfault == 0 {
        info!("memory locked (mlockall, on-fault): daemon is unswappable");
        return;
    }

    // Older kernels lack MCL_ONFAULT and return EINVAL. Fall back to the plain
    // form, which needs a larger limit but is worth attempting.
    let plain = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
    if plain == 0 {
        info!("memory locked (mlockall): daemon is unswappable");
        return;
    }

    let err = std::io::Error::last_os_error();
    warn!(
        %err,
        "could not lock memory; prism may be paged out under the pressure it \
         exists to resolve. Raise LimitMEMLOCK, or set \
         DefaultLimitMEMLOCK=infinity in /etc/systemd/user.conf."
    );
}
