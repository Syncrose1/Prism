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
mod rescue;

use anyhow::Context as _;
use prism_core::auth::{AuthPolicy, Authenticator, session::SessionKey, totp};
use prism_core::config::{self, HostConfig, Profile};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

fn main() -> anyhow::Result<()> {
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

    let secret = load_or_enrol_secret(&state_dir)?;
    let session_key = SessionKey::load_or_create(&state_dir.join("session.key"))
        .context("loading session key")?;
    let auth = Arc::new(Authenticator::new(
        session_key,
        secret,
        AuthPolicy::default(),
    ));

    let vitals: api::SharedVitals = Arc::new(RwLock::new(api::Vitals::default()));
    let facets = Arc::new(profile.facet.clone());

    // The monitor owns its own thread so that neither half can stall the other.
    let monitor_vitals = Arc::clone(&vitals);
    std::thread::Builder::new()
        .name("prism-monitor".into())
        .spawn(move || {
            let mut monitor = monitor::Monitor::new(profile, monitor_vitals);
            if let Err(e) = monitor.run() {
                tracing::error!(error = %e, "monitor loop exited");
            }
        })
        .context("spawning monitor thread")?;

    serve(host, auth, vitals, facets)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn serve(
    host: HostConfig,
    auth: Arc<Authenticator>,
    vitals: api::SharedVitals,
    facets: Arc<Vec<prism_core::config::Facet>>,
) -> anyhow::Result<()> {
    let addr = bind::resolve(&host.server.bind, host.server.port)?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    info!(%addr, "api listening");
    let app = api::router(api::AppState {
        auth,
        vitals,
        facets,
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

/// Load the TOTP secret, enrolling on first run.
///
/// Enrolment prints the provisioning URI to the log once. There is no way to
/// retrieve it later by design — a running daemon that will re-display its own
/// second factor on request is not a second factor. Losing it means deleting the
/// file and enrolling again, which is the correct recovery.
fn load_or_enrol_secret(state_dir: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;

    let path = state_dir.join("totp.secret");
    if let Ok(mut file) = std::fs::File::open(&path) {
        let mut secret = Vec::new();
        file.read_to_end(&mut secret)?;
        if secret.len() == totp::SECRET_LEN {
            return Ok(secret);
        }
        warn!(
            path = %path.display(),
            "totp secret is the wrong length; re-enrolling"
        );
    }

    let secret = totp::generate_secret().context("reading /dev/urandom")?;
    std::fs::create_dir_all(state_dir)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?
        .write_all(&secret)?;

    let account = format!(
        "{}@{}",
        std::env::var("USER").unwrap_or_else(|_| "prism".into()),
        hostname()
    );
    let uri = totp::provisioning_uri(&secret, "Prism", &account);

    info!("");
    info!("──────────────── PRISM ENROLMENT ────────────────");
    info!("Add this to Google or Microsoft Authenticator.");
    info!("");
    info!("  secret : {}", totp::base32_encode(&secret));
    info!("  uri    : {uri}");
    info!("");
    info!("Shown once only. To re-enrol, delete:");
    info!("  {}", path.display());
    info!("─────────────────────────────────────────────────");
    info!("");

    Ok(secret.to_vec())
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "localhost".into())
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
