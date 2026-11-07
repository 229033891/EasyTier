use std::{collections::HashMap, net::IpAddr};

use network_interface::{NetworkInterface, NetworkInterfaceConfig};
#[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
use tokio::sync::Mutex;

use crate::proto::peer_rpc::GetIpListResponse;

use super::netns::NetNS;

#[derive(Clone, Copy, Debug, Default)]
struct InterfaceState {
    #[cfg_attr(
        any(
            target_os = "android",
            target_os = "ios",
            all(target_os = "macos", feature = "macos-ne"),
            target_env = "ohos"
        ),
        allow(dead_code)
    )]
    is_point_to_point: bool,
    #[cfg_attr(
        any(
            target_os = "android",
            target_os = "ios",
            all(target_os = "macos", feature = "macos-ne"),
            target_env = "ohos"
        ),
        allow(dead_code)
    )]
    is_loopback: bool,
    #[cfg_attr(
        any(
            target_os = "android",
            target_os = "ios",
            all(target_os = "macos", feature = "macos-ne"),
            target_env = "ohos"
        ),
        allow(dead_code)
    )]
    is_up: bool,
    #[cfg(target_os = "linux")]
    is_lower_up: bool,
}

#[cfg(any(
    all(target_os = "linux", not(target_env = "ohos")),
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
fn collect_interface_states() -> HashMap<String, InterfaceState> {
    let mut states = HashMap::new();
    if let Ok(interfaces) = nix::ifaddrs::getifaddrs() {
        use nix::net::if_::InterfaceFlags;

        for interface in interfaces {
            let flags = interface.flags;
            #[cfg(target_os = "linux")]
            let is_lower_up = flags.contains(InterfaceFlags::IFF_LOWER_UP);
            states.insert(
                interface.interface_name,
                InterfaceState {
                    is_point_to_point: flags.contains(InterfaceFlags::IFF_POINTOPOINT),
                    is_loopback: flags.contains(InterfaceFlags::IFF_LOOPBACK),
                    is_up: flags.contains(InterfaceFlags::IFF_UP),
                    #[cfg(target_os = "linux")]
                    is_lower_up,
                },
            );
        }
    }
    states
}

#[cfg(not(any(
    all(target_os = "linux", not(target_env = "ohos")),
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
)))]
fn collect_interface_states() -> HashMap<String, InterfaceState> {
    HashMap::new()
}

#[cfg(any(target_os = "freebsd", target_os = "windows"))]
fn has_nonzero_mac(iface: &NetworkInterface) -> bool {
    iface.mac_addr.as_deref().is_some_and(|mac| {
        let mut octets = mac.split([':', '-']);
        let mut nonzero = false;
        for _ in 0..6 {
            let Some(value) = octets
                .next()
                .and_then(|octet| u8::from_str_radix(octet, 16).ok())
            else {
                return false;
            };
            nonzero |= value != 0;
        }
        octets.next().is_none() && nonzero
    })
}

#[allow(dead_code)]
pub(crate) fn ip_mask_to_prefix(mask: IpAddr) -> Result<u8, ()> {
    match mask {
        IpAddr::V4(mask) => {
            let raw = u32::from(mask);
            let prefix = raw.leading_ones() as u8;
            let expected = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            (raw == expected).then_some(prefix).ok_or(())
        }
        IpAddr::V6(mask) => {
            let raw = u128::from(mask);
            let prefix = raw.leading_ones() as u8;
            let expected = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            (raw == expected).then_some(prefix).ok_or(())
        }
    }
}

struct InterfaceFilter {
    #[cfg_attr(
        any(
            target_os = "android",
            target_os = "ios",
            all(target_os = "macos", feature = "macos-ne"),
            target_env = "ohos"
        ),
        allow(dead_code)
    )]
    iface: NetworkInterface,
    #[cfg_attr(
        any(
            target_os = "android",
            target_os = "ios",
            all(target_os = "macos", feature = "macos-ne"),
            target_env = "ohos"
        ),
        allow(dead_code)
    )]
    state: InterfaceState,
}

fn interface_state(
    iface: &NetworkInterface,
    states: &HashMap<String, InterfaceState>,
) -> InterfaceState {
    states.get(&iface.name).copied().unwrap_or(InterfaceState {
        is_loopback: iface.internal,
        is_up: true,
        #[cfg(target_os = "linux")]
        is_lower_up: true,
        ..Default::default()
    })
}

#[cfg(any(
    target_os = "android",
    target_os = "ios",
    all(target_os = "macos", feature = "macos-ne"),
    target_env = "ohos"
))]
impl InterfaceFilter {
    async fn filter_iface(&self) -> bool {
        true
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl InterfaceFilter {
    async fn is_tun_tap_device(&self) -> bool {
        let path = format!("/sys/class/net/{}/tun_flags", self.iface.name);
        tokio::fs::metadata(&path).await.is_ok()
    }

    async fn has_valid_ip(&self) -> bool {
        self.iface
            .addr
            .iter()
            .map(|ip| ip.ip())
            .any(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
    }

    async fn filter_iface(&self) -> bool {
        tracing::trace!(
            "filter linux iface: {:?}, is_point_to_point: {}, is_loopback: {}, is_up: {}, is_lower_up: {}, is_tun: {}, has_valid_ip: {}",
            self.iface,
            self.state.is_point_to_point,
            self.state.is_loopback,
            self.state.is_up,
            self.state.is_lower_up,
            self.is_tun_tap_device().await,
            self.has_valid_ip().await
        );

        !self.state.is_point_to_point
            && !self.state.is_loopback
            && self.state.is_up
            && self.state.is_lower_up
            && !self.is_tun_tap_device().await
            && self.has_valid_ip().await
    }
}

// Cache for networksetup command output
#[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
static NETWORKSETUP_CACHE: std::sync::OnceLock<Mutex<(String, std::time::Instant)>> =
    std::sync::OnceLock::new();

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
impl InterfaceFilter {
    #[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
    async fn get_networksetup_output() -> String {
        use anyhow::Context;
        use std::time::{Duration, Instant};
        let cache = NETWORKSETUP_CACHE.get_or_init(|| Mutex::new((String::new(), Instant::now())));
        let mut cache_guard = cache.lock().await;

        // Check if cache is still valid (less than 1 minute old)
        if cache_guard.1.elapsed() < Duration::from_secs(60) && !cache_guard.0.is_empty() {
            return cache_guard.0.clone();
        }

        // Cache is expired or empty, fetch new data
        let stdout = tokio::process::Command::new("networksetup")
            .args(["-listallhardwareports"])
            .output()
            .await
            .with_context(|| "Failed to execute networksetup command")
            .and_then(|output| {
                std::str::from_utf8(&output.stdout)
                    .map(|s| s.to_string())
                    .with_context(|| "Failed to convert networksetup output to string")
            })
            .unwrap_or_else(|e| {
                tracing::error!("Failed to execute networksetup command: {:?}", e);
                String::new()
            });

        // Update cache
        cache_guard.0 = stdout.clone();
        cache_guard.1 = Instant::now();

        stdout
    }

    #[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
    async fn is_interface_physical(&self) -> bool {
        let interface_name = &self.iface.name;
        let stdout = Self::get_networksetup_output().await;

        let lines: Vec<&str> = stdout.lines().collect();

        for i in 0..lines.len() {
            let line = lines[i];

            if line.contains("Device:") && line.contains(interface_name) {
                let next_line = lines[i + 1];
                return !next_line.contains("Virtual Interface");
            }
        }

        false
    }

    #[cfg(target_os = "freebsd")]
    async fn is_interface_physical(&self) -> bool {
        // if mac addr is not zero, then it's physical interface
        has_nonzero_mac(&self.iface)
    }

    async fn filter_iface(&self) -> bool {
        !self.state.is_point_to_point
            && !self.state.is_loopback
            && self.state.is_up
            && self.is_interface_physical().await
    }
}

#[cfg(target_os = "windows")]
impl InterfaceFilter {
    async fn filter_iface(&self) -> bool {
        tracing::debug!(
            "iface_name: {:?}, p2p: {:?}, is_up: {:?}, iface: {:?}",
            self.iface.name,
            self.state.is_point_to_point,
            self.state.is_up,
            self.iface
        );
        !self.state.is_point_to_point
            && !self.state.is_loopback
            && self
                .iface
                .addr
                .iter()
                .map(|ip| ip.ip())
                .any(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
            && has_nonzero_mac(&self.iface)
    }
}

pub async fn local_ipv4() -> std::io::Result<std::net::Ipv4Addr> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect("8.8.8.8:80").await?;
    let addr = socket.local_addr()?;
    match addr.ip() {
        std::net::IpAddr::V4(ip) => Ok(ip),
        std::net::IpAddr::V6(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "no ipv4 address",
        )),
    }
}

pub async fn local_ipv6() -> std::io::Result<std::net::Ipv6Addr> {
    let socket = tokio::net::UdpSocket::bind("[::]:0").await?;
    socket
        .connect("[2001:4860:4860:0000:0000:0000:0000:8888]:80")
        .await?;
    let addr = socket.local_addr()?;
    match addr.ip() {
        std::net::IpAddr::V6(ip) => Ok(ip),
        std::net::IpAddr::V4(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "no ipv4 address",
        )),
    }
}

pub(crate) async fn collect_interfaces(net_ns: NetNS, filter: bool) -> Vec<NetworkInterface> {
    #[cfg(target_os = "linux")]
    {
        run_in_namespace(net_ns, move || async move {
            collect_interfaces_in_current_namespace(filter).await
        })
        .await
        .unwrap_or_default()
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _g = net_ns.guard();
        collect_interfaces_in_current_namespace(filter)
            .await
            .unwrap_or_default()
    }
}

#[cfg(feature = "faketcp")]
fn convert_pnet_interface(iface: pnet_datalink::NetworkInterface) -> NetworkInterface {
    let internal = iface.is_loopback();
    let addr = iface
        .ips
        .into_iter()
        .filter_map(|network| match (network.ip(), network.mask()) {
            (IpAddr::V4(ip), IpAddr::V4(netmask)) => {
                Some(network_interface::Addr::V4(network_interface::V4IfAddr {
                    ip,
                    broadcast: None,
                    netmask: Some(netmask),
                }))
            }
            (IpAddr::V6(ip), IpAddr::V6(netmask)) => {
                Some(network_interface::Addr::V6(network_interface::V6IfAddr {
                    ip,
                    broadcast: None,
                    netmask: Some(netmask),
                }))
            }
            _ => None,
        })
        .collect();
    NetworkInterface {
        name: iface.name,
        addr,
        mac_addr: iface.mac.map(|mac| mac.to_string()),
        index: iface.index,
        internal,
    }
}

/// `None` = enumeration failed (do not treat as an empty adapter list).
/// `Some` = successful snapshot (may be empty after filtering).
async fn collect_interfaces_in_current_namespace(filter: bool) -> Option<Vec<NetworkInterface>> {
    // Keep `show()` synchronous and on the calling thread: netns (`setns`) is
    // thread-bound. Callers that must leave the async runtime do so *above*
    // this helper (see `collect_local_ipv4_networks` / `run_in_namespace`).
    let ifaces = match NetworkInterface::show() {
        Ok(ifaces) => ifaces,
        Err(error) => {
            tracing::warn!(?error, "failed to enumerate network interfaces");
            #[cfg(feature = "faketcp")]
            {
                match std::panic::catch_unwind(pnet_datalink::interfaces) {
                    Ok(ifaces) => ifaces.into_iter().map(convert_pnet_interface).collect(),
                    Err(_) => {
                        tracing::error!(
                            "failed to enumerate network interfaces via network-interface and pnet"
                        );
                        return None;
                    }
                }
            }
            #[cfg(not(feature = "faketcp"))]
            return None;
        }
    };
    let states = collect_interface_states();
    let mut ret = vec![];
    for iface in ifaces {
        let f = InterfaceFilter {
            iface: iface.clone(),
            state: interface_state(&iface, &states),
        };

        if filter && !f.filter_iface().await {
            continue;
        }

        ret.push(iface);
    }

    Some(ret)
}

/// Run `operation` on a blocking thread after switching to `net_ns`.
///
/// `None` = the namespace-local runtime could not be built, or the blocking
/// task panicked. Discovery must degrade, never kill its caller: the proxy-CIDR
/// route updater would otherwise stop both installing *and* tearing down routes
/// until the instance restarts.
async fn run_in_namespace_opt<T, F, Fut>(net_ns: NetNS, operation: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T> + 'static,
{
    match tokio::task::spawn_blocking(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::error!(?error, "failed to build namespace-local runtime");
                return None;
            }
        };
        // `NetNS::run` switches ns on *this* thread, and the current-thread
        // runtime below polls the future on the same thread.
        Some(net_ns.run(|| runtime.block_on(operation())))
    })
    .await
    {
        Ok(result) => result,
        Err(join_err) => {
            tracing::error!(?join_err, "namespace-local network operation panicked");
            None
        }
    }
}

#[cfg(target_os = "linux")]
async fn run_in_namespace<T, F, Fut>(net_ns: NetNS, operation: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T> + 'static,
{
    run_in_namespace_opt(net_ns, operation)
        .await
        .expect("namespace-local network operation failed")
}

#[tracing::instrument(skip(net_ns))]
pub(crate) async fn collect_local_ip_addrs(net_ns: NetNS) -> GetIpListResponse {
    #[cfg(target_os = "linux")]
    {
        return run_in_namespace(net_ns, || async {
            collect_local_ip_addrs_in_current_namespace().await
        })
        .await;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _g = net_ns.guard();
        collect_local_ip_addrs_in_current_namespace().await
    }
}

/// Collect local IPv4 networks (address + netmask → CIDR) for LAN L1 prune / L2 on-link.
///
/// Returns `None` when interface enumeration fails — callers must **not** treat
/// that as an empty LAN (which would flip L2 to "all underlay off-link").
///
/// `exclude_ifname` skips the EasyTier TUN interface (required on Windows where
/// the interface filter does not exclude Wintun the way Linux excludes tun/tap).
pub async fn collect_local_ipv4_networks(
    net_ns: NetNS,
    exclude_ifname: Option<&str>,
) -> Option<Vec<cidr::Ipv4Cidr>> {
    let exclude = exclude_ifname.map(str::to_owned);
    // `NetworkInterface::show()` is a sync full-adapter walk (tens–hundreds of ms
    // on Windows), so this always leaves the async runtime; `run_in_namespace_opt`
    // then switches ns on that same blocking thread, which keeps netns affinity
    // independent of tokio blocking-pool reuse.
    run_in_namespace_opt(net_ns, move || async move {
        collect_local_ipv4_networks_in_current_namespace(exclude.as_deref()).await
    })
    .await
    .flatten()
}

fn warn_rate_limited(last_warn_ms: &std::sync::atomic::AtomicU64, interval_ms: u64) -> bool {
    use std::sync::atomic::Ordering;
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let prev = last_warn_ms.load(Ordering::Relaxed);
    if now.saturating_sub(prev) >= interval_ms {
        last_warn_ms.store(now, Ordering::Relaxed);
        true
    } else {
        false
    }
}

async fn collect_local_ipv4_networks_in_current_namespace(
    exclude_ifname: Option<&str>,
) -> Option<Vec<cidr::Ipv4Cidr>> {
    use std::collections::BTreeSet;
    use std::sync::atomic::AtomicU64;

    static WIDE_MASK_LAST_WARN_MS: AtomicU64 = AtomicU64::new(0);
    const WIDE_MASK_WARN_INTERVAL_MS: u64 = 60_000;

    let ifaces = collect_interfaces_in_current_namespace(true).await?;
    let mut out = BTreeSet::new();
    for iface in ifaces {
        if exclude_ifname.is_some_and(|name| iface.name.eq_ignore_ascii_case(name)) {
            continue;
        }
        for addr in iface.addr {
            let network_interface::Addr::V4(v4) = addr else {
                continue;
            };
            if v4.ip.is_loopback() || v4.ip.is_unspecified() || v4.ip.is_multicast() {
                continue;
            }
            let Some(netmask) = v4.netmask else {
                continue;
            };
            let Ok(prefix) = ip_mask_to_prefix(IpAddr::V4(netmask)) else {
                continue;
            };
            // 0.0.0.0 mask → /0 would hole-cover every proxy CIDR and silently
            // disable subnet proxy (PPP/DHCP/virtual NIC glitches). Skip /0 and
            // other absurdly wide masks (< /8) rather than treating them as LAN.
            if prefix < 8 {
                if warn_rate_limited(&WIDE_MASK_LAST_WARN_MS, WIDE_MASK_WARN_INTERVAL_MS) {
                    tracing::warn!(
                        iface = %iface.name,
                        ip = %v4.ip,
                        %netmask,
                        prefix,
                        "skipping local IPv4 network with too-wide netmask for LAN prune"
                    );
                } else {
                    tracing::debug!(
                        iface = %iface.name,
                        ip = %v4.ip,
                        %netmask,
                        prefix,
                        "skipping local IPv4 network with too-wide netmask for LAN prune"
                    );
                }
                continue;
            }
            if let Ok(inet) = cidr::Ipv4Inet::new(v4.ip, prefix) {
                out.insert(inet.network());
            }
        }
    }
    Some(out.into_iter().collect())
}

async fn collect_local_ip_addrs_in_current_namespace() -> GetIpListResponse {
    let mut ret = GetIpListResponse::default();

    let ifaces = collect_interfaces_in_current_namespace(true)
        .await
        .unwrap_or_default();
    for iface in ifaces {
        for ip in iface.addr {
            let ip: std::net::IpAddr = ip.ip();
            if let std::net::IpAddr::V4(v4) = ip {
                if ip.is_loopback() || ip.is_multicast() {
                    continue;
                }
                ret.interface_ipv4s.push(v4.into());
            }
        }
    }

    let ifaces = collect_interfaces_in_current_namespace(false)
        .await
        .unwrap_or_default();
    for iface in ifaces {
        for ip in iface.addr {
            let ip: std::net::IpAddr = ip.ip();
            if let std::net::IpAddr::V6(v6) = ip {
                if v6.is_multicast() || v6.is_loopback() || v6.is_unicast_link_local() {
                    continue;
                }
                ret.interface_ipv6s.push(v6.into());
            }
        }
    }

    if let Ok(v4_addr) = local_ipv4().await {
        tracing::trace!("got local ipv4: {}", v4_addr);
        if !ret.interface_ipv4s.contains(&v4_addr.into()) {
            ret.interface_ipv4s.push(v4_addr.into());
        }
    }

    if let Ok(v6_addr) = local_ipv6().await {
        tracing::trace!("got local ipv6: {}", v6_addr);
        if !ret.interface_ipv6s.contains(&v6_addr.into()) {
            ret.interface_ipv6s.push(v6_addr.into());
        }
    }

    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_contiguous_ip_masks_to_prefixes() {
        assert_eq!(
            ip_mask_to_prefix(IpAddr::V4("255.255.254.0".parse().unwrap())),
            Ok(23)
        );
        assert_eq!(
            ip_mask_to_prefix(IpAddr::V6("ffff:ffff:ffff:ffff::".parse().unwrap())),
            Ok(64)
        );
        assert_eq!(
            ip_mask_to_prefix(IpAddr::V4("255.0.255.0".parse().unwrap())),
            Err(())
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn namespace_operation_does_not_migrate_between_os_threads() {
        let (before, after) = run_in_namespace(NetNS::new(None), || async {
            let before = std::thread::current().id();
            tokio::task::yield_now().await;
            (before, std::thread::current().id())
        })
        .await;

        assert_eq!(before, after);
    }

    /// A panicking discovery operation must surface as `None`, not kill the
    /// caller (the route updater would stop installing/tearing down routes).
    /// The panic backtrace printed by the test harness is expected.
    #[tokio::test]
    async fn namespace_operation_panic_degrades_to_none() {
        let result: Option<()> = run_in_namespace_opt(NetNS::new(None), || async {
            panic!("simulated namespace operation failure");
        })
        .await;

        assert!(result.is_none());
    }
}
