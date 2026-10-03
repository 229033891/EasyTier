use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use url::Url;

use crate::{
    peers::peer_manager::PeerManagerCore,
    proto::common::{StunInfo, TunnelInfo, Url as ProtoUrl},
};

/// Underlay destinations that must keep a more-specific host route when the
/// local exit-node installs `0.0.0.0/0` on TUN (peer tunnels, STUN publics,
/// and any extra candidates such as the config-server management plane).
pub async fn collect_underlay_exclude_ips(
    peer_manager: &PeerManagerCore,
    extra_candidates: impl IntoIterator<Item = IpAddr>,
) -> BTreeSet<IpAddr> {
    let mut ips = BTreeSet::new();

    for snapshot in peer_manager.list_peer_snapshots().await {
        for conn in snapshot.conns {
            if let Some(ip) = tunnel_underlay_ip(conn.tunnel.as_ref()) {
                if should_exclude_ip(peer_manager, ip).await {
                    ips.insert(ip);
                }
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
    if !is_global_underlay_ip(ip) {
        return false;
    }
    if peer_manager.is_local_virtual_ip(&ip) {
        return false;
    }
    if let IpAddr::V6(v6) = ip
        && peer_manager.is_easytier_managed_ipv6(&v6).await
    {
        return false;
    }
    true
}

fn is_global_underlay_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_global_ipv4(ip),
        IpAddr::V6(ip) => is_global_ipv6(ip),
    }
}

fn is_global_ipv4(ip: Ipv4Addr) -> bool {
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_link_local()
        || ip.is_private()
        || ip.is_documentation())
}

fn is_global_ipv6(ip: Ipv6Addr) -> bool {
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || ip.is_unicast_link_local()
        || ip.is_unique_local())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_vip_like_addresses_are_not_global() {
        assert!(!is_global_ipv4("10.0.0.1".parse().unwrap()));
        assert!(!is_global_ipv4("192.168.1.1".parse().unwrap()));
        assert!(!is_global_ipv4("127.0.0.1".parse().unwrap()));
        assert!(is_global_ipv4("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn global_ipv6_filter_matches_underlay_policy() {
        assert!(!is_global_ipv6("fe80::1".parse().unwrap()));
        assert!(!is_global_ipv6("fd00::1".parse().unwrap()));
        assert!(is_global_ipv6("2001:db8::1".parse().unwrap()));
    }
}
