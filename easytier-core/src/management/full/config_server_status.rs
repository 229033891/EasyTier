//! Process-wide config-server connection status for local UI / RPC queries,
//! plus underlay destinations that must stay off the TUN exit default.

use std::{
    collections::BTreeSet,
    future::Future,
    net::IpAddr,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::RwLock;
use url::Url;

use crate::proto::common::{TunnelInfo, Url as ProtoUrl};

const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_millis(300);
const DNS_CACHE_TTL: Duration = Duration::from_secs(45);

/// Optional host DNS lookup used for config-server underlay excludes.
/// Native desktop installs a physical-iface-bound resolver; tests may omit it.
pub type HostDnsLookupFn = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, String>> + Send>>
        + Send
        + Sync,
>;

static HOST_DNS_LOOKUP: RwLock<Option<HostDnsLookupFn>> = RwLock::new(None);

/// Install the process-wide hostname lookup used by underlay exclude collection.
pub fn set_host_dns_lookup(lookup: Option<HostDnsLookupFn>) {
    *HOST_DNS_LOOKUP.write() = lookup;
}

fn host_dns_lookup() -> Option<HostDnsLookupFn> {
    HOST_DNS_LOOKUP.read().clone()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigServerStatusSnapshot {
    pub enabled: bool,
    pub connected: bool,
    pub last_error: Option<String>,
    /// Host from the config-server connect URL (IP literal or DNS name).
    pub endpoint_host: Option<String>,
}

#[derive(Clone, Default)]
struct DnsCache {
    ips: BTreeSet<IpAddr>,
    fetched_at: Option<Instant>,
}

#[derive(Default)]
struct Status {
    snapshot: ConfigServerStatusSnapshot,
    /// IPs from URL literals and live tunnel remotes.
    resolved_ips: BTreeSet<IpAddr>,
    dns_cache: DnsCache,
}

static STATUS: RwLock<Status> = RwLock::new(Status {
    snapshot: ConfigServerStatusSnapshot {
        enabled: false,
        connected: false,
        last_error: None,
        endpoint_host: None,
    },
    resolved_ips: BTreeSet::new(),
    dns_cache: DnsCache {
        ips: BTreeSet::new(),
        fetched_at: None,
    },
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
    let host = url.host_str().map(|host| host.to_string());
    if status.snapshot.endpoint_host.as_ref() != host.as_ref() {
        status.dns_cache = DnsCache::default();
    }
    status.snapshot.endpoint_host = host;
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
    // Only reset config-server status. The process-wide host DNS lookup hook is
    // installed once by the native runtime and must survive WebClient teardown.
    *STATUS.write() = Status::default();
}

/// Candidate underlay IPs for the active config-server endpoint (cached + DNS).
pub async fn underlay_exclude_candidate_ips() -> BTreeSet<IpAddr> {
    let (host, mut ips, cached_dns) = {
        let status = STATUS.read();
        if !status.snapshot.enabled {
            return BTreeSet::new();
        }
        let cached_dns = status
            .dns_cache
            .fetched_at
            .is_some_and(|at| at.elapsed() < DNS_CACHE_TTL)
            .then(|| status.dns_cache.ips.clone());
        (
            status.snapshot.endpoint_host.clone(),
            status.resolved_ips.clone(),
            cached_dns,
        )
    };

    let Some(host) = host else {
        return ips;
    };

    if host.parse::<IpAddr>().is_ok() {
        return ips;
    }

    if let Some(cached) = cached_dns {
        ips.extend(cached);
        return ips;
    }

    #[cfg(not(any(target_os = "wasi", target_arch = "wasm32")))]
    {
        let lookup = tokio::time::timeout(DNS_LOOKUP_TIMEOUT, async {
            if let Some(host_lookup) = host_dns_lookup() {
                host_lookup(host.clone())
                    .await
                    .map(|ips| ips.into_iter().collect::<BTreeSet<_>>())
                    .map_err(std::io::Error::other)
            } else {
                let addrs = tokio::net::lookup_host((host.as_str(), 0)).await?;
                Ok(addrs.map(|addr| addr.ip()).collect::<BTreeSet<_>>())
            }
        })
        .await;
        match lookup {
            Ok(Ok(resolved)) => {
                {
                    let mut status = STATUS.write();
                    if status.snapshot.endpoint_host.as_deref() == Some(host.as_str()) {
                        status.dns_cache = DnsCache {
                            ips: resolved.clone(),
                            fetched_at: Some(Instant::now()),
                        };
                    }
                }
                ips.extend(resolved);
            }
            Ok(Err(err)) => {
                tracing::warn!(%host, %err, "config-server DNS lookup failed for underlay exclude");
                let stale = STATUS.read().dns_cache.ips.clone();
                ips.extend(stale);
            }
            Err(_) => {
                tracing::warn!(
                    %host,
                    timeout_ms = DNS_LOOKUP_TIMEOUT.as_millis(),
                    "config-server DNS lookup timed out for underlay exclude"
                );
                let stale = STATUS.read().dns_cache.ips.clone();
                ips.extend(stale);
            }
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
    use std::sync::{Mutex, MutexGuard};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_status() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[tokio::test]
    async fn host_dns_lookup_hook_is_used_for_hostname_endpoints() {
        let _guard = lock_status();
        clear();
        set_host_dns_lookup(Some(Arc::new(|_host| {
            Box::pin(async {
                Ok(vec![
                    "198.51.100.50".parse().unwrap(),
                    "2001:db8::50".parse().unwrap(),
                ])
            })
        })));
        mark_enabled();
        set_endpoint_url(&Url::parse("udp://config.example.test:22020/token").unwrap());
        let ips = underlay_exclude_candidate_ips().await;
        assert!(ips.contains(&"198.51.100.50".parse().unwrap()));
        assert!(ips.contains(&"2001:db8::50".parse().unwrap()));
        set_host_dns_lookup(None);
        clear();
    }

    #[test]
    fn status_transitions_clear_error_on_connect() {
        let _guard = lock_status();
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
        let _guard = lock_status();
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
        let _guard = lock_status();
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
