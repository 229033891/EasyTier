use crate::runtime::state::runtime_state::RuntimeInstanceState;
use ipnet::IpNet;
use std::collections::HashSet;
use std::net::IpAddr;

fn normalize_route_cidr(route: &str) -> Option<String> {
    let normalized = route.split("->").next().unwrap_or(route).trim();
    normalized
        .parse::<IpNet>()
        .ok()
        .map(|network| match network {
            IpNet::V4(net) => net.trunc().to_string(),
            IpNet::V6(net) => net.trunc().to_string(),
        })
        .or_else(|| {
            normalized.parse::<IpAddr>().ok().map(|addr| match addr {
                IpAddr::V4(ip) => format!("{}/32", ip),
                IpAddr::V6(ip) => format!("{}/128", ip),
            })
        })
}

fn simplify_routes(routes: Vec<String>) -> Vec<String> {
    let mut parsed = routes
        .into_iter()
        .filter_map(|route| normalize_route_cidr(&route))
        .filter_map(|route| route.parse::<IpNet>().ok())
        .collect::<Vec<_>>();
    parsed.sort_by(|left, right| {
        left.prefix_len()
            .cmp(&right.prefix_len())
            .then_with(|| left.network().to_string().cmp(&right.network().to_string()))
    });

    let mut simplified = Vec::<IpNet>::new();
    'outer: for route in parsed {
        for existing in &simplified {
            if existing.contains(&route.network()) && existing.prefix_len() <= route.prefix_len() {
                continue 'outer;
            }
        }
        simplified.retain(|existing| {
            !(route.contains(&existing.network()) && route.prefix_len() <= existing.prefix_len())
        });
        simplified.push(route);
    }

    let mut seen = HashSet::new();
    simplified
        .into_iter()
        .map(|route| route.to_string())
        .filter(|route| seen.insert(route.clone()))
        .collect()
}

fn is_default_ipv4_route(route: &str) -> bool {
    normalize_route_cidr(route)
        .and_then(|route| route.parse::<IpNet>().ok())
        .is_some_and(|net| matches!(net, IpNet::V4(v4) if v4.prefix_len() == 0))
}

fn route_vip(route: &crate::runtime::state::runtime_state::RouteView) -> Option<&str> {
    route
        .ipv4
        .as_deref()
        .map(|ip| ip.split('/').next().unwrap_or(ip))
        .filter(|ip| !ip.is_empty())
}

/// Mirror desktop `local_exit_default`: configured exit VIP appears in live
/// routes with a next hop.
fn local_exit_default(instance: &RuntimeInstanceState) -> bool {
    if instance.exit_nodes.is_empty() {
        return false;
    }
    for exit in &instance.exit_nodes {
        let exit_ip = exit.split('/').next().unwrap_or(exit.trim());
        if exit_ip.is_empty() {
            continue;
        }
        for route in &instance.routes {
            if route_vip(route) == Some(exit_ip)
                && route.next_hop_peer_id.is_some_and(|id| id > 0)
            {
                return true;
            }
        }
    }
    false
}

/// Aggregate TUN routes for an instance.
///
/// D+ Phase 1b: peer-advertised `0.0.0.0/0` is dropped unless a configured
/// exit VIP is reachable in live routes, or
/// `allow_peer_default_without_exit` is set. Manual routes are kept as-is.
pub fn aggregate_tun_routes(instance: &RuntimeInstanceState) -> Vec<String> {
    aggregate_tun_routes_with_policy(
        instance,
        local_exit_default(instance),
        instance.allow_peer_default_without_exit,
    )
}

pub fn aggregate_tun_routes_with_policy(
    instance: &RuntimeInstanceState,
    local_exit_default: bool,
    allow_peer_default_without_exit: bool,
) -> Vec<String> {
    let virtual_ipv4_cidr = instance
        .my_node_info
        .as_ref()
        .and_then(|info| info.virtual_ipv4_cidr.clone());
    let runtime_proxy_cidrs = instance
        .routes
        .iter()
        .flat_map(|route| route.proxy_cidrs.iter().cloned())
        .filter(|cidr| {
            local_exit_default
                || allow_peer_default_without_exit
                || !is_default_ipv4_route(cidr)
        })
        .collect::<Vec<_>>();
    let mut raw_routes = Vec::new();

    if let Some(cidr) = virtual_ipv4_cidr.clone() {
        raw_routes.push(cidr);
    }

    raw_routes.extend(instance.manual_routes.iter().cloned());
    // Local proxy CIDRs are advertisements for networks reached through this
    // node. Installing them into the same local TUN would recapture the proxy's
    // own destination sockets instead of using the physical network.
    raw_routes.extend(runtime_proxy_cidrs.iter().cloned());
    if local_exit_default {
        raw_routes.push("0.0.0.0/0".to_string());
    }
    simplify_routes(raw_routes)
}

pub fn aggregate_requested_tun_routes(instances: &[RuntimeInstanceState]) -> Vec<String> {
    let mut aggregated_routes = Vec::new();
    let mut seen_routes = HashSet::new();
    for instance in instances.iter().filter(|instance| instance.tun_required) {
        for route in aggregate_tun_routes(instance) {
            if seen_routes.insert(route.clone()) {
                aggregated_routes.push(route);
            }
        }
    }
    aggregated_routes
}

#[cfg(test)]
mod tests {
    use super::{aggregate_tun_routes, simplify_routes};
    use crate::runtime::state::runtime_state::{RouteView, runtime_instance_from_config_snapshot};
    use easytier::proto::api::manage::NetworkConfig;

    #[test]
    fn simplify_routes_normalizes_deduplicates_and_removes_subnets() {
        let routes = simplify_routes(vec![
            "10.0.0.7".to_string(),
            "10.0.0.0/24".to_string(),
            "10.0.0.42/32->peer-a".to_string(),
            "2001:db8::1".to_string(),
            "2001:db8::/64".to_string(),
        ]);

        assert_eq!(routes, vec!["10.0.0.0/24", "2001:db8::/64"]);
    }

    #[test]
    fn local_proxy_cidr_is_not_installed_in_tun_routes() {
        let mut instance = runtime_instance_from_config_snapshot(
            "routing-test".to_string(),
            "test".to_string(),
            NetworkConfig {
                virtual_ipv4: Some("10.144.144.1".to_string()),
                network_length: Some(24),
                routes: vec!["172.16.0.0/16".to_string()],
                proxy_cidrs: vec!["192.168.1.0/24".to_string()],
                ..Default::default()
            },
            true,
        );
        instance.routes.push(RouteView {
            peer_id: 2,
            hostname: None,
            ipv4: Some("10.144.144.2".to_string()),
            ipv4_cidr: Some("10.144.144.2/24".to_string()),
            ipv6_cidr: None,
            proxy_cidrs: vec!["10.20.0.0/16".to_string()],
            next_hop_peer_id: Some(2),
            cost: Some(1),
            path_latency: None,
            udp_nat_type: None,
            tcp_nat_type: None,
            inst_id: None,
            version: None,
            is_public_server: None,
        });

        let routes = aggregate_tun_routes(&instance);

        assert!(routes.contains(&"10.144.144.0/24".to_string()));
        assert!(routes.contains(&"172.16.0.0/16".to_string()));
        assert!(routes.contains(&"10.20.0.0/16".to_string()));
        assert!(!routes.contains(&"192.168.1.0/24".to_string()));
    }

    #[test]
    fn peer_default_filtered_without_exit() {
        let mut instance = runtime_instance_from_config_snapshot(
            "routing-default".to_string(),
            "test".to_string(),
            NetworkConfig {
                virtual_ipv4: Some("10.144.144.1".to_string()),
                network_length: Some(24),
                ..Default::default()
            },
            true,
        );
        instance.routes.push(RouteView {
            peer_id: 2,
            hostname: None,
            ipv4: Some("10.144.144.2".to_string()),
            ipv4_cidr: Some("10.144.144.2/24".to_string()),
            ipv6_cidr: None,
            proxy_cidrs: vec!["0.0.0.0/0".to_string(), "10.20.0.0/16".to_string()],
            next_hop_peer_id: Some(2),
            cost: Some(1),
            path_latency: None,
            udp_nat_type: None,
            tcp_nat_type: None,
            inst_id: None,
            version: None,
            is_public_server: None,
        });

        let routes = aggregate_tun_routes(&instance);
        assert!(routes.contains(&"10.20.0.0/16".to_string()));
        assert!(!routes.iter().any(|r| r == "0.0.0.0/0"));
    }

    #[test]
    fn peer_default_installed_with_reachable_exit_nodes() {
        let mut instance = runtime_instance_from_config_snapshot(
            "routing-exit".to_string(),
            "test".to_string(),
            NetworkConfig {
                virtual_ipv4: Some("10.144.144.1".to_string()),
                network_length: Some(24),
                exit_nodes: vec!["10.144.144.2".to_string()],
                ..Default::default()
            },
            true,
        );
        instance.routes.push(RouteView {
            peer_id: 2,
            hostname: None,
            ipv4: Some("10.144.144.2".to_string()),
            ipv4_cidr: Some("10.144.144.2/24".to_string()),
            ipv6_cidr: None,
            proxy_cidrs: vec!["10.20.0.0/16".to_string()],
            next_hop_peer_id: Some(2),
            cost: Some(1),
            path_latency: None,
            udp_nat_type: None,
            tcp_nat_type: None,
            inst_id: None,
            version: None,
            is_public_server: None,
        });

        let routes = aggregate_tun_routes(&instance);
        assert!(routes.contains(&"0.0.0.0/0".to_string()));
        // Specific prefixes collapse into /0 after simplify.
        assert!(!routes.contains(&"10.20.0.0/16".to_string()));
    }

    #[test]
    fn configured_but_unreachable_exit_does_not_install_default() {
        let mut instance = runtime_instance_from_config_snapshot(
            "routing-exit-down".to_string(),
            "test".to_string(),
            NetworkConfig {
                virtual_ipv4: Some("10.144.144.1".to_string()),
                network_length: Some(24),
                exit_nodes: vec!["10.144.144.2".to_string()],
                ..Default::default()
            },
            true,
        );
        instance.routes.push(RouteView {
            peer_id: 3,
            hostname: None,
            ipv4: Some("10.144.144.3".to_string()),
            ipv4_cidr: Some("10.144.144.3/24".to_string()),
            ipv6_cidr: None,
            proxy_cidrs: vec!["0.0.0.0/0".to_string(), "10.20.0.0/16".to_string()],
            next_hop_peer_id: Some(3),
            cost: Some(1),
            path_latency: None,
            udp_nat_type: None,
            tcp_nat_type: None,
            inst_id: None,
            version: None,
            is_public_server: None,
        });

        let routes = aggregate_tun_routes(&instance);
        assert!(routes.contains(&"10.20.0.0/16".to_string()));
        assert!(!routes.iter().any(|r| r == "0.0.0.0/0"));
    }
}
