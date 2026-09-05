//! Enrolment and re-enrolment of the authenticator secret.
//!
//! The threat model here is worth stating, because it decides the design.
//!
//! Re-enrolment is gated on **local filesystem access as the owning user** —
//! running `prismd enrol` as `raahats` on the machine. That is not a weaker
//! boundary than the secret already has: anyone who can run this command can
//! also just read `totp.secret`, which is mode 0600 in the same directory. The
//! command therefore adds convenience, not exposure.
//!
//! It is deliberately **not** an HTTP endpoint, not even a localhost-only one.
//! Localhost is reachable through an SSH tunnel from anywhere, so a
//! "local-only" route would silently be a remote route for anyone with SSH —
//! and the whole point is that re-enrolment requires something stronger than a
//! network position.
//!
//! Losing the phone must therefore be recoverable without deleting files by
//! hand, which is where this started.

use anyhow::Context as _;
use prism_core::auth::totp;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Write a secret with owner-only permissions, set before any content exists.
fn write_secret(path: &Path, secret: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .write_all(secret)?;
    Ok(())
}

/// Render the provisioning URI as a terminal QR code, so the operator can scan
/// it rather than typing 32 base32 characters on a phone.
///
/// `qrencode` is optional: absent, the secret is still printed and can be
/// entered by hand. A missing convenience must never block enrolment.
fn print_qr(uri: &str) -> bool {
    let Ok(output) = std::process::Command::new("qrencode")
        .args(["-t", "UTF8", "-m", "2", "-o", "-", uri])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    println!("{}", String::from_utf8_lossy(&output.stdout));
    true
}

fn banner(secret: &[u8], uri: &str, path: &Path, reset: bool) {
    println!();
    println!("──────────────────── PRISM ENROLMENT ────────────────────");
    if reset {
        println!("The previous secret has been revoked. Any device still");
        println!("holding it can no longer sign in.");
        println!();
    }
    println!("Scan this with Google or Microsoft Authenticator:");
    println!();
    if !print_qr(uri) {
        println!("  (install `qrencode` to display a scannable code)");
        println!();
    }
    println!("  secret : {}", totp::base32_encode(secret));
    println!("  type   : time-based, 6 digits, 30 seconds");
    println!();
    println!("Stored at {}", path.display());
    println!("Re-run `prismd enrol --reset` to revoke and replace it.");
    println!("─────────────────────────────────────────────────────────");
    println!();
}

/// Load the secret, enrolling on first run.
///
/// Used by the daemon at startup. Prints the banner only when a secret is
/// actually created — a running daemon must never re-display its own second
/// factor, or it is not a second factor.
pub fn load_or_enrol(state_dir: &Path) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let path = state_dir.join("totp.secret");

    if let Ok(mut file) = std::fs::File::open(&path) {
        let mut secret = Vec::new();
        file.read_to_end(&mut secret)?;
        if secret.len() == totp::SECRET_LEN {
            return Ok(secret);
        }
        tracing::warn!(
            path = %path.display(),
            "totp secret is the wrong length; enrolling a new one"
        );
    }

    let secret = totp::generate_secret().context("reading /dev/urandom")?;
    write_secret(&path, &secret)?;
    let uri = totp::provisioning_uri(&secret, "Prism", &account());
    banner(&secret, &uri, &path, false);
    Ok(secret.to_vec())
}

/// `prismd enrol [--reset]`.
///
/// Without `--reset`, an existing secret is left alone and the command says so
/// — re-displaying a live secret on demand would defeat the point of having
/// one. With `--reset`, the old secret is revoked and replaced.
pub fn command(state_dir: &Path, reset: bool) -> anyhow::Result<()> {
    let path = state_dir.join("totp.secret");
    let exists = path.exists();

    if exists && !reset {
        println!();
        println!("An authenticator secret is already enrolled at");
        println!("  {}", path.display());
        println!();
        println!("It is not displayed again by design: a second factor that can");
        println!("be re-read on request is not a second factor.");
        println!();
        println!("If the device holding it is lost, revoke and replace it with:");
        println!("  prismd enrol --reset");
        println!();
        return Ok(());
    }

    let secret = totp::generate_secret().context("reading /dev/urandom")?;
    write_secret(&path, &secret)?;

    // Sessions are signed with a separate key. Rotating it too means every
    // device already holding a session is signed out — which is the point of a
    // reset: a lost phone may have a live 30-day session on it, and revoking
    // only the TOTP secret would leave that session working.
    if reset && exists {
        let key_path = state_dir.join("session.key");
        if key_path.exists() {
            std::fs::remove_file(&key_path)
                .with_context(|| format!("removing {}", key_path.display()))?;
            println!();
            println!("Existing sessions have also been invalidated.");
        }
    }

    let uri = totp::provisioning_uri(&secret, "Prism", &account());
    banner(&secret, &uri, &path, reset && exists);

    if exists {
        println!("Restart prismd for the change to take effect.");
        println!();
    }
    Ok(())
}

fn account() -> String {
    format!(
        "{}@{}",
        std::env::var("USER").unwrap_or_else(|_| "prism".into()),
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "localhost".into())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "prism-enrol-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn first_run_creates_a_secret_of_the_right_length() {
        let d = tmp("first");
        let s = load_or_enrol(&d).expect("enrol");
        assert_eq!(s.len(), totp::SECRET_LEN);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_secret_is_stable_across_restarts() {
        // Otherwise every daemon restart would silently invalidate the phone.
        let d = tmp("stable");
        let a = load_or_enrol(&d).unwrap();
        let b = load_or_enrol(&d).unwrap();
        assert_eq!(a, b);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_secret_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("perms");
        load_or_enrol(&d).unwrap();
        let mode = std::fs::metadata(d.join("totp.secret"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the secret must not be readable by others");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_truncated_secret_is_replaced_rather_than_used() {
        let d = tmp("short");
        std::fs::write(d.join("totp.secret"), b"too short").unwrap();
        let s = load_or_enrol(&d).unwrap();
        assert_eq!(s.len(), totp::SECRET_LEN);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn enrol_without_reset_leaves_an_existing_secret_untouched() {
        let d = tmp("noreset");
        let before = load_or_enrol(&d).unwrap();
        command(&d, false).unwrap();
        let after = std::fs::read(d.join("totp.secret")).unwrap();
        assert_eq!(before, after, "a live secret must not be rotated by accident");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reset_replaces_the_secret() {
        let d = tmp("reset");
        let before = load_or_enrol(&d).unwrap();
        command(&d, true).unwrap();
        let after = std::fs::read(d.join("totp.secret")).unwrap();
        assert_ne!(before, after, "reset must revoke the old secret");
        assert_eq!(after.len(), totp::SECRET_LEN);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reset_also_invalidates_live_sessions() {
        // A lost phone may hold a 30-day session; revoking only the TOTP secret
        // would leave that session working.
        let d = tmp("sessions");
        load_or_enrol(&d).unwrap();
        std::fs::write(d.join("session.key"), [7u8; 32]).unwrap();
        command(&d, true).unwrap();
        assert!(
            !d.join("session.key").exists(),
            "existing sessions must not survive a reset"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn enrol_on_a_fresh_dir_creates_rather_than_refusing() {
        let d = tmp("fresh");
        command(&d, false).unwrap();
        assert!(d.join("totp.secret").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
