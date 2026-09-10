//! `prismd open` — reach the shell from the machine that is hosting it.
//!
//! Prism is normally operated from somewhere else, and the authenticator is
//! right for that. It is wrong for sitting at the machine: the secret it guards
//! is a file on this disk, so demanding a phone to reach a service on the desk
//! in front of you protects nothing and costs every time.
//!
//! This exchanges the console key — a 0600 file only the owning user can read —
//! for a single-use grant, and opens a browser straight into the shell.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use prism_core::auth::console;
use prism_core::config;

pub fn command(state_dir: &Path, print_only: bool) -> std::io::Result<()> {
    let key = console::load_or_create_key(&state_dir.join("console.key"))?;
    // The same file the daemon reads, so `open` and the running service can
    // never disagree about which port to use.
    let host: config::HostConfig =
        config::load_or_default(&config::config_dir().join("prism.toml")).unwrap_or_default();
    let port = host.server.port;

    // Ask over loopback whatever the daemon is BOUND to. Loopback is not a
    // trust boundary here — the key already did that work — it is just the
    // address that is always reachable from this host.
    let base = format!("http://127.0.0.1:{port}");
    let grant = match request_grant(&base, &key) {
        Ok(g) => g,
        Err(e) => {
            // A tailnet-bound daemon does not answer on loopback, which is the
            // common case rather than an error. Try the address it advertises.
            let advertised = advertised_base(port);
            match advertised.as_deref().map(|b| request_grant(b, &key)) {
                Some(Ok(g)) => g,
                _ => {
                    eprintln!("Could not reach prismd: {e}");
                    eprintln!("Is it running?  systemctl --user status prismd");
                    return Ok(());
                }
            }
        }
    };

    let reach = advertised_base(port).unwrap_or(base);
    let url = format!("{reach}/auth/console?grant={grant}");

    if print_only {
        println!("{url}");
        return Ok(());
    }

    match open_browser(&url) {
        Ok(()) => {
            println!("Opening Prism.");
            println!("If nothing appeared, use this within 60 seconds:\n  {url}");
        }
        Err(_) => {
            println!("Open this within 60 seconds:\n  {url}");
        }
    }
    Ok(())
}

/// POST the console key, get a grant. Hand-rolled because the whole exchange is
/// one request to a service on this machine, and an HTTP client dependency in
/// the CLI path would be more than the thing it carries.
fn request_grant(base: &str, key: &str) -> std::io::Result<String> {
    use std::io::Write;
    use std::net::TcpStream;

    let (host, port) = split_authority(base)?;
    let body = format!("{{\"key\":\"{key}\"}}");
    let mut stream = TcpStream::connect((host.as_str(), port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(
        stream,
        "POST /api/auth/console HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;

    // One field out of one small JSON object. A parser would be a dependency
    // for `"grant":"…"`.
    response
        .split_once("\"grant\":\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(g, _)| g.to_string())
        .ok_or_else(|| {
            std::io::Error::other(if response.contains("bad_key") {
                "the console key was refused — prismd may be running with a different state dir"
            } else {
                "no grant in the reply"
            })
        })
}

fn split_authority(base: &str) -> std::io::Result<(String, u16)> {
    let rest = base.trim_start_matches("http://");
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| std::io::Error::other("no port in address"))?;
    let port = port
        .trim_end_matches('/')
        .parse()
        .map_err(|_| std::io::Error::other("bad port"))?;
    Ok((host.to_string(), port))
}

/// The address Prism actually advertises, so the URL that gets opened is the
/// same one that works from a phone a minute later.
fn advertised_base(port: u16) -> Option<String> {
    let out = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()?;
    let addr = String::from_utf8_lossy(&out.stdout).lines().next()?.trim().to_string();
    (!addr.is_empty()).then(|| format!("http://{addr}:{port}"))
}

fn open_browser(url: &str) -> std::io::Result<()> {
    let status = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("xdg-open failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_splits() {
        assert_eq!(
            split_authority("http://127.0.0.1:9000").unwrap(),
            ("127.0.0.1".to_string(), 9000)
        );
        assert_eq!(
            split_authority("http://100.64.0.1:9411/").unwrap(),
            ("100.64.0.1".to_string(), 9411)
        );
        assert!(split_authority("http://nope").is_err());
    }
}
