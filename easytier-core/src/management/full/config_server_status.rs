//! Process-wide config-server connection status for local UI / RPC queries,
//! plus underlay destinations that must stay off the TUN exit default.

use std::{collections::BTreeSet, net::IpAddr, time::Duration};

use parking_lot::RwLock;
use url::Url;

use crate::proto::common::{TunnelInfo, Url as ProtoUrl};

const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigServerStatusSnapshot {
    pub enabled: bool,
    pub connected: bool,
    pub last_error: Option<String>,
    /// Host from the config-server connect URL (IP literal or DNS name).
    pub endpoint_host: Option<String>,
}

#[derive(Default)]
struct Status {
    snapshot: ConfigServerStatusSnapshot,
    /// IPs from URL literals and live tunnel remotes (not DNS — refreshed each lookup).
    resolved_ips: BTreeSet<IpAddr>,
}

static STATUS: RwLock<Status> = RwLock::new(Status {
    snapshot: ConfigServerStatusSnapshot {
        enabled: false,
        connected: false,
        last_error: None,
        endpoint_host: None,
    },
    resolved_ips: BTreeSet::new(),
});

pub fn snapshot() -> ConfigServerStatusSnapshot {
    STATUS.read().snapshot.clone()
}

pub fn mark_enabled() {
    let mut status = STATUS.write();
    status.snapshot.enabled = true;
    status.snapshot.connected = false;
    status.snapshot.last_error = None;
}

/// Remember the config-server connect URL so exit-node default routing can
/// pin management-plane traffic to the physical gateway.
pub fn set_endpoint_url(url: &Url) {
    let mut status = STATUS.write();
    status.snapshot.endpoint_host = url.host_str().map(|host| host.to_string());
    status.resolved_ips.clear();
    if let Some(ip) = url_host_ip(url) {
        status.resolved_ips.insert(ip);
    }
}

pub fn record_tunnel_remote(info: Option<&TunnelInfo>) {
    let Some(ip) = tunnel_remote_ip(info) else {
        return;
    };
    STATUS.write().resolved_ips.insert(ip);
}

pub fn mark_connected() {
    let mut status = STATUS.write();
    status.snapshot.enabled = true;
    status.snapshot.connected = true;
    status.snapshot.last_error = None;
}

pub fn mark_disconnected() {
    let mut status = STATUS.write();
    if status.snapshot.enabled {
        status.snapshot.connected = false;
    }
}

pub fn mark_error(error: impl Into<String>) {
    let mut status = STATUS.write();
    status.snapshot.enabled = true;
    status.snapshot.connected = false;
    status.snapshot.last_error = Some(error.into());
}

pub fn clear() {
    *STATUS.write() = Status::default();
}

/// Candidate underlay IPs for the active config-server endpoint (cached + DNS).
pub async fn underlay_exclude_candidate_ips() -> BTreeSet<IpAddr> {
    let (host, mut ips) = {
        let status = STATUS.read();
        if !status.snapshot.enabled {
            return BTreeSet::new();
        }
        (
            status.snapshot.endpoint_host.clone(),
            status.resolved_ips.clone(),
        )
    };

    let Some(host) = host else {
        return ips;
    };

    if host.parse::<IpAddr>().is_ok() {
        return ips;
    }

    let lookup = tokio::time::timeout(
        DNS_LOOKUP_TIMEOUT,
        tokio::net::lookup_host((host.as_str(), 0)),
    )
    .await;
    if let Ok(Ok(addrs)) = lookup {
        for addr in addrs {
            ips.insert(addr.ip());
        }
    }

    ips
}

fn url_host_ip(url: &Url) -> Option<IpAddr> {
    match url.host()? {
        url::Host::Ipv4(ip) => Some(IpAddr::V4(ip)),
        url::Host::Ipv6(ip) => Some(IpAddr::V6(ip)),
        url::Host::Domain(host) => host.parse().ok(),
    }
}

fn tunnel_remote_ip(info: Option<&TunnelInfo>) -> Option<IpAddr> {
    let info = info?;
    parse_proto_url_host_ip(info.resolved_remote_addr.as_ref())
        .or_else(|| parse_proto_url_host_ip(info.remote_addr.as_ref()))
}

fn parse_proto_url_host_ip(url: Option<&ProtoUrl>) -> Option<IpAddr> {
    url_host_ip(&Url::from(url?.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_transitions_clear_error_on_connect() {
        clear();
        mark_enabled();
        mark_error("boom");
        assert_eq!(
            snapshot(),
            ConfigServerStatusSnapshot {
                enabled: true,
                connected: false,
                last_error: Some("boom".into()),
                endpoint_host: None,
            }
        );
        mark_connected();
        assert_eq!(
            snapshot(),
            ConfigServerStatusSnapshot {
                enabled: true,
                connected: true,
                last_error: None,
                endpoint_host: None,
            }
        );
        clear();
    }

    #[tokio::test]
    async fn set_endpoint_url_records_literal_ip() {
        clear();
        mark_enabled();
        set_endpoint_url(&Url::parse("udp://203.0.113.9:22020/token").unwrap());
        let snap = snapshot();
        assert_eq!(snap.endpoint_host.as_deref(), Some("203.0.113.9"));
        assert!(
            underlay_exclude_candidate_ips()
                .await
                .contains(&"203.0.113.9".parse().unwrap())
        );
        clear();
    }

    #[tokio::test]
    async fn set_endpoint_url_replaces_previous_resolved_ips() {
        clear();
        mark_enabled();
        set_endpoint_url(&Url::parse("udp://203.0.113.9:22020/token").unwrap());
        set_endpoint_url(&Url::parse("udp://198.51.100.7:22020/token").unwrap());
        let ips = underlay_exclude_candidate_ips().await;
        assert!(!ips.contains(&"203.0.113.9".parse().unwrap()));
        assert!(ips.contains(&"198.51.100.7".parse().unwrap()));
        clear();
    }
}
