//! Pseudo-terminals.
//!
//! A real PTY, not captured stdout. The distinction is the whole reason this
//! module exists: the operator's ComfyUI launcher prompts for input, and a pipe
//! cannot answer a prompt. Interactive scripts, `vim`, `htop` and progress bars
//! all require a terminal on the other end or they behave differently — or
//! refuse to run.
//!
//! The child is normally `systemd-run --user --scope`, so a session gets a
//! terminal *and* a cgroup from one mechanism. That matters here more than
//! anywhere else in Prism: a terminal is the least supervised code path in the
//! system and precisely how somebody accidentally runs the thing that eats the
//! machine. See ADR 0003.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

/// A running pseudo-terminal and its child process.
#[derive(Debug)]
pub struct Pty {
    master: OwnedFd,
    pid: libc::pid_t,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinSize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for WinSize {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

impl Pty {
    /// Fork a child attached to a new pseudo-terminal.
    ///
    /// `argv[0]` is resolved via `PATH`. `cwd`, if given, is entered before exec.
    pub fn spawn(argv: &[String], cwd: Option<&str>, size: WinSize) -> io::Result<Self> {
        if argv.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty argv"));
        }

        // Everything the child needs must be allocated *before* the fork. After
        // forking, only async-signal-safe calls are permitted, and this process
        // is multithreaded (tokio, the monitor thread) — allocating in the child
        // can deadlock on a malloc lock held by a thread that no longer exists.
        let program = CString::new(argv[0].as_str())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in program"))?;
        let args: Vec<CString> = argv
            .iter()
            .map(|a| CString::new(a.as_str()))
            .collect::<Result<_, _>>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in argument"))?;
        let mut argv_ptrs: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());

        let cwd_c = match cwd {
            Some(d) => Some(
                CString::new(d)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in cwd"))?,
            ),
            None => None,
        };

        let winsize = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let mut master: libc::c_int = -1;

        // SAFETY: `forkpty` writes the master fd into `master` and returns 0 in
        // the child. The winsize pointer is valid for the duration of the call.
        let pid = unsafe {
            libc::forkpty(
                &mut master,
                std::ptr::null_mut(),
                std::ptr::null(),
                &winsize as *const _ as *mut _,
            )
        };

        if pid < 0 {
            return Err(io::Error::last_os_error());
        }

        if pid == 0 {
            // Child. Async-signal-safe calls only — no allocation, no logging,
            // no panicking. Everything below was prepared before the fork.
            // SAFETY: all pointers were built in the parent and remain valid.
            unsafe {
                if let Some(dir) = &cwd_c {
                    // A failed chdir is not fatal: running in the wrong
                    // directory beats refusing to give the operator a shell.
                    libc::chdir(dir.as_ptr());
                }
                libc::execvp(program.as_ptr(), argv_ptrs.as_ptr());
                // Only reached if exec failed. 127 is the shell convention for
                // "command not found".
                libc::_exit(127);
            }
        }

        // SAFETY: forkpty returned a fresh master fd that we now solely own.
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        Ok(Self { master, pid })
    }

    pub fn pid(&self) -> u32 {
        self.pid as u32
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }

    /// Read output. Returns `Ok(0)` once the child has closed the terminal.
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: writes at most buf.len() bytes into a buffer we own.
        let n = unsafe {
            libc::read(
                self.master.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            // EIO from a PTY master means the slave side is gone. That is a
            // normal exit, not a failure, and reporting it as an error would
            // make every clean shell exit look like a fault.
            if err.raw_os_error() == Some(libc::EIO) {
                return Ok(0);
            }
            return Err(err);
        }
        Ok(n as usize)
    }

    /// Send input, as if typed.
    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        // SAFETY: reads at most buf.len() bytes from a buffer we own.
        let n = unsafe {
            libc::write(
                self.master.as_raw_fd(),
                buf.as_ptr() as *const libc::c_void,
                buf.len(),
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    /// Tell the child its terminal changed size.
    ///
    /// Without this, a resized browser window leaves full-screen programs
    /// drawing to the old geometry — the classic "vim thinks the screen is 80
    /// columns" symptom.
    pub fn resize(&self, size: WinSize) -> io::Result<()> {
        let ws = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: TIOCSWINSZ reads one winsize from the pointer we supply.
        let rc = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Has the child exited? Reaps it if so, returning the exit status.
    ///
    /// Non-blocking: a session manager polls this rather than waiting, so one
    /// stuck child cannot stall every other session.
    pub fn try_wait(&self) -> Option<i32> {
        let mut status: libc::c_int = 0;
        // SAFETY: writes one int into a local we own.
        let rc = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
        if rc == self.pid {
            return Some(if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                128 + libc::WTERMSIG(status)
            } else {
                -1
            });
        }
        None
    }

    /// Signal the child directly.
    ///
    /// The preferred kill path for a session is `cgroup.kill` on its scope,
    /// which is atomic across the whole tree. This exists for the case where no
    /// scope was used, and for the SIGHUP that a polite close sends first.
    pub fn signal(&self, sig: i32) -> io::Result<()> {
        if self.pid <= 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no child"));
        }
        // SAFETY: scalar arguments; an exited pid returns ESRCH.
        if unsafe { libc::kill(self.pid, sig) } < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(err);
            }
        }
        Ok(())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Closing the master sends SIGHUP to the foreground process group, so a
        // dropped Pty does not leave an orphaned shell attached to nothing.
        let _ = self.signal(libc::SIGHUP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Read until `needle` appears or the deadline passes.
    fn read_until(pty: &Pty, needle: &str, secs: u64) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            match pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    if String::from_utf8_lossy(&out).contains(needle) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&out).to_string()
    }

    #[test]
    fn spawns_and_reads_output() {
        let pty = Pty::spawn(&["echo".into(), "prism-pty".into()], None, WinSize::default())
            .expect("spawn echo");
        assert!(read_until(&pty, "prism-pty", 5).contains("prism-pty"));
    }

    #[test]
    fn empty_argv_is_rejected() {
        assert!(Pty::spawn(&[], None, WinSize::default()).is_err());
    }

    #[test]
    fn argument_containing_nul_is_rejected() {
        let bad = String::from("echo\0evil");
        assert!(Pty::spawn(&[bad], None, WinSize::default()).is_err());
    }

    #[test]
    fn a_missing_program_exits_127_rather_than_hanging() {
        let pty = Pty::spawn(
            &["definitely-not-a-real-binary-xyzzy".into()],
            None,
            WinSize::default(),
        )
        .expect("fork succeeds even when exec will not");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut code = None;
        while Instant::now() < deadline {
            if let Some(c) = pty.try_wait() {
                code = Some(c);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(code, Some(127), "exec failure must surface as 127");
    }

    /// The requirement that motivated a real PTY: a program that reads from its
    /// terminal and would receive nothing over a pipe.
    #[test]
    fn interactive_input_reaches_the_child() {
        let pty = Pty::spawn(
            &[
                "sh".into(),
                "-c".into(),
                "read line; echo GOT:$line".into(),
            ],
            None,
            WinSize::default(),
        )
        .expect("spawn sh");

        std::thread::sleep(Duration::from_millis(150));
        pty.write(b"comfyui\n").expect("write to pty");
        assert!(
            read_until(&pty, "GOT:comfyui", 5).contains("GOT:comfyui"),
            "an interactive prompt must be answerable — this is why a pipe will not do"
        );
    }

    #[test]
    fn child_sees_the_requested_terminal_size() {
        let pty = Pty::spawn(
            &["sh".into(), "-c".into(), "stty size".into()],
            None,
            WinSize { rows: 40, cols: 132 },
        )
        .expect("spawn sh");
        assert!(read_until(&pty, "40 132", 5).contains("40 132"));
    }

    #[test]
    fn resize_is_visible_to_the_child() {
        let pty = Pty::spawn(
            &[
                "sh".into(),
                "-c".into(),
                "read x; stty size".into(),
            ],
            None,
            WinSize::default(),
        )
        .expect("spawn sh");

        std::thread::sleep(Duration::from_millis(150));
        pty.resize(WinSize { rows: 50, cols: 200 }).expect("resize");
        pty.write(b"\n").expect("nudge");
        assert!(read_until(&pty, "50 200", 5).contains("50 200"));
    }

    #[test]
    fn cwd_is_entered_before_exec() {
        let pty = Pty::spawn(&["pwd".into()], Some("/tmp"), WinSize::default()).expect("spawn pwd");
        assert!(read_until(&pty, "/tmp", 5).contains("/tmp"));
    }

    #[test]
    fn reports_a_real_child_pid() {
        let pty = Pty::spawn(&["sleep".into(), "5".into()], None, WinSize::default()).unwrap();
        let pid = pty.pid();
        assert!(pid > 1);
        assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
        let _ = pty.signal(libc::SIGKILL);
    }

    #[test]
    fn exit_after_close_reads_zero_not_an_error() {
        // A shell exiting yields EIO on the master. That is a normal end of
        // session and must not be reported as a failure.
        let pty = Pty::spawn(&["true".into()], None, WinSize::default()).unwrap();
        let mut buf = [0u8; 128];
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut sawe_end = false;
        while Instant::now() < deadline {
            match pty.read(&mut buf) {
                Ok(0) => {
                    sawe_end = true;
                    break;
                }
                Ok(_) => continue,
                Err(e) => panic!("clean exit surfaced as an error: {e}"),
            }
        }
        assert!(sawe_end, "should observe end of output");
    }
}
