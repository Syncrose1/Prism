//! The graceful hook: asking a workload to give something back.
//!
//! Everything else Prism can do to a facet is violent. `memory.max` truncates
//! an allocation, `SIGTERM` ends the process, a cgroup kill ends all of them at
//! once. Those are the right instruments for a runaway, and the wrong ones for
//! the case this file exists for: a workload that is behaving correctly, is
//! holding a resource somebody else now needs, and could give part of it back
//! if anyone asked.
//!
//! That case is not hypothetical — it is the normal state of a 12 GiB card. A
//! resident model stack and a working desktop do not both fit, so the question
//! is never *whether* to give VRAM back but *how much*, and killing the stack
//! to answer it loses the several seconds of model load that made it worth
//! keeping resident in the first place.
//!
//! ## What the hook is, and what it deliberately is not
//!
//! It is one HTTP POST to an endpoint the workload already has, with the
//! pressure figures substituted into the body. It is **not** a protocol, and
//! Prism does not interpret the response beyond "the workload answered".
//!
//! The reason is a division of knowledge. Prism knows how much has to go. It
//! has no idea what any given workload can afford to lose, and any scheme where
//! it decided that — unload models, drop caches, reduce batch — would be Prism
//! guessing at the internals of software it did not write. ComfyUI already
//! knows that `/free` means its checkpoints; a voice stack knows which of its
//! three models it can survive without. So Prism states the deficit and the
//! workload chooses the concession.
//!
//! ## Why it is blocking, on its own thread
//!
//! The monitor loop is synchronous by design, so that pressure handling never
//! waits on an async runtime which is itself being starved. A hook with a ten
//! second timeout must therefore not be called on the monitor thread — one
//! unresponsive facet would stall sensing for every other. `fire` is blocking
//! and `spawn` hands it to a detached thread, so a tick costs a thread spawn
//! and never a timeout.
//!
//! ## Plain HTTP only
//!
//! No TLS, and that is a statement about who this addresses rather than an
//! omission: the endpoint belongs to a workload Prism itself started, on this
//! machine, reachable on loopback. A graceful hook that could reach an
//! arbitrary HTTPS host would be an egress channel that fires automatically
//! under load, which is not a thing a resilience daemon should grow by
//! accident.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};

use crate::graceful_config::Graceful;
use crate::governor::Tier;

/// What the workload is being told. Substituted into the configured body.
#[derive(Debug, Clone, Default)]
pub struct Deficit {
    pub facet: String,
    pub tier: Tier,
    /// MiB of VRAM the workload is over its budget, if a GPU is sensed.
    pub vram_overdraft_mib: Option<u64>,
    /// MiB of VRAM it may hold, if a GPU is sensed.
    pub vram_budget_mib: Option<u64>,
    /// MiB of honest host headroom remaining.
    pub headroom_mib: u64,
}

impl Deficit {
    /// Fill `{placeholder}` spans in a configured body.
    ///
    /// Substitution rather than JSON construction, because the body is the
    /// workload's own API shape and Prism has no business knowing it. A facet
    /// whose endpoint takes form encoding or a bare integer is served by the
    /// same mechanism as one taking JSON.
    ///
    /// An unsensed figure renders as `null` rather than as zero. Zero would
    /// read as "you owe nothing", which is a claim, where absent VRAM sensing
    /// means Prism has no idea.
    pub fn render(&self, template: &str) -> String {
        let mut out = template.to_string();
        for (key, value) in [
            ("{facet}", self.facet.clone()),
            ("{tier}", self.tier.as_str().to_string()),
            ("{vram_overdraft_mib}", opt(self.vram_overdraft_mib)),
            ("{vram_budget_mib}", opt(self.vram_budget_mib)),
            ("{headroom_mib}", self.headroom_mib.to_string()),
        ] {
            if out.contains(key) {
                out = out.replace(key, &value);
            }
        }
        out
    }
}

fn opt(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "null".into())
}

/// Post the hook and wait for a response. Blocking; see the module note.
pub fn fire(hook: &Graceful, deficit: &Deficit) -> anyhow::Result<u16> {
    let (host, port, path) = split_url(&hook.http_post.url)?;
    let body = deficit.render(&hook.http_post.body);
    let timeout = hook.timeout();

    let addr = (host.as_str(), port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("{host}:{port} did not resolve"))?;

    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    // `Connection: close` so the read ends at EOF and no response needs to be
    // length-parsed. The hook is one request; keep-alive would only add a state
    // machine to maintain.
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}:{port}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        hook.content_type(),
        body.len(),
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    // Only the status line is read. The body is the workload's own business and
    // could be megabytes; capping the read also means a facet cannot stall the
    // hook thread by dribbling bytes forever.
    let mut buf = [0u8; 512];
    let n = stream.read(&mut buf).unwrap_or(0);
    status_of(&buf[..n]).ok_or_else(|| anyhow::anyhow!("no status line in reply"))
}

/// Fire on a detached thread, logging the outcome. Never blocks the caller.
pub fn spawn(facet_id: String, hook: Graceful, deficit: Deficit) {
    std::thread::spawn(move || match fire(&hook, &deficit) {
        Ok(code) if (200..400).contains(&code) => {
            tracing::info!(facet = %facet_id, tier = deficit.tier.as_str(), code, "facet yielded")
        }
        Ok(code) => {
            tracing::warn!(facet = %facet_id, code, "graceful hook refused")
        }
        Err(e) => {
            // Not an error for the machine: a facet that cannot be asked
            // nicely is simply one the governor will have to constrain by
            // other means, which is the behaviour with no hook configured.
            tracing::warn!(facet = %facet_id, error = %e, "graceful hook failed")
        }
    });
}

fn status_of(bytes: &[u8]) -> Option<u16> {
    let head = std::str::from_utf8(bytes).ok()?;
    head.split_whitespace().nth(1)?.parse().ok()
}

/// Split `http://host:port/path` into its parts.
///
/// Hand-rolled rather than pulling a URL crate for one shape. Rejects anything
/// that is not plain HTTP, so a misconfigured `https://` fails loudly at the
/// hook rather than silently posting cleartext to a TLS port.
fn split_url(url: &str) -> anyhow::Result<(String, u16, String)> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow::anyhow!("graceful hooks are plain HTTP only: {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(80)),
        None => (authority.to_string(), 80),
    };
    if host.is_empty() {
        anyhow::bail!("no host in {url}");
    }
    Ok((host, port, path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graceful_config::HttpPost;

    fn deficit() -> Deficit {
        Deficit {
            facet: "stargazer".into(),
            tier: Tier::Red,
            vram_overdraft_mib: Some(2560),
            vram_budget_mib: Some(4096),
            headroom_mib: 9000,
        }
    }

    #[test]
    fn the_documented_comfyui_body_passes_through_untouched() {
        // The body in docs/architecture.md has no placeholders. It must arrive
        // exactly as written, or the documented example would be broken by the
        // machinery added to generalise it.
        let body = r#"{"unload_models":true,"free_memory":true}"#;
        assert_eq!(deficit().render(body), body);
    }

    #[test]
    fn placeholders_carry_the_deficit() {
        let rendered = deficit().render(
            r#"{"tier":"{tier}","give_back_mib":{vram_overdraft_mib},"keep_mib":{vram_budget_mib}}"#,
        );
        assert_eq!(
            rendered,
            r#"{"tier":"red","give_back_mib":2560,"keep_mib":4096}"#
        );
    }

    #[test]
    fn an_unsensed_figure_is_null_not_zero() {
        // Zero says "you owe nothing", which is a claim. No GPU means Prism
        // does not know, and the workload should be able to tell the two apart.
        let d = Deficit { vram_overdraft_mib: None, ..deficit() };
        assert_eq!(d.render("{vram_overdraft_mib}"), "null");
    }

    #[test]
    fn urls_split_with_and_without_a_port_or_path() {
        assert_eq!(
            split_url("http://127.0.0.1:8188/free").unwrap(),
            ("127.0.0.1".into(), 8188, "/free".into())
        );
        assert_eq!(
            split_url("http://localhost/x").unwrap(),
            ("localhost".into(), 80, "/x".into())
        );
        assert_eq!(
            split_url("http://127.0.0.1:9000").unwrap(),
            ("127.0.0.1".into(), 9000, "/".into())
        );
    }

    #[test]
    fn https_is_refused_rather_than_sent_in_the_clear() {
        assert!(split_url("https://example.com/free").is_err());
    }

    #[test]
    fn a_status_line_is_read_from_a_partial_response() {
        assert_eq!(status_of(b"HTTP/1.1 200 OK\r\nContent-Len"), Some(200));
        assert_eq!(status_of(b"HTTP/1.1 503 Service Unavailable\r\n"), Some(503));
        assert_eq!(status_of(b""), None);
    }

    #[test]
    fn a_hook_that_cannot_be_reached_is_an_error_and_not_a_panic() {
        // Port 1 on loopback: nothing listens, and the governor must be able to
        // carry on governing a facet that ignores it.
        let hook = Graceful {
            http_post: HttpPost {
                url: "http://127.0.0.1:1/free".into(),
                body: "{}".into(),
                content_type: None,
            },
            timeout: "1s".into(),
        };
        assert!(fire(&hook, &deficit()).is_err());
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::graceful_config::{Graceful, HttpPost};

    /// Post a real hook at Contract's real endpoint. Ignored by default: it
    /// needs the daemon listening on 8770. Run with `--ignored --nocapture`.
    ///
    /// Worth having as a test rather than a script because both ends of this
    /// are hand-rolled HTTP written in the same afternoon, and two homemade
    /// implementations agreeing with each other is exactly the thing that
    /// cannot be assumed.
    #[test]
    #[ignore]
    fn posts_to_contract() {
        let hook = Graceful {
            http_post: HttpPost {
                url: "http://127.0.0.1:8770/yield".into(),
                body: r#"{"tier":"{tier}","give_back_mib":{vram_overdraft_mib},"keep_mib":{vram_budget_mib}}"#.into(),
                content_type: None,
            },
            timeout: "5s".into(),
        };
        // Black tier with a *roomy* card: the spill case. ComfyUI has already
        // moved its weights into RAM, which relieved the card, so the VRAM
        // arithmetic alone says Stargazer may keep everything. The tier is the
        // only thing carrying the truth, and Contract has to act on it.
        let deficit = Deficit {
            facet: "stargazer".into(),
            tier: Tier::Black,
            vram_overdraft_mib: Some(0),
            vram_budget_mib: Some(20_000),
            headroom_mib: 400,
        };
        println!("sending: {}", deficit.render(&hook.http_post.body));
        let code = fire(&hook, &deficit).expect("contract did not answer");
        println!("contract replied {code}");
        assert_eq!(code, 200);
    }
}
