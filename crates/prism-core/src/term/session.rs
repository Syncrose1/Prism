//! Session lifecycle.
//!
//! The central decision, from ADR 0003: **closing a window detaches; only an
//! explicit kill destroys.**
//!
//! The naive reading of "killing it in Prism should kill it on the machine" is
//! right for *Kill* and wrong for *Close*. The operator's own use case proves
//! it — launching ComfyUI from a terminal and then closing the window must not
//! kill ComfyUI. A locked phone, a closed laptop, or a train entering a tunnel
//! would otherwise take down the workload. So the two are separate operations
//! with separate controls, never merged.
//!
//! Each session runs under `systemd-run --user --scope`, which yields a PTY and
//! a cgroup from one mechanism. A terminal is the least supervised path in the
//! whole system — it is exactly how somebody accidentally runs the thing that
//! eats the machine — so it gets the same containment as every other workload
//! rather than being the one hole in the design.

use super::pty::{Pty, WinSize};
use super::ring::Ring;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::broadcast;

/// How long an exited session lingers before being reaped.
///
/// Without this, a command that fails instantly would vanish before its error
/// could be read — the session would disappear along with the explanation.
const LINGER: Duration = Duration::from_secs(300);

/// Buffered chunks per subscriber. Terminal output is bursty; a build flooding
/// the channel must not stall the reader thread feeding every other subscriber.
const BROADCAST_DEPTH: usize = 256;

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub pid: u32,
    pub created_unix: u64,
    pub attached: usize,
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub scrollback_bytes: usize,
    /// Bytes discarded to stay within the cap. Non-zero means the history shown
    /// on reattach is incomplete, and the UI should say so.
    pub scrollback_dropped: u64,
}

pub struct Session {
    pub id: String,
    pub title: Mutex<String>,
    pub created: SystemTime,
    /// Transient scope unit, when systemd was available.
    pub unit: Option<String>,
    pty: Arc<Pty>,
    scrollback: Arc<Mutex<Ring>>,
    tx: broadcast::Sender<Arc<Vec<u8>>>,
    exited: Arc<AtomicBool>,
    exit_code: Arc<Mutex<Option<i32>>>,
    ended_at: Arc<Mutex<Option<SystemTime>>>,
}

impl Session {
    /// Subscribe to live output.
    ///
    /// Returns the scrollback *and* the receiver together, under one lock, so a
    /// subscriber cannot miss bytes written between reading history and
    /// subscribing.
    pub fn attach(&self) -> (Vec<u8>, broadcast::Receiver<Arc<Vec<u8>>>) {
        let history = self.scrollback.lock().expect("scrollback poisoned");
        let rx = self.tx.subscribe();
        (history.snapshot(), rx)
    }

    /// Send input, as if typed at the keyboard.
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.exited.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "session has exited",
            ));
        }
        self.pty.write(bytes)
    }

    pub fn resize(&self, size: WinSize) -> std::io::Result<()> {
        self.pty.resize(size)
    }

    pub fn pid(&self) -> u32 {
        self.pty.pid()
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }

    pub fn info(&self) -> SessionInfo {
        let ring = self.scrollback.lock().expect("scrollback poisoned");
        SessionInfo {
            id: self.id.clone(),
            title: self.title.lock().expect("title poisoned").clone(),
            pid: self.pty.pid(),
            created_unix: self
                .created
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            attached: self.tx.receiver_count(),
            exited: self.exited.load(Ordering::Relaxed),
            exit_code: *self.exit_code.lock().expect("exit code poisoned"),
            scrollback_bytes: ring.len(),
            scrollback_dropped: ring.dropped(),
        }
    }

    /// Shrink retained scrollback — used when the governor de-escalates.
    pub fn set_scrollback_capacity(&self, cap: usize) {
        self.scrollback
            .lock()
            .expect("scrollback poisoned")
            .set_capacity(cap);
    }

    /// Destroy the session and everything it started.
    ///
    /// Prefers `cgroup.kill`, which is atomic across the entire process tree:
    /// a shell that launched a workload that forked workers dies in one
    /// operation, with no pid races and nothing orphaned. Falls back to
    /// signalling the child directly when no scope was created.
    fn destroy(&self) {
        if let Some(unit) = &self.unit
            && kill_scope(unit).is_ok()
        {
            return;
        }
        let _ = self.pty.signal(libc::SIGKILL);
    }
}

/// Write to a transient scope's `cgroup.kill`.
fn kill_scope(unit: &str) -> std::io::Result<()> {
    let output = std::process::Command::new("systemctl")
        .args(["--user", "show", unit, "-p", "ControlGroup", "--value"])
        .output()?;
    let rel = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if rel.is_empty() || rel == "/" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no cgroup",
        ));
    }
    let path = std::path::Path::new("/sys/fs/cgroup")
        .join(rel.trim_start_matches('/'))
        .join("cgroup.kill");
    std::fs::write(path, "1")
}

#[derive(Debug, Clone)]
pub struct TermConfig {
    pub enabled: bool,
    pub shell: String,
    pub scrollback_bytes: usize,
    pub max_sessions: usize,
    /// Wrap sessions in a transient systemd scope. Disabled in tests and on
    /// hosts without a user manager.
    pub use_scope: bool,
}

/// The account's login shell, from the password database.
///
/// Deliberately not `$SHELL`: the daemon inherits that from whatever launched
/// it, so a service started from a script would hand the operator a different
/// shell than the one they actually use. `/etc/passwd` is the source of truth.
pub fn login_shell() -> String {
    // SAFETY: getpwuid returns a pointer into a static buffer owned by libc; we
    // copy out of it immediately and never retain it. A null return means no
    // entry, which is handled.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_shell.is_null() {
            if let Ok(s) = std::ffi::CStr::from_ptr((*pw).pw_shell).to_str()
                && !s.is_empty()
            {
                return s.to_string();
            }
        }
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

impl Default for TermConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shell: login_shell(),
            scrollback_bytes: super::ring::DEFAULT_CAPACITY,
            // A ceiling on concurrent sessions: each holds a shell, a thread and
            // a scrollback buffer, and an accidental loop opening terminals
            // should hit a wall rather than the machine's memory.
            max_sessions: 16,
            use_scope: true,
        }
    }
}

#[derive(Debug)]
pub enum SpawnError {
    Disabled,
    TooMany,
    Io(std::io::Error),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::Disabled => write!(f, "terminal sessions are disabled"),
            SpawnError::TooMany => write!(f, "too many open sessions"),
            SpawnError::Io(e) => write!(f, "{e}"),
        }
    }
}

pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    cfg: TermConfig,
    counter: Mutex<u64>,
}

impl SessionManager {
    pub fn new(cfg: TermConfig) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            cfg,
            counter: Mutex::new(0),
        }
    }

    pub fn config(&self) -> &TermConfig {
        &self.cfg
    }

    /// Start a session. `argv` empty means the configured login shell.
    pub fn create(
        &self,
        argv: &[String],
        cwd: Option<&str>,
        size: WinSize,
        title: &str,
    ) -> Result<Arc<Session>, SpawnError> {
        self.create_with(argv, cwd, size, title, false, None)
    }

    /// As [`Self::create`], but the command is made proof against hangup.
    ///
    /// Used for facets: a long-running workload started through a terminal
    /// should survive the terminal, and the daemon, and anything short of an
    /// explicit Kill.
    pub fn create_hangup_proof(
        &self,
        argv: &[String],
        cwd: Option<&str>,
        size: WinSize,
        title: &str,
    ) -> Result<Arc<Session>, SpawnError> {
        self.create_with(argv, cwd, size, title, true, None)
    }

    /// Start a facet's interactive launcher, under a unit named after the
    /// facet rather than after the session.
    ///
    /// The name is the whole point. A session id is random and lives only in
    /// this process, so once the daemon restarts nothing can tell that the
    /// scope still running belongs to a facet — and the workload gets reported
    /// as something Prism did not start and cannot control, which is both
    /// wrong and unhelpful. Naming the unit after the facet puts that
    /// association in systemd, where it survives the daemon.
    pub fn create_for_facet(
        &self,
        facet_id: &str,
        argv: &[String],
        cwd: Option<&str>,
        size: WinSize,
        title: &str,
    ) -> Result<Arc<Session>, SpawnError> {
        self.create_with(argv, cwd, size, title, true, Some(facet_id))
    }

    fn create_with(
        &self,
        argv: &[String],
        cwd: Option<&str>,
        size: WinSize,
        title: &str,
        hangup_proof: bool,
        facet_id: Option<&str>,
    ) -> Result<Arc<Session>, SpawnError> {
        if !self.cfg.enabled {
            return Err(SpawnError::Disabled);
        }

        self.reap();

        {
            let live = self.sessions.lock().expect("sessions poisoned");
            if live.values().filter(|s| !s.has_exited()).count() >= self.cfg.max_sessions {
                return Err(SpawnError::TooMany);
            }
        }

        let id = {
            let mut c = self.counter.lock().expect("counter poisoned");
            *c += 1;
            format!(
                "{:x}-{}",
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                c
            )
        };

        // `-l` so the shell sources the operator's profile and behaves like the
        // terminal they normally open, rather than a bare non-login shell.
        let base: Vec<String> = if argv.is_empty() {
            vec![self.cfg.shell.clone(), "-l".to_string()]
        } else if hangup_proof {
            // A workload launched through a terminal must outlive the terminal.
            // Ignoring SIGHUP before exec is enough: a signal set to *ignored*
            // survives exec, unlike one that is merely caught. Without this,
            // closing the window or restarting the daemon kills the workload,
            // which is exactly what detaching is supposed to prevent.
            let mut v = vec![
                "/bin/sh".to_string(),
                "-c".into(),
                r#"trap "" HUP; exec "$@""#.into(),
                "prism".into(),
            ];
            v.extend(argv.iter().cloned());
            v
        } else {
            argv.to_vec()
        };

        // Wrap in a transient scope so the session is contained by a cgroup.
        // `--quiet` keeps systemd's "Running scope as unit…" line out of the
        // terminal the operator is looking at.
        let unit = self.cfg.use_scope.then(|| match facet_id {
            Some(f) => crate::supervisor::facet_scope_name(f),
            None => format!("prism-term-{id}.scope"),
        });
        let full: Vec<String> = match &unit {
            Some(u) => {
                let mut v = vec![
                    "systemd-run".to_string(),
                    "--user".into(),
                    "--scope".into(),
                    "--quiet".into(),
                    format!("--unit={u}"),
                    "--property=MemoryAccounting=yes".into(),
                    "--".into(),
                ];
                v.extend(base.clone());
                v
            }
            None => base.clone(),
        };

        let pty = Arc::new(Pty::spawn(&full, cwd, size).map_err(SpawnError::Io)?);
        let scrollback = Arc::new(Mutex::new(Ring::new(self.cfg.scrollback_bytes)));
        let (tx, _) = broadcast::channel(BROADCAST_DEPTH);
        let exited = Arc::new(AtomicBool::new(false));
        let exit_code = Arc::new(Mutex::new(None));
        let ended_at = Arc::new(Mutex::new(None));

        let session = Arc::new(Session {
            id: id.clone(),
            title: Mutex::new(title.to_string()),
            created: SystemTime::now(),
            unit,
            pty: Arc::clone(&pty),
            scrollback: Arc::clone(&scrollback),
            tx: tx.clone(),
            exited: Arc::clone(&exited),
            exit_code: Arc::clone(&exit_code),
            ended_at: Arc::clone(&ended_at),
        });

        // One reader thread per session. Blocking reads on the PTY master keep
        // output latency at the kernel's, which polling would not: terminal echo
        // has to feel immediate or the whole thing feels broken.
        std::thread::Builder::new()
            .name(format!("prism-pty-{id}"))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match pty.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = Arc::new(buf[..n].to_vec());
                            scrollback
                                .lock()
                                .expect("scrollback poisoned")
                                .push(&chunk);
                            // Errors mean nobody is attached, which is the
                            // normal detached state, not a failure.
                            let _ = tx.send(chunk);
                        }
                        Err(_) => break,
                    }
                }
                exited.store(true, Ordering::Relaxed);
                *ended_at.lock().expect("ended_at poisoned") = Some(SystemTime::now());

                // EOF on the master means the slave closed, which usually but
                // not always means the child has already been reaped. Polling
                // once here loses the exit code whenever the read wins the
                // race — so retry briefly rather than reporting "exited, code
                // unknown" for a command that plainly returned one.
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                loop {
                    if let Some(code) = pty.try_wait() {
                        *exit_code.lock().expect("exit code poisoned") = Some(code);
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
            .map_err(SpawnError::Io)?;

        self.sessions
            .lock()
            .expect("sessions poisoned")
            .insert(id, Arc::clone(&session));
        Ok(session)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .get(id)
            .cloned()
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        let mut out: Vec<SessionInfo> = self
            .sessions
            .lock()
            .expect("sessions poisoned")
            .values()
            .map(|s| s.info())
            .collect();
        out.sort_by_key(|s| s.created_unix);
        out
    }

    /// Kill a session and remove it. This is the only operation that destroys.
    pub fn kill(&self, id: &str) -> bool {
        let session = self.sessions.lock().expect("sessions poisoned").remove(id);
        match session {
            Some(s) => {
                s.destroy();
                true
            }
            None => false,
        }
    }

    /// Drop exited sessions once their linger has elapsed.
    pub fn reap(&self) {
        let now = SystemTime::now();
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .retain(|_, s| {
                if !s.has_exited() {
                    return true;
                }
                match *s.ended_at.lock().expect("ended_at poisoned") {
                    Some(t) => now.duration_since(t).map(|d| d < LINGER).unwrap_or(true),
                    None => true,
                }
            });
    }

    /// Number of sessions that have not exited.
    pub fn live_count(&self) -> usize {
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .values()
            .filter(|s| !s.has_exited())
            .count()
    }

    /// Reduce scrollback across all sessions — the de-escalation path.
    pub fn shrink_all_scrollback(&self, cap: usize) {
        for s in self.sessions.lock().expect("sessions poisoned").values() {
            s.set_scrollback_capacity(cap);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Scopes are avoided in tests: the systemd user manager may be absent, and
    /// the behaviour under test is lifecycle, not containment.
    fn mgr() -> SessionManager {
        SessionManager::new(TermConfig {
            shell: "/bin/sh".into(),
            use_scope: false,
            ..Default::default()
        })
    }

    fn wait_for(f: impl Fn() -> bool, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    fn collect(session: &Session, needle: &str, secs: u64) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            let (history, _) = session.attach();
            let text = String::from_utf8_lossy(&history).to_string();
            if text.contains(needle) {
                return text;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let (history, _) = session.attach();
        String::from_utf8_lossy(&history).to_string()
    }

    #[test]
    fn creates_a_session_and_captures_output() {
        let m = mgr();
        let s = m
            .create(&["echo".into(), "hello-prism".into()], None, WinSize::default(), "test")
            .expect("create");
        assert!(collect(&s, "hello-prism", 5).contains("hello-prism"));
    }

    #[test]
    fn disabled_manager_refuses_to_spawn() {
        let m = SessionManager::new(TermConfig {
            enabled: false,
            use_scope: false,
            ..Default::default()
        });
        assert!(matches!(
            m.create(&[], None, WinSize::default(), "t"),
            Err(SpawnError::Disabled)
        ));
    }

    #[test]
    fn enforces_a_session_ceiling() {
        let m = SessionManager::new(TermConfig {
            shell: "/bin/sh".into(),
            use_scope: false,
            max_sessions: 2,
            ..Default::default()
        });
        let a = m.create(&["sleep".into(), "30".into()], None, WinSize::default(), "a");
        let b = m.create(&["sleep".into(), "30".into()], None, WinSize::default(), "b");
        assert!(a.is_ok() && b.is_ok());
        assert!(
            matches!(
                m.create(&["sleep".into(), "30".into()], None, WinSize::default(), "c"),
                Err(SpawnError::TooMany)
            ),
            "an accidental loop opening terminals must hit a wall"
        );
        m.kill(&a.unwrap().id);
        m.kill(&b.unwrap().id);
    }

    /// The central lifecycle guarantee.
    #[test]
    fn detaching_does_not_kill_the_session() {
        let m = mgr();
        let s = m
            .create(&["sleep".into(), "30".into()], None, WinSize::default(), "detach")
            .expect("create");
        let pid = s.pid();

        // Attach, then drop the receiver — exactly what closing a window does.
        {
            let (_history, _rx) = s.attach();
        }
        std::thread::sleep(Duration::from_millis(200));

        assert!(
            std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "closing a window must not kill the workload it launched"
        );
        assert!(!s.has_exited());
        m.kill(&s.id);
    }

    #[test]
    fn a_hangup_proof_command_ignores_sighup() {
        // The bug this prevents: restarting the daemon killed every workload
        // launched through a terminal, which is the opposite of what detaching
        // promises. `trap "" HUP` before exec survives the exec, because an
        // ignored disposition does and a caught one does not.
        let m = mgr();
        let s = m
            .create_hangup_proof(
                &["sleep".into(), "30".into()],
                None,
                WinSize::default(),
                "proof",
            )
            .expect("create");
        std::thread::sleep(Duration::from_millis(400));

        // Signal the whole group, as closing a terminal would.
        let pid = s.pid();
        unsafe { libc::kill(-(pid as i32), libc::SIGHUP) };
        std::thread::sleep(Duration::from_millis(400));

        assert!(
            std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "a facet must survive a hangup"
        );
        m.kill(&s.id);
    }

    #[test]
    fn an_ordinary_session_is_not_wrapped() {
        // A plain terminal should be the operator's shell, not a shell inside a
        // wrapper, or the process tree and job control become confusing.
        let m = mgr();
        let s = m
            .create(&["sleep".into(), "30".into()], None, WinSize::default(), "plain")
            .expect("create");
        std::thread::sleep(Duration::from_millis(300));
        let cmdline = std::fs::read_to_string(format!("/proc/{}/cmdline", s.pid()))
            .unwrap_or_default();
        assert!(cmdline.contains("sleep"), "got {cmdline:?}");
        m.kill(&s.id);
    }

    #[test]
    fn kill_destroys_the_session_and_removes_it() {
        let m = mgr();
        let s = m
            .create(&["sleep".into(), "30".into()], None, WinSize::default(), "kill")
            .expect("create");
        let (id, pid) = (s.id.clone(), s.pid());

        assert!(m.kill(&id));
        assert!(wait_for(
            || !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            5
        ));
        assert!(m.get(&id).is_none());
        assert!(!m.kill(&id), "killing twice is not an error but is not a kill");
    }

    #[test]
    fn reattaching_replays_scrollback() {
        let m = mgr();
        let s = m
            .create(&["echo".into(), "earlier-output".into()], None, WinSize::default(), "re")
            .expect("create");
        collect(&s, "earlier-output", 5);

        // A later attacher sees what happened before it arrived.
        let (history, _rx) = s.attach();
        assert!(String::from_utf8_lossy(&history).contains("earlier-output"));
    }

    #[test]
    fn live_output_reaches_a_subscriber() {
        let m = mgr();
        let s = m
            .create(&["sh".into(), "-c".into(), "read x; echo ACK:$x".into()], None, WinSize::default(), "live")
            .expect("create");
        std::thread::sleep(Duration::from_millis(200));

        let (_h, mut rx) = s.attach();
        s.write(b"ping\n").expect("write");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = String::new();
        while Instant::now() < deadline {
            if let Ok(chunk) = rx.try_recv() {
                seen.push_str(&String::from_utf8_lossy(&chunk));
                if seen.contains("ACK:ping") {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(seen.contains("ACK:ping"), "got: {seen:?}");
    }

    #[test]
    fn exit_is_observed_with_its_code() {
        let m = mgr();
        let s = m
            .create(&["sh".into(), "-c".into(), "exit 3".into()], None, WinSize::default(), "code")
            .expect("create");
        assert!(wait_for(|| s.has_exited(), 5));
        assert!(wait_for(|| s.info().exit_code == Some(3), 5));
    }

    #[test]
    fn writing_to_an_exited_session_errors_rather_than_hanging() {
        let m = mgr();
        let s = m
            .create(&["true".into()], None, WinSize::default(), "gone")
            .expect("create");
        assert!(wait_for(|| s.has_exited(), 5));
        assert!(s.write(b"anything\n").is_err());
    }

    #[test]
    fn exited_sessions_linger_so_their_error_can_be_read() {
        let m = mgr();
        let s = m
            .create(&["sh".into(), "-c".into(), "echo boom; exit 1".into()], None, WinSize::default(), "linger")
            .expect("create");
        let id = s.id.clone();
        assert!(wait_for(|| s.has_exited(), 5));

        m.reap();
        assert!(
            m.get(&id).is_some(),
            "a command that fails instantly must not vanish with its explanation"
        );
        assert!(collect(&s, "boom", 2).contains("boom"));
    }

    #[test]
    fn listing_reports_sessions_oldest_first() {
        let m = mgr();
        let a = m.create(&["sleep".into(), "30".into()], None, WinSize::default(), "first").unwrap();
        std::thread::sleep(Duration::from_millis(1100)); // created_unix has 1s resolution
        let b = m.create(&["sleep".into(), "30".into()], None, WinSize::default(), "second").unwrap();

        let list = m.list();
        assert_eq!(list.len(), 2);
        assert!(list[0].created_unix <= list[1].created_unix);
        m.kill(&a.id);
        m.kill(&b.id);
    }

    #[test]
    fn live_count_ignores_exited_sessions() {
        let m = mgr();
        let alive = m.create(&["sleep".into(), "30".into()], None, WinSize::default(), "a").unwrap();
        let dead = m.create(&["true".into()], None, WinSize::default(), "b").unwrap();
        assert!(wait_for(|| dead.has_exited(), 5));
        assert!(wait_for(|| m.live_count() == 1, 5));
        m.kill(&alive.id);
    }

    #[test]
    fn scrollback_can_be_shrunk_across_all_sessions() {
        let m = mgr();
        let s = m
            .create(&["echo".into(), "x".repeat(500)], None, WinSize::default(), "shrink")
            .expect("create");
        collect(&s, "x", 5);
        m.shrink_all_scrollback(64);
        assert!(s.info().scrollback_bytes <= 64);
    }

    #[test]
    fn cwd_is_honoured() {
        let m = mgr();
        let s = m
            .create(&["pwd".into()], Some("/tmp"), WinSize::default(), "cwd")
            .expect("create");
        assert!(collect(&s, "/tmp", 5).contains("/tmp"));
    }
}
