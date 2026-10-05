//! **The private overlay network**: whichever mesh joins the operator's
//! devices, whoever makes it. Tailscale, a self-hosted Headscale, NetBird,
//! ZeroTier, Nebula or plain WireGuard all look the same from here: an
//! interface carrying an address only the operator's devices can reach.
//! Prism binds to that, never to one vendor's CLI, so swapping the overlay
//! changes nothing above this file.
//!
//! Found by the interface's name (each overlay names its own), or failing that
//! by an address in the shared range most of them use (100.64.0.0/10, RFC 6598)
//! on an interface that isn't a physical link. Commercial VPN tunnels (`tun*`)
//! are deliberately not overlays: their far side is a provider's network, not
//! the operator's devices.

use std::net::IpAddr;

/// Interface-name prefixes of overlays, as each names its device.
pub const KNOWN: &[(&str, &str)] = &[
    ("tailscale", "Tailscale or Headscale"),
    ("zt", "ZeroTier"),
    ("wt", "NetBird"),
    ("netbird", "NetBird"),
    ("nebula", "Nebula"),
    ("wg", "WireGuard"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    pub interface: String,
    pub address: IpAddr,
    /// Which overlay, as far as the name tells.
    pub kind: &'static str,
}

/// Is this address on this interface an overlay?
pub fn classify(interface: &str, address: IpAddr) -> Option<&'static str> {
    if !reachable(address) { return None }
    if let Some((_, kind)) = KNOWN.iter().find(|(p, _)| interface.starts_with(p)) { return Some(kind) }
    let physical = ["en", "eth", "wl", "ww", "lo", "docker", "br-", "virbr", "veth", "tun", "tap"].iter().any(|p| interface.starts_with(p));
    match address {
        IpAddr::V4(v4) if !physical && v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64 => Some("an overlay"),
        _ => None,
    }
}

/// Could another device reach this address at all?
fn reachable(address: IpAddr) -> bool {
    if address.is_loopback() || address.is_unspecified() { return false }
    // Link-local addresses aren't reachable across an overlay.
    !matches!(address, IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80)
}

/// Every interface address on this machine.
pub fn addresses() -> Vec<(String, IpAddr)> {
    let mut out = Vec::new();
    // SAFETY: getifaddrs allocates a list we walk read-only and free once.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 { return out }
        let mut cur = head;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() && !ifa.ifa_name.is_null() {
                let name = std::ffi::CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
                match (*ifa.ifa_addr).sa_family as i32 {
                    libc::AF_INET => {
                        let a = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                        out.push((name, IpAddr::from(u32::from_be(a.sin_addr.s_addr).to_be_bytes())));
                    }
                    libc::AF_INET6 => {
                        let a = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                        out.push((name, IpAddr::from(a.sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

/// The overlay addresses here: IPv4 first (every overlay has one; not every
/// client has IPv6). [interface], when the operator named one, is the only
/// interface considered, whatever its name.
pub fn find(interface: Option<&str>) -> Vec<Overlay> {
    let mut found: Vec<Overlay> = addresses().into_iter().filter_map(|(i, a)| {
        let kind = match interface {
            Some(want) => (i == want && reachable(a)).then_some("the named interface"),
            None => classify(&i, a),
        }?;
        Some(Overlay { interface: i, address: a, kind })
    }).collect();
    found.sort_by_key(|o| (o.address.is_ipv6(), o.interface.clone()));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_overlay_is_recognised_and_nothing_else() {
        let v4 = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(classify("tailscale0", v4("100.109.171.51")), Some("Tailscale or Headscale"));
        assert_eq!(classify("ztks5abc12", v4("10.147.17.4")), Some("ZeroTier"));
        assert_eq!(classify("wt0", v4("100.92.1.2")), Some("NetBird"));
        assert_eq!(classify("wg0", v4("10.8.0.2")), Some("WireGuard"));
        assert_eq!(classify("nebula1", v4("192.168.100.5")), Some("Nebula"));
        // An unknown name in the shared overlay range: still an overlay.
        assert_eq!(classify("mesh0", v4("100.70.0.9")), Some("an overlay"));
        // The LAN, Wi-Fi, containers, a commercial VPN: never.
        for (i, a) in [("enp5s0", "10.200.187.87"), ("wlan0", "10.130.5.3"), ("docker0", "172.17.0.1"), ("tun0", "100.64.0.2"), ("lo", "127.0.0.1")] {
            assert_eq!(classify(i, v4(a)), None, "{i}");
        }
        // Link-local v6 on an overlay can't be reached across it.
        assert_eq!(classify("tailscale0", v4("fe80::1")), None);
        assert!(classify("tailscale0", v4("fd7a:115c:a1e0::1")).is_some());
    }

    #[test]
    fn this_machines_addresses_are_read() {
        assert!(addresses().iter().any(|(_, a)| a.is_loopback()));
    }
}
