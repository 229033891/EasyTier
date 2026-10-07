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
    /// IPs from URL literals and live tunnel remotes for the *current* host.
    resolved_ips: BTreeSet<IpAddr>,
    dns_cache: DnsCache,
    /// Previous host's exclude IPs kept only while the new host still has no
    /// resolved destinations — closes the URL-switch underlay gap without
    /// pinning traffic to a stale host after the new one resolves.
    stale_exclude_ips: BTreeSet<IpAddr>,
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
    stale_exclude_ips: BTreeSet::new(),
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
        // Preserve previous excludes as a short-lived fallback until the new
        // host resolves; otherwise TUN `/0` can swallow management traffic in
        // the DNS/connect gap after a URL change.
        let mut stale = status.resolved_ips.clone();
        stale.extend(status.dns_cache.ips.iter().copied());
        status.stale_exclude_ips = stale;
        status.dns_cache = DnsCache::default();
        status.resolved_ips.clear();
    }
    status.snapshot.endpoint_host = host;
    if let Some(ip) = url_host_ip(url) {
        status.resolved_ips.insert(ip);
        // Literal IP is immediately usable — drop stale previous host.
        status.stale_exclude_ips.clear();
    }
}

pub fn record_tunnel_remote(info: Option<&TunnelInfo>) {
    let Some(ip) = tunnel_remote_ip(info) else {
        return;
    };
    let mut status = STATUS.write();
    status.resolved_ips.insert(ip);
    // Live tunnel remote proves the current host is reachable — drop stale
    // previous-host excludes so we do not pin underlay to an obsolete peer.
    //
    // Accepted race (narrow): this path is not generation-keyed. After a URL
    // host change, a late report from a dying old connection can insert the
    // previous remote and clear `stale_exclude_ips` before the new host's DNS
    // returns. Only matters if that report lands in the DNS gap; reconnect
    // + DNS then repair. Not worth endpoint-generation plumbing for now.
    status.stale_exclude_ips.clear();
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

/// Clear a prior dial error after a new TCP tunnel is up but before the
/// session is fully ready (feature probe / secure upgrade). Keeps
/// `connected=false` so the UI stays on "connecting" instead of "failed".
pub fn clear_last_error() {
    let mut status = STATUS.write();
    status.snapshot.last_error = None;
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
    let (host, mut ips, cached_dns, stale) = {
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
            status.stale_exclude_ips.clone(),
        )
    };

    let Some(host) = host else {
        ips.extend(stale);
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
                if !resolved.is_empty() {
                    let mut status = STATUS.write();
                    if status.snapshot.endpoint_host.as_deref() == Some(host.as_str()) {
                        status.dns_cache = DnsCache {
                            ips: resolved.clone(),
                            fetched_at: Some(Instant::now()),
                        };
                        // Retire stale fallback once the new host has concrete
                        // destinations.
                        status.stale_exclude_ips.clear();
                    }
                }
                ips.extend(resolved);
            }
            Ok(Err(err)) => {
                tracing::warn!(%host, %err, "config-server DNS lookup failed for underlay exclude");
                let (dns_stale, url_stale) = {
                    let status = STATUS.read();
                    (
                        status.dns_cache.ips.clone(),
                        status.stale_exclude_ips.clone(),
                    )
                };
                ips.extend(dns_stale);
                if ips.is_empty() {
                    ips.extend(url_stale);
                }
            }
            Err(_) => {
                tracing::warn!(
                    %host,
                    timeout_ms = DNS_LOOKUP_TIMEOUT.as_millis(),
                    "config-server DNS lookup timed out for underlay exclude"
                );
                let (dns_stale, url_stale) = {
                    let status = STATUS.read();
                    (
                        status.dns_cache.ips.clone(),
                        status.stale_exclude_ips.clone(),
                    )
                };
                ips.extend(dns_stale);
                if ips.is_empty() {
                    ips.extend(url_stale);
                }
            }
        }
    }

    if ips.is_empty() {
        ips.extend(stale);
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
    use tokio::sync::{Mutex, MutexGuard};

    static TEST_LOCK: Mutex<()> = Mutex::const_new(());

    async fn lock_status() -> MutexGuard<'static, ()> {
        TEST_LOCK.lock().await
    }

    #[tokio::test]
    async fn host_dns_lookup_hook_is_used_for_hostname_endpoints() {
        let _guard = lock_status().await;
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

    #[tokio::test]
    async fn status_transitions_clear_error_on_connect() {
        let _guard = lock_status().await;
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
        let _guard = lock_status().await;
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
        let _guard = lock_status().await;
        clear();
        mark_enabled();
        set_endpoint_url(&Url::parse("udp://203.0.113.9:22020/token").unwrap());
        set_endpoint_url(&Url::parse("udp://198.51.100.7:22020/token").unwrap());
        let ips = underlay_exclude_candidate_ips().await;
        assert!(!ips.contains(&"203.0.113.9".parse().unwrap()));
        assert!(ips.contains(&"198.51.100.7".parse().unwrap()));
        clear();
    }

    #[tokio::test]
    async fn url_host_change_keeps_stale_excludes_until_new_host_resolves() {
        let _guard = lock_status().await;
        clear();
        set_host_dns_lookup(Some(Arc::new(|host| {
            Box::pin(async move {
                if host == "new.example.test" {
                    // Simulate DNS still empty for the new host.
                    Ok(Vec::new())
                } else {
                    Ok(vec!["198.51.100.50".parse().unwrap()])
                }
            })
        })));
        mark_enabled();
        set_endpoint_url(&Url::parse("udp://old.example.test:22020/token").unwrap());
        // Prime DNS cache for the old host.
        let old_ips = underlay_exclude_candidate_ips().await;
        assert!(old_ips.contains(&"198.51.100.50".parse().unwrap()));

        set_endpoint_url(&Url::parse("udp://new.example.test:22020/token").unwrap());
        let during_gap = underlay_exclude_candidate_ips().await;
        assert!(
            during_gap.contains(&"198.51.100.50".parse().unwrap()),
            "previous host must remain excluded until the new host resolves"
        );

        set_host_dns_lookup(Some(Arc::new(|_host| {
            Box::pin(async { Ok(vec!["203.0.113.77".parse().unwrap()]) })
        })));
        let after = underlay_exclude_candidate_ips().await;
        assert!(after.contains(&"203.0.113.77".parse().unwrap()));
        assert!(
            !after.contains(&"198.51.100.50".parse().unwrap()),
            "stale previous-host excludes must clear once the new host resolves"
        );
        set_host_dns_lookup(None);
        clear();
    }
}
