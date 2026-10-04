use std::{collections::BTreeSet, net::IpAddr, sync::Arc};

use arc_swap::ArcSwap;
use async_trait::async_trait;

use crate::{
    config::runtime::CoreRuntimeConfigStore,
    gateway::magic_dns::{MagicDnsRouteSnapshot, MagicDnsRouteSource},
    gateway::proxy::cidr_monitor::{
        ProxyCidrDiff, ProxyCidrRouteSyncStatus, collect_proxy_cidr_diff,
    },
    gateway::proxy::underlay_exclude::collect_underlay_exclude_ips,
    host::packet::HostPacket,
    peers::peer_manager::PeerManagerCore,
};

#[cfg(feature = "proxy-packet")]
use crate::foundation::stats::{LabelSet, LabelType, MetricName};
#[cfg(feature = "proxy-packet")]
use crate::gateway::magic_dns::{
    MagicDnsQueryResolver, MagicDnsResolverRegistration, magic_dns_packet_filter,
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

    pub fn proxy_cidr_route_sync_status(&self) -> ProxyCidrRouteSyncStatus {
        self.proxy_cidr_route_sync.load_full().as_ref().clone()
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
        let runtime = tokio::runtime::Handle::current();
        let pipeline = self
            .peer_manager
            .add_managed_nic_packet_process_pipeline(magic_dns_packet_filter(
                fake_ip,
                self.peer_manager.my_peer_id(),
                resolver,
            ))
            .await;
        MagicDnsResolverRegistration::new(Arc::downgrade(&self.peer_manager), pipeline, runtime)
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
