//! What is already listening on this machine.
//!
//! Prism used to learn about workloads by recognising them: a path check for
//! ComfyUI, a port for Syncthing. That does not generalise. Every app the
//! operator installs afterwards is invisible until someone edits TOML, and the
//! list of things Prism knows about is a list of things somebody thought of in
//! advance.
//!
//! So instead of recognising applications, this enumerates *sockets*. Anything
//! serving HTTP on this host is a candidate, whatever it is and whoever wrote
//! it. The sweep proposes; the operator disposes. Nothing here adds a facet on
//! its own — a machine that silently published whatever happened to be
//! listening would be a poor citizen, and the operator's own judgement about
//! what belongs in their desktop is the point.
//!
//! Probing is done over loopback because that is what the proxy connects to. A
//! service reachable here is one Prism can serve; one it cannot reach here it
//! cannot serve, whatever else it may be bound to. The two answers agree by
//! construction rather than by coincidence.

use crate::api::AppState;
use axum::{Json, extract::State, http::HeaderMap, response::Response, routing::get, Router};
use http_body_util::BodyExt;
use serde::Serialize;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A listening TCP socket, straight from the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub addr: IpAddr,
    pub port: u16,
    pub inode: u64,
}

/// Parse `/proc/net/tcp` or `/proc/net/tcp6`.
///
/// Split from reading the file so the format can be tested against real
/// fixtures without a machine that happens to be running the right things.
///
/// Only state `0A` (TCP_LISTEN) is of interest. Established connections share
/// the file and would otherwise be reported as services.
pub fn parse_listeners(contents: &str) -> Vec<Listener> {
    let mut out = Vec::new();
    for line in contents.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        // sl local rem st tx:rx tr tm retrnsmt uid timeout inode
        if f.len() < 10 || f[3] != "0A" {
            continue;
        }
        let Some((addr_hex, port_hex)) = f[1].split_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        let Some(addr) = parse_hex_addr(addr_hex) else {
            continue;
        };
        let inode = f[9].parse().unwrap_or(0);
        out.push(Listener { addr, port, inode });
    }
    out
}

/// Addresses in `/proc/net/tcp` are hex in host byte order, so on a
/// little-endian machine each 32-bit word reads back-to-front. 8 hex digits is
/// IPv4; 32 is IPv6 as four such words.
fn parse_hex_addr(hex: &str) -> Option<IpAddr> {
    match hex.len() {
        8 => {
            let n = u32::from_str_radix(hex, 16).ok()?;
            Some(IpAddr::V4(Ipv4Addr::from(n.swap_bytes())))
        }
        32 => {
            let mut bytes = [0u8; 16];
            for w in 0..4 {
                let word = u32::from_str_radix(&hex[w * 8..w * 8 + 8], 16).ok()?;
                bytes[w * 4..w * 4 + 4].copy_from_slice(&word.swap_bytes().to_be_bytes());
            }
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        }
        _ => None,
    }
}

/// Map socket inodes to the process holding them, by walking `/proc/*/fd`.
///
/// Only processes this user owns are readable, which is the relevant set: a
/// root daemon's port is not something Prism can manage anyway. Unreadable
/// entries are skipped rather than reported, since "permission denied" is not
/// a fact about the service.
pub fn inode_owners() -> HashMap<u64, (u32, String)> {
    let mut map = HashMap::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return map;
    };
    for entry in procs.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        let mut name = None;
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let t = target.to_string_lossy();
            let Some(inode) = t
                .strip_prefix("socket:[")
                .and_then(|r| r.strip_suffix(']'))
                .and_then(|n| n.parse::<u64>().ok())
            else {
                continue;
            };
            let comm = name.get_or_insert_with(|| {
                std::fs::read_to_string(entry.path().join("comm"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default()
            });
            map.insert(inode, (pid, comm.clone()));
        }
    }
    map
}

/// The full command line, for telling three python processes apart.
fn cmdline(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let s = String::from_utf8_lossy(&raw)
        .split('\0')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!s.trim().is_empty()).then_some(s)
}

/// A service the operator could plausibly add to their desktop.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub port: u16,
    /// The process holding the socket, when it belongs to this user.
    pub pid: Option<u32>,
    pub process: Option<String>,
    pub command: Option<String>,
    /// Whether anything answered an HTTP request on loopback.
    pub http: bool,
    /// Whether it needed TLS.
    pub tls: bool,
    /// `<title>` of the response, which is usually the app's own name for
    /// itself and a far better default than the process name.
    pub title: Option<String>,
    /// Already configured as a facet, so the UI can show it as claimed rather
    /// than offer to add it twice.
    pub known: bool,
}

/// Ports that are never interesting as desktop apps.
///
/// Not a blocklist of applications — that would recreate the problem this
/// module exists to solve — but of *protocols* that cannot be a web app:
/// resolvers, mail, SSH and the like. Anything not named here is offered.
fn is_infrastructure(port: u16) -> bool {
    matches!(port, 22 | 25 | 53 | 111 | 123 | 137..=139 | 445 | 465 | 587 | 631 | 993 | 995)
}

/// Extract a `<title>`, tolerating attributes and newlines.
pub fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let end = lower[open_end..].find("</title>")? + open_end;
    let raw = html[open_end..end].trim();
    // Entities that actually show up in page titles. A full decoder would be
    // more than this needs.
    let text = raw
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty() && text.len() <= 200).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real excerpt: two listeners and one established connection.
    const TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 41231 1
   1: 00000000:1FF6 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 41232 1
   2: 0100007F:1F90 0100007F:B3A2 01 00000000:00000000 00:00000000 00000000  1000        0 41233 1
";

    #[test]
    fn only_listening_sockets_are_reported() {
        // An established connection shares the file and would otherwise be
        // reported as a service.
        let l = parse_listeners(TCP);
        assert_eq!(l.len(), 2, "{l:?}");
        assert!(l.iter().all(|x| x.inode != 41233));
    }

    #[test]
    fn addresses_are_byte_swapped_back() {
        // 0100007F is 127.0.0.1 written in host order, not 1.0.0.127.
        let l = parse_listeners(TCP);
        assert_eq!(l[0].addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(l[0].port, 8080);
        assert_eq!(l[1].addr, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(l[1].port, 8182);
    }

    #[test]
    fn ipv6_loopback_parses() {
        let hex = "00000000000000000000000001000000";
        assert_eq!(parse_hex_addr(hex), Some(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn a_malformed_table_yields_nothing_rather_than_panicking() {
        // /proc is not a stable API contract; a short line must not take the
        // daemon down.
        assert!(parse_listeners("header\nnonsense\n1: x\n").is_empty());
        assert!(parse_listeners("").is_empty());
    }

    #[test]
    fn titles_survive_attributes_entities_and_newlines() {
        assert_eq!(extract_title("<title>ComfyUI</title>").as_deref(), Some("ComfyUI"));
        assert_eq!(
            extract_title("<TITLE lang=\"en\">\n  Syncthing &amp; friends\n</TITLE>").as_deref(),
            Some("Syncthing & friends")
        );
        assert_eq!(extract_title("<title>   </title>"), None);
        assert_eq!(extract_title("<p>no title here</p>"), None);
        // Unterminated: a truncated body must not yield the rest of the page.
        assert_eq!(extract_title("<title>oops"), None);
    }

    #[test]
    fn protocols_that_cannot_be_web_apps_are_excluded() {
        assert!(is_infrastructure(22) && is_infrastructure(53) && is_infrastructure(445));
        // Application ports must not be, or the sweep stops generalising.
        assert!(!is_infrastructure(8188) && !is_infrastructure(8384) && !is_infrastructure(3000));
    }

    #[test]
    fn this_machine_is_listening_on_something() {
        // Guards the read path and the field order together: parsing that
        // silently returns nothing would pass every fixture test above.
        let raw = std::fs::read_to_string("/proc/net/tcp").expect("procfs");
        let l = parse_listeners(&raw);
        assert!(!l.is_empty(), "no listeners found on a running machine");
        assert!(l.iter().all(|x| x.port != 0));
    }
}

/// Ask a port whether it speaks HTTP, and what it calls itself.
///
/// Plain first, then TLS, matching the proxy's own discovery order: most
/// self-hosted apps are plain on loopback, so the common case costs one
/// request. The body is capped because this is a probe, not a download — a
/// port serving a disk image should not be read into memory to look for a
/// `<title>` that is not there.
async fn probe(state: &AppState, port: u16) -> Option<(bool, Option<String>)> {
    const CAP: usize = 64 * 1024;

    async fn read_capped(body: axum::body::Body) -> String {
        let bytes = match body.collect().await {
            Ok(c) => c.to_bytes(),
            Err(_) => return String::new(),
        };
        String::from_utf8_lossy(&bytes[..bytes.len().min(CAP)]).into_owned()
    }

    let uri = |scheme: &str| -> Option<hyper::Uri> {
        format!("{scheme}://127.0.0.1:{port}/").parse().ok()
    };

    if let Some(u) = uri("http")
        && let Ok(Ok(res)) = tokio::time::timeout(
            std::time::Duration::from_millis(900),
            state.proxy.get(u),
        )
        .await
    {
        let html = read_capped(axum::body::Body::new(res.into_body())).await;
        return Some((false, extract_title(&html)));
    }

    if let Some(u) = uri("https")
        && let Ok(Ok(res)) = tokio::time::timeout(
            std::time::Duration::from_millis(1200),
            state.proxy_tls.get(u),
        )
        .await
    {
        let html = read_capped(axum::body::Body::new(res.into_body())).await;
        return Some((true, extract_title(&html)));
    }

    None
}

/// Everything on this host that could plausibly become an app.
pub async fn sweep(state: &AppState, own_port: u16) -> Vec<Candidate> {
    let mut listeners = Vec::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(raw) = std::fs::read_to_string(f) {
            listeners.extend(parse_listeners(&raw));
        }
    }

    let claimed: std::collections::HashSet<u16> = state
        .facets
        .read()
        .expect("facets poisoned")
        .iter()
        .filter_map(|f| f.expose.as_ref().map(|e| e.port))
        .collect();

    // One entry per port: a service bound to both stacks, or to several
    // addresses, is one app and should be offered once.
    let mut ports: Vec<(u16, u64)> = Vec::new();
    for l in listeners {
        if l.port == own_port || is_infrastructure(l.port) {
            continue;
        }
        if !ports.iter().any(|(p, _)| *p == l.port) {
            ports.push((l.port, l.inode));
        }
    }
    ports.sort_by_key(|(p, _)| *p);

    let owners = inode_owners();

    // Probes run together: a dozen ports each waiting up to two seconds in
    // series would be a visibly slow interface.
    let probes = ports.into_iter().map(|(port, inode)| async move {
        let (http, tls, title) = match probe(state, port).await {
            Some((tls, title)) => (true, tls, title),
            None => (false, false, None),
        };
        (port, inode, http, tls, title)
    });
    let results = futures_util::future::join_all(probes).await;

    results
        .into_iter()
        // A port that does not answer HTTP cannot be shown in a window, and
        // offering it would only produce an app that never loads.
        .filter(|(_, _, http, _, _)| *http)
        .map(|(port, inode, http, tls, title)| {
            let owner = owners.get(&inode);
            Candidate {
                port,
                pid: owner.map(|(p, _)| *p),
                process: owner.map(|(_, c)| c.clone()),
                command: owner.and_then(|(p, _)| cmdline(*p)),
                http,
                tls,
                title,
                known: claimed.contains(&port),
            }
        })
        .collect()
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/discover", get(list))
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    use axum::response::IntoResponse;
    // What is running is exactly the kind of inventory that should not leak to
    // a device holding only a device token.
    if let Some(denied) = crate::api::require(&state, &headers, prism_core::auth::Sensitivity::Session) {
        return denied;
    }
    let own = state.port;
    Json(sweep(&state, own).await).into_response()
}
