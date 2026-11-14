use std::{collections::BTreeSet, net::IpAddr, sync::Arc};

use arc_swap::ArcSwap;
use async_trait::async_trait;

use crate::{
    config::runtime::CoreRuntimeConfigStore,
    gateway::magic_dns::{MagicDnsRouteSnapshot, MagicDnsRouteSource},
    gateway::proxy::cidr_monitor::{
        ProxyCidrDiff, ProxyCidrRouteSyncStatus, collect_proxy_cidr_diff, collect_proxy_cidrs,
    },
    gateway::proxy::underlay_exclude::collect_underlay_exclude_ips,
    host::packet::HostPacket,
    peers::peer_manager::PeerManagerCore,
};

#[cfg(feature = "proxy-packet")]
use crate::foundation::stats::{LabelSet, LabelType, MetricName};
#[cfg(feature = "proxy-packet")]
use crate::gateway::magic_dns::{
    MagicDnsQueryResolver, MagicDnsResolverRegistration, ObservingDnsResolver,
    magic_dns_packet_filter,
};
#[cfg(feature = "proxy-packet")]
use crate::gateway::udp_broadcast::UdpBroadcastRelayStats;

use super::packet_io::parse_ip_packet;

/// Stable packet- and route-plane projection for platform integrations.
pub struct CorePacketPlane {
    peer_manager: Arc<PeerManagerCore>,
    runtime_config: CoreRuntimeConfigStore,
    proxy_cidr_monitor_available: bool,
    proxy_cidr_route_sync: ArcSwap<ProxyCidrRouteSyncStatus>,
    underlay_exclude_state: ArcSwap<UnderlayExcludeState>,
}

/// Mirror of the installer's physical default route snapshot (the host
/// crate's `PhysicalDefaultRoute`, mirrored as plain data so `easytier-core`
/// never depends back on the host crate). Cleanup must delete with the
/// **same** via the installer used: stop-time re-resolution fails once the
/// network is down, and Windows identifies the route by `ifindex + gateway`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnderlayExcludeViaMirror {
    pub ifindex: u32,
    pub ifname: String,
    pub gateway: Option<IpAddr>,
}

/// What the desktop underlay-exclude installer currently believes is pinned
/// on the physical interface: the installed host routes plus the via used
/// for each family. Published incrementally after every successful route
/// add/remove, so a stop-time cleanup can uninstall even when the desired
/// set has already drained (peers gone).
///
/// Abort window (default scheme, roadmap §4): during a single
/// `add_*_host_route().await` the published `installed` is a **subset** of
/// the kernel — an abort landing exactly there loses that one entry's
/// accounting. Incremental publishing keeps the window at one entry instead
/// of the whole reconcile batch; the "publish intent first" alternative in
/// roadmap §4 closes it entirely. Remove tolerates NotFound, so transient
/// over-reporting is harmless — do not claim stronger guarantees than the
/// implemented scheme in code or docs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnderlayExcludeState {
    pub installed: BTreeSet<IpAddr>,
    pub ipv4_via: Option<UnderlayExcludeViaMirror>,
    pub ipv6_via: Option<UnderlayExcludeViaMirror>,
}

impl CorePacketPlane {
    pub(super) fn new(
        peer_manager: Arc<PeerManagerCore>,
        runtime_config: CoreRuntimeConfigStore,
        proxy_cidr_monitor_available: bool,
    ) -> Self {
        Self {
            peer_manager,
            runtime_config,
            proxy_cidr_monitor_available,
            proxy_cidr_route_sync: ArcSwap::from_pointee(ProxyCidrRouteSyncStatus::default()),
            underlay_exclude_state: ArcSwap::from_pointee(UnderlayExcludeState::default()),
        }
    }

    pub async fn send_ip_packet(&self, packet: HostPacket) -> anyhow::Result<()> {
        let meta = parse_ip_packet(packet.payload())?;
        let source_is_local = self.peer_manager.is_local_virtual_ip(&meta.source);
        if matches!(meta.source, IpAddr::V6(ip) if ip.is_unicast_link_local()) && !source_is_local {
            return Ok(());
        }
        self.peer_manager
            .send_msg_by_ip(packet.into_core_packet(), meta.destination, source_is_local)
            .await
            .map_err(Into::into)
    }

    pub async fn send_local_ip_packet(&self, packet: HostPacket) -> anyhow::Result<()> {
        let destination = parse_ip_packet(packet.payload())?.destination;
        self.peer_manager
            .send_msg_by_ip(packet.into_core_packet(), destination, true)
            .await
            .map_err(Into::into)
    }

    pub async fn proxy_cidr_diff(
        &self,
        previous: &BTreeSet<cidr::Ipv4Cidr>,
    ) -> Option<ProxyCidrDiff> {
        if !self.proxy_cidr_monitor_available {
            return None;
        }
        Some(
            collect_proxy_cidr_diff(self.peer_manager.as_ref(), &self.runtime_config, previous)
                .await,
        )
    }

    /// On-demand L2 desired proxy-CIDR set for host-side installers (Android
    /// VpnService / OHOS) that cannot read the core config. Same computation
    /// the desktop installer reconciles against (`resolve_proxy_cidrs`, incl.
    /// manual_routes override/append, policy local_routes, exit default and
    /// escape hatch) — hosts install it verbatim instead of reimplementing
    /// the semantics. Deliberately NOT gated on `proxy_cidr_monitor_available`:
    /// an empty result must mean "install nothing", never "subsystem off", so
    /// a new core always yields a usable (possibly empty) set.
    pub async fn proxy_cidr_desired(&self) -> Vec<String> {
        let config = self.runtime_config.snapshot();
        collect_proxy_cidrs(self.peer_manager.as_ref(), config.as_ref())
            .await
            .into_iter()
            .map(|cidr| cidr.to_string())
            .collect()
    }

    pub fn proxy_cidr_route_sync_status(&self) -> ProxyCidrRouteSyncStatus {
        self.proxy_cidr_route_sync.load_full().as_ref().clone()
    }

    /// Publish the underlay-exclude installer's current view. Called after
    /// every successful route add/remove (and after via switches), not once
    /// per reconcile batch: the updater task is aborted on stop, and an abort
    /// between "route installed" and "state published" must not widen the
    /// loss window beyond a single entry.
    pub fn report_underlay_exclude_state(&self, state: UnderlayExcludeState) {
        self.underlay_exclude_state.store(Arc::new(state));
    }

    /// Installed underlay-exclude host routes + via snapshots, for the
    /// stop-time cleanup. Defaults to empty: hosts that never ran the
    /// installer (mobile / no-TUN runtimes) get a no-op cleanup instead of
    /// the old "re-collect desired and hope" behavior.
    pub fn underlay_exclude_state(&self) -> UnderlayExcludeState {
        self.underlay_exclude_state.load_full().as_ref().clone()
    }

    pub fn report_proxy_cidr_route_sync(
        &self,
        desired: &BTreeSet<cidr::Ipv4Cidr>,
        installed: &BTreeSet<cidr::Ipv4Cidr>,
        local_exit_default: bool,
        last_error: Option<String>,
    ) {
        let status = ProxyCidrRouteSyncStatus {
            desired: desired.iter().map(ToString::to_string).collect(),
            installed: installed.iter().map(ToString::to_string).collect(),
            local_exit_default,
            last_error,
        };
        self.proxy_cidr_route_sync.store(Arc::new(status));
    }

    /// Already-resolved underlay destinations that must not follow the TUN
    /// exit-node default route (peer tunnel remotes, peer STUN public IPs,
    /// and the process config-server / management endpoint when enabled).
    pub async fn underlay_exclude_ips(&self) -> BTreeSet<IpAddr> {
        if !self.proxy_cidr_monitor_available {
            return BTreeSet::new();
        }
        #[cfg(feature = "web-client")]
        let extra = crate::management::config_server_underlay_ips().await;
        #[cfg(not(feature = "web-client"))]
        let extra = std::iter::empty::<IpAddr>();
        collect_underlay_exclude_ips(self.peer_manager.as_ref(), extra).await
    }

    /// Inject physical local IPv4 networks for L1 proxy-CIDR prune / L2 on-link.
    ///
    /// `None` = enumeration failed (keep previous); `Some` = known snapshot.
    pub fn set_local_ipv4_networks(&self, nets: Option<Vec<cidr::Ipv4Cidr>>) {
        self.peer_manager.set_local_ipv4_networks(nets);
    }

    /// `None` = host has not injected physical nets yet; `Some` = known set
    /// (possibly empty).
    pub fn local_ipv4_networks(&self) -> Arc<Option<Vec<cidr::Ipv4Cidr>>> {
        self.peer_manager.local_ipv4_networks()
    }

    /// L2 underlay `/32` gate: TUN `/0` **or** a desired prefix covering an
    /// off-link peer underlay address (see open-bugs-wave1-impl §2.2).
    ///
    /// Unknown physical nets (`None`) only trip the `/0` gate — treating
    /// "unknown" as "empty" would mark every underlay IP off-link on hosts that
    /// never inject nets (non-desktop) and install spurious `/32` excludes.
    pub fn needs_underlay_excludes(
        &self,
        effective_desired: &BTreeSet<cidr::Ipv4Cidr>,
        underlay_ips: &BTreeSet<IpAddr>,
    ) -> bool {
        needs_underlay_excludes(
            effective_desired,
            underlay_ips,
            self.local_ipv4_networks().as_ref().as_deref(),
        )
    }

    pub async fn public_ipv6_routes(&self) -> BTreeSet<cidr::Ipv6Inet> {
        self.peer_manager.list_public_ipv6_routes().await
    }

    pub async fn public_ipv6_addr(&self) -> Option<cidr::Ipv6Inet> {
        self.peer_manager.public_ipv6_addr().await
    }

    #[cfg(feature = "proxy-packet")]
    pub fn udp_broadcast_relay_stats(&self) -> UdpBroadcastRelayStats {
        let network_name = self
            .runtime_config
            .snapshot()
            .peer
            .runtime
            .network_identity
            .network_name
            .clone();
        let labels = LabelSet::new().with_label_type(LabelType::NetworkName(network_name));
        let stats = self.peer_manager.stats_manager();
        UdpBroadcastRelayStats::new(
            stats.get_counter(MetricName::UdpBroadcastRelayPacketsCaptured, labels.clone()),
            stats.get_counter(MetricName::UdpBroadcastRelayPacketsIgnored, labels.clone()),
            stats.get_counter(
                MetricName::UdpBroadcastRelayPacketsForwarded,
                labels.clone(),
            ),
            stats.get_counter(MetricName::UdpBroadcastRelayPacketsForwardFailed, labels),
        )
    }

    #[cfg(feature = "proxy-packet")]
    pub async fn register_magic_dns_resolver(
        &self,
        fake_ip: std::net::Ipv4Addr,
        resolver: Arc<dyn MagicDnsQueryResolver>,
    ) -> MagicDnsResolverRegistration {
        // P3: every answer the resolver hands back is fed into the unified
        // routing table so domain policy routes learn their IPs. The wrapper is
        // a no-op while `[[policy_routes]]` carries no domain rules, and it is
        // the only place that can observe what an application actually
        // resolved (§9.8.3).
        let resolver: Arc<dyn MagicDnsQueryResolver> = Arc::new(ObservingDnsResolver::new(
            resolver,
            Arc::downgrade(&self.peer_manager),
        ));
        let runtime = tokio::runtime::Handle::current();
        let (filter, absorb_cancel) = magic_dns_packet_filter(
            fake_ip,
            self.peer_manager.my_peer_id(),
            resolver,
            Arc::downgrade(&self.peer_manager),
        );
        let pipeline = self
            .peer_manager
            .add_managed_nic_packet_process_pipeline(filter)
            .await;
        MagicDnsResolverRegistration::new(
            Arc::downgrade(&self.peer_manager),
            pipeline,
            absorb_cancel,
            runtime,
        )
    }
}

#[async_trait]
impl MagicDnsRouteSource for CorePacketPlane {
    async fn snapshot(&self) -> MagicDnsRouteSnapshot {
        MagicDnsRouteSource::snapshot(self.peer_manager.as_ref()).await
    }

    async fn revision(&self) -> quanta::Instant {
        MagicDnsRouteSource::revision(self.peer_manager.as_ref()).await
    }
}

/// Pure L2 exclude predicate (unit-tested).
pub(crate) fn needs_underlay_excludes(
    effective_desired: &BTreeSet<cidr::Ipv4Cidr>,
    underlay_ips: &BTreeSet<IpAddr>,
    local_nets: Option<&[cidr::Ipv4Cidr]>,
) -> bool {
    if effective_desired.iter().any(|c| c.network_length() == 0) {
        return true;
    }
    let Some(local_nets) = local_nets else {
        return false;
    };
    underlay_ips.iter().any(|ip| {
        let IpAddr::V4(v4) = ip else {
            return false;
        };
        let covered = effective_desired.iter().any(|c| c.contains(v4));
        let on_link = local_nets.iter().any(|n| n.contains(v4));
        covered && !on_link
    })
}

#[cfg(test)]
mod underlay_exclude_state_tests {
    use super::*;

    #[test]
    fn default_state_installs_nothing() {
        let state = UnderlayExcludeState::default();
        assert!(state.installed.is_empty());
        assert!(state.ipv4_via.is_none());
        assert!(state.ipv6_via.is_none());
    }
}

#[cfg(test)]
mod underlay_exclude_gate_tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn c(s: &str) -> cidr::Ipv4Cidr {
        s.parse().unwrap()
    }

    #[test]
    fn unknown_local_nets_only_trigger_on_default_route() {
        let mut desired = BTreeSet::new();
        desired.insert(c("192.168.8.0/22"));
        let mut underlay = BTreeSet::new();
        underlay.insert(IpAddr::V4(Ipv4Addr::new(192, 168, 10, 101)));
        assert!(!needs_underlay_excludes(&desired, &underlay, None));

        desired.insert(c("0.0.0.0/0"));
        assert!(needs_underlay_excludes(&desired, &underlay, None));
    }

    #[test]
    fn off_link_covered_underlay_needs_exclude() {
        let mut desired = BTreeSet::new();
        desired.insert(c("192.168.8.0/22"));
        let mut underlay = BTreeSet::new();
        underlay.insert(IpAddr::V4(Ipv4Addr::new(192, 168, 10, 101)));
        let local = [c("192.168.11.0/24")];
        assert!(needs_underlay_excludes(
            &desired,
            &underlay,
            Some(local.as_slice())
        ));
    }

    #[test]
    fn on_link_underlay_skips_exclude() {
        let mut desired = BTreeSet::new();
        desired.insert(c("192.168.8.0/22"));
        let mut underlay = BTreeSet::new();
        underlay.insert(IpAddr::V4(Ipv4Addr::new(192, 168, 11, 50)));
        let local = [c("192.168.11.0/24")];
        assert!(!needs_underlay_excludes(
            &desired,
            &underlay,
            Some(local.as_slice())
        ));
    }

    #[test]
    fn empty_known_local_nets_marks_all_covered_off_link() {
        let mut desired = BTreeSet::new();
        desired.insert(c("10.0.0.0/8"));
        let mut underlay = BTreeSet::new();
        underlay.insert(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)));
        let local: [cidr::Ipv4Cidr; 0] = [];
        assert!(needs_underlay_excludes(
            &desired,
            &underlay,
            Some(local.as_slice())
        ));
    }
}
