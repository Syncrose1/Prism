//! Resolving where the API listens.
//!
//! The default binds the private overlay's address specifically (whichever
//! overlay: `prism_core::platform::overlay`), not `0.0.0.0`, plus loopback.
//! That makes the network boundary and the auth boundary independent failure
//! domains: a mistake in the auth code does not expose Prism to the local
//! network or the internet, and an overlay ACL mistake still meets TOTP.
//!
//! Binding a wildcard is possible but never silent — it warns, because a remote
//! management interface reachable from a café network is a materially different
//! product from one reachable only from the operator's own devices.

use prism_core::config::BindMode;
use prism_core::platform::overlay;
use std::net::{IpAddr, SocketAddr};
use tracing::{info, warn};

/// Where to listen now, and whether an overlay is still to come.
pub struct Plan {
    pub now: Vec<SocketAddr>,
    /// The overlay (or the named interface) isn't up yet: keep looking, and
    /// listen there the moment it is. Fixes the race where Prism started
    /// before the overlay had an address and served only this machine for
    /// the rest of its life while reporting itself healthy.
    pub awaiting: Option<BindMode>,
}

fn loopback(port: u16) -> SocketAddr { SocketAddr::from(([127, 0, 0, 1], port)) }

/// The overlay addresses for [mode] right now (empty when it isn't up).
pub fn overlay_addrs(mode: &BindMode, port: u16) -> Vec<SocketAddr> {
    let named = match mode { BindMode::Interface(i) => Some(i.as_str()), _ => None };
    overlay::find(named).into_iter().map(|o| {
        info!(interface = %o.interface, address = %o.address, kind = o.kind, "overlay");
        SocketAddr::new(o.address, port)
    }).collect()
}

pub fn resolve(mode: &BindMode, port: u16) -> anyhow::Result<Plan> {
    match mode {
        BindMode::Localhost => Ok(Plan { now: vec![loopback(port)], awaiting: None }),
        BindMode::Address(addr) => {
            if mode.is_wildcard() {
                warn!(
                    %addr,
                    "binding a wildcard address: Prism will be reachable beyond the \
                     overlay. Auth is now the only boundary."
                );
            }
            let ip: IpAddr = addr
                .parse()
                .map_err(|_| anyhow::anyhow!("`{addr}` is not a valid IP address"))?;
            let mut now = vec![SocketAddr::new(ip, port)];
            if !ip.is_loopback() && !ip.is_unspecified() { now.push(loopback(port)); }
            Ok(Plan { now, awaiting: None })
        }
        BindMode::Overlay | BindMode::Interface(_) => {
            let mut now = overlay_addrs(mode, port);
            let awaiting = if now.is_empty() {
                warn!("no overlay network is up yet; serving this machine only until one is");
                Some(mode.clone())
            } else { None };
            now.push(loopback(port));
            Ok(Plan { now, awaiting })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_binds_loopback() {
        let addr = resolve(&BindMode::Localhost, 9000).unwrap().now[0];
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 9000);
    }

    #[test]
    fn explicit_address_is_honoured() {
        let addr = resolve(&BindMode::Address("100.64.0.1".into()), 9000).unwrap().now[0];
        assert_eq!(addr.to_string(), "100.64.0.1:9000");
    }

    #[test]
    fn invalid_address_is_an_error_not_a_wildcard_fallback() {
        // Falling back to 0.0.0.0 on a typo would silently publish the service.
        assert!(resolve(&BindMode::Address("not-an-ip".into()), 9000).is_err());
    }

    #[test]
    fn overlay_mode_never_yields_a_wildcard_and_always_serves_this_machine() {
        // Whatever overlay is or isn't up: specific addresses only, loopback
        // among them (POLARIS's bridge), and an overlay awaited if none.
        let plan = resolve(&BindMode::Overlay, 9000).unwrap();
        assert!(plan.now.iter().all(|a| !a.ip().is_unspecified()));
        assert!(plan.now.contains(&loopback(9000)));
        assert_eq!(plan.awaiting.is_some(), plan.now.len() == 1);
        let missing = resolve(&BindMode::Interface("no-such-if0".into()), 9000).unwrap();
        assert_eq!((missing.now, missing.awaiting), (vec![loopback(9000)], Some(BindMode::Interface("no-such-if0".into()))));
    }
}
