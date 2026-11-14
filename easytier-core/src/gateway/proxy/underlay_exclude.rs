use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

use url::Url;

use crate::{
    peers::peer_manager::PeerManagerCore,
    proto::common::{StunInfo, TunnelInfo, Url as ProtoUrl},
};

/// Underlay destinations that must keep a more-specific host route when TUN
/// installs an effective default route (peer tunnels, STUN publics, and
/// config-server / management-plane targets — including private LAN addresses).
pub async fn collect_underlay_exclude_ips(
    peer_manager: &PeerManagerCore,
    extra_candidates: impl IntoIterator<Item = IpAddr>,
) -> BTreeSet<IpAddr> {
    let mut ips = BTreeSet::new();

    for snapshot in peer_manager.list_peer_snapshots().await {
        for conn in snapshot.conns {
            if let Some(ip) = tunnel_underlay_ip(conn.tunnel.as_ref())
                && should_exclude_ip(peer_manager, ip).await
            {
                ips.insert(ip);
            }
        }
    }

    for route in peer_manager.get_route().list_routes().await {
        if let Some(stun) = route.stun_info.as_ref() {
            for ip in stun_public_ips(stun) {
                if should_exclude_ip(peer_manager, ip).await {
                    ips.insert(ip);
                }
            }
        }
    }

    for ip in extra_candidates {
        if should_exclude_ip(peer_manager, ip).await {
            ips.insert(ip);
        }
    }

    ips
}

fn tunnel_underlay_ip(tunnel: Option<&TunnelInfo>) -> Option<IpAddr> {
    let tunnel = tunnel?;
    parse_url_host_ip(tunnel.resolved_remote_addr.as_ref())
        .or_else(|| parse_url_host_ip(tunnel.remote_addr.as_ref()))
}

fn parse_url_host_ip(url: Option<&ProtoUrl>) -> Option<IpAddr> {
    let url = Url::from(url?.clone());
    match url.host()? {
        url::Host::Ipv4(ip) => Some(IpAddr::V4(ip)),
        url::Host::Ipv6(ip) => Some(IpAddr::V6(ip)),
        url::Host::Domain(host) => host.parse().ok(),
    }
}

fn stun_public_ips(stun: &StunInfo) -> impl Iterator<Item = IpAddr> + '_ {
    stun.public_ip.iter().filter_map(|value| value.parse().ok())
}

async fn should_exclude_ip(peer_manager: &PeerManagerCore, ip: IpAddr) -> bool {
    if !is_excludable_underlay_ip(ip) {
        return false;
    }
    // Overlay VIP / managed addresses must keep using TUN, not the physical default.
    if peer_manager.is_local_virtual_ip(&ip) {
        return false;
    }
    if let IpAddr::V6(v6) = ip
        && peer_manager.is_easytier_managed_ipv6(&v6).await
    {
        return false;
    }
    // G1 (docs/roadmap/underlay-exclude-onlink-filter.md §4 F1): an address
    // inside a local physical subnet already has a more-specific connected
    // route — pinning a `/32` via the physical default is pure degradation
    // (LAN traffic hairpins through the gateway) and outlives the process
    // (cleanup misses it once the desired set drains). Filter at collection
    // time so the install side never pins on-link routes at all; the stop-
    // time cleanup derives from the installer's published accounting (F2),
    // not from this set. IPv6 has no local-net injection yet, so it keeps
    // the old behavior (F3's territory).
    if let IpAddr::V4(v4) = ip {
        let local_nets = peer_manager.local_ipv4_networks();
        // `None` = host never injected nets (enumeration failed / non-desktop
        // runtime): stay conservative and keep the current behavior rather
        // than dropping every exclude on hosts we know nothing about.
        if is_on_link_ipv4(v4, local_nets_slice(&local_nets)) {
            return false;
        }
    }
    true
}

/// Borrow the injected physical-subnet snapshot as a slice.
///
/// Uses `Arc::deref` explicitly: `Arc::as_ref` / `Option::as_ref` are
/// ambiguous on this type, and the earlier `as_ref().map(...)` form moved out
/// of the shared reference. `Deref` → `Option::as_deref` is deterministic
/// and keeps clippy's `manual_map` untriggered.
fn local_nets_slice(local_nets: &Arc<Option<Vec<cidr::Ipv4Cidr>>>) -> Option<&[cidr::Ipv4Cidr]> {
    use std::ops::Deref as _;
    local_nets.deref().as_deref()
}

/// Pure on-link predicate for the collection filter.
///
/// `None` means "physical nets unknown" → **not** on-link (conservative:
/// nothing is filtered, current behavior preserved). `Some` means the host
/// injected a snapshot — an address inside any of those subnets is on-link.
fn is_on_link_ipv4(ip: Ipv4Addr, local_nets: Option<&[cidr::Ipv4Cidr]>) -> bool {
    match local_nets {
        None => false,
        Some(nets) => nets.iter().any(|net| net.contains(&ip)),
    }
}

/// Host routes are useful for both public and RFC1918/ULA underlay peers and
/// management consoles. Reject only addresses that are never valid tunnel or
/// console endpoints on the physical path.
fn is_excludable_underlay_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_excludable_ipv4(ip),
        IpAddr::V6(ip) => is_excludable_ipv6(ip),
    }
}

fn is_excludable_ipv4(ip: Ipv4Addr) -> bool {
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_link_local()
        || ip.is_documentation())
}

fn is_excludable_ipv6(ip: Ipv6Addr) -> bool {
    !(ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() || ip.is_unicast_link_local())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_lan_addresses_are_excludable() {
        assert!(is_excludable_ipv4("10.0.0.1".parse().unwrap()));
        assert!(is_excludable_ipv4("192.168.1.1".parse().unwrap()));
        assert!(is_excludable_ipv4("172.16.5.5".parse().unwrap()));
        assert!(is_excludable_ipv4("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn non_forwardable_ipv4_is_not_excludable() {
        assert!(!is_excludable_ipv4("127.0.0.1".parse().unwrap()));
        assert!(!is_excludable_ipv4("0.0.0.0".parse().unwrap()));
        assert!(!is_excludable_ipv4("169.254.1.1".parse().unwrap()));
        assert!(!is_excludable_ipv4("224.0.0.1".parse().unwrap()));
    }

    #[test]
    fn ula_ipv6_is_excludable_but_link_local_is_not() {
        assert!(is_excludable_ipv6("fd00::1".parse().unwrap()));
        assert!(is_excludable_ipv6("2001:db8::1".parse().unwrap()));
        assert!(!is_excludable_ipv6("fe80::1".parse().unwrap()));
        assert!(!is_excludable_ipv6("::1".parse().unwrap()));
    }

    #[test]
    fn on_link_ipv4_is_filtered_only_when_local_nets_are_known() {
        let net: cidr::Ipv4Cidr = "192.168.10.0/24".parse().unwrap();
        let nets = [net];
        assert!(is_on_link_ipv4(
            "192.168.10.101".parse().unwrap(),
            Some(&nets)
        ));
        assert!(!is_on_link_ipv4(
            "172.18.10.10".parse().unwrap(),
            Some(&nets)
        ));
        // Unknown physical nets ⇒ conservative: nothing is filtered.
        assert!(!is_on_link_ipv4("192.168.10.101".parse().unwrap(), None));
    }
}
