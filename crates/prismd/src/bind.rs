//! Resolving where the API listens.
//!
//! The default binds the host's Tailscale address specifically, not `0.0.0.0`.
//! That makes the network boundary and the auth boundary independent failure
//! domains: a mistake in the auth code does not expose Prism to the local
//! network or the internet, and a Tailscale ACL mistake still meets TOTP.
//!
//! Binding a wildcard is possible but never silent — it warns, because a remote
//! management interface reachable from a café network is a materially different
//! product from one reachable only from the operator's own devices.

use prism_core::config::BindMode;
use std::net::{IpAddr, SocketAddr};
use tracing::{info, warn};

/// Resolve a bind mode to a concrete socket address.
///
/// Falls back to loopback when the tailnet address cannot be determined. That is
/// deliberately conservative: an unreachable Prism is a nuisance, whereas one
/// that quietly binds every interface because Tailscale was down is a hazard.
pub fn resolve(mode: &BindMode, port: u16) -> anyhow::Result<SocketAddr> {
    match mode {
        BindMode::Localhost => Ok(SocketAddr::from(([127, 0, 0, 1], port))),
        BindMode::Address(addr) => {
            if mode.is_wildcard() {
                warn!(
                    %addr,
                    "binding a wildcard address: Prism will be reachable beyond the \
                     tailnet. Auth is now the only boundary."
                );
            }
            let ip: IpAddr = addr
                .parse()
                .map_err(|_| anyhow::anyhow!("`{addr}` is not a valid IP address"))?;
            Ok(SocketAddr::new(ip, port))
        }
        BindMode::Tailscale => match tailscale_ip() {
            Some(ip) => {
                info!(%ip, "binding tailnet interface");
                Ok(SocketAddr::new(ip, port))
            }
            None => {
                warn!(
                    "could not determine a Tailscale address; falling back to \
                     localhost. Prism will only be reachable from this machine."
                );
                Ok(SocketAddr::from(([127, 0, 0, 1], port)))
            }
        },
    }
}

/// Ask the Tailscale CLI for this host's tailnet IPv4 address.
fn tailscale_ip() -> Option<IpAddr> {
    let output = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_binds_loopback() {
        let addr = resolve(&BindMode::Localhost, 9000).unwrap();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 9000);
    }

    #[test]
    fn explicit_address_is_honoured() {
        let addr = resolve(&BindMode::Address("100.64.0.1".into()), 9000).unwrap();
        assert_eq!(addr.to_string(), "100.64.0.1:9000");
    }

    #[test]
    fn invalid_address_is_an_error_not_a_wildcard_fallback() {
        // Falling back to 0.0.0.0 on a typo would silently publish the service.
        assert!(resolve(&BindMode::Address("not-an-ip".into()), 9000).is_err());
    }

    #[test]
    fn tailscale_mode_never_yields_a_wildcard() {
        // Whether or not tailscale is present, the result must be a specific
        // address — either the tailnet IP or loopback.
        let addr = resolve(&BindMode::Tailscale, 9000).unwrap();
        assert!(
            !addr.ip().is_unspecified(),
            "tailscale mode must never resolve to 0.0.0.0"
        );
    }
}
