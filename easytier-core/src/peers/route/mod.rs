//! Route trait surface shared by the peer domain, plus the OSPF route
//! implementation and the graph algorithms backing it.

pub(crate) mod graph_algo;
pub(crate) mod local_route_table;
pub(crate) mod peer_ospf_route;
mod route_peer_wire;

use cidr::Ipv6Inet;
use cidr::{Ipv4Cidr, Ipv6Cidr};
use dashmap::DashMap;
use quanta::Instant;
use smallvec::SmallVec;
use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

/// How many **equal-cost alternates** a route may offer in addition to the
/// primary next hop.
///
/// Only equal-cost alternates are offered: that is the case with a local
/// loop-freedom proof (see `graph_algo::equal_cost_first_hops`). A
/// cheaper-or-within-a-margin alternate needs the neighbour's own distance to
/// the destination — the RFC 8402 LFA computation, deliberately not attempted
/// here (roadmap `live-next-hop.md` §5.2).
pub const MAX_ALTERNATE_NEXT_HOPS: usize = 2;

/// Next-hop candidates for one destination: the primary first, then alternates.
/// Inline so the per-packet walk allocates nothing.
pub type NextHopCandidates = SmallVec<[PeerId; MAX_ALTERNATE_NEXT_HOPS + 1]>;

use crate::{
    config::PeerId,
    peers::context::NetworkIdentity,
    proto::{
        core_peer::peer::{ListPublicIpv6InfoResponse, Route as CoreRoute},
        peer_rpc::{
            ForeignNetworkRouteInfoEntry, ForeignNetworkRouteInfoKey, PeerIdentityType,
            RouteForeignNetworkInfos, RouteForeignNetworkSummary, RoutePeerInfo,
        },
    },
};

#[derive(Clone, Debug, Default)]
pub enum NextHopPolicy {
    #[default]
    LeastHop,
    LeastCost,
}

pub type ForeignNetworkRouteInfoMap =
    DashMap<ForeignNetworkRouteInfoKey, ForeignNetworkRouteInfoEntry>;

#[async_trait::async_trait]
pub trait RouteInterface {
    async fn list_peers(&self) -> Vec<PeerId>;
    /// Peers we are connected to whose live conns are all *sustainedly*
    /// fast-probe degraded — "connected, but carrying nothing".
    ///
    /// The OSPF builder re-prices **our own outgoing edges** to these peers, so a
    /// black-holing link stops winning the local SPF immediately instead of
    /// waiting for the peer-center to hand back an inflated cost.
    ///
    /// Defaults to empty, which is the safe direction: an implementation that
    /// cannot answer simply does not get the local override.
    async fn list_peers_without_usable_conn(&self) -> Vec<PeerId> {
        Vec::new()
    }
    fn my_peer_id(&self) -> PeerId;
    fn need_periodic_requery_peers(&self) -> bool {
        false
    }
    async fn close_peer(&self, _peer_id: PeerId) {}
    async fn get_peer_public_key(&self, _peer_id: PeerId) -> Option<Vec<u8>> {
        None
    }
    async fn get_peer_identity_type(&self, _peer_id: PeerId) -> Option<PeerIdentityType> {
        None
    }
    async fn list_foreign_networks(&self) -> ForeignNetworkRouteInfoMap {
        DashMap::new()
    }
}

pub type RouteInterfaceBox = Box<dyn RouteInterface + Send + Sync>;

#[auto_impl::auto_impl(Box , &mut)]
pub trait RouteCostCalculatorInterface: Send + Sync {
    fn begin_update(&mut self) {}
    fn end_update(&mut self) {}

    fn calculate_cost(&self, _src: PeerId, _dst: PeerId) -> i32 {
        1
    }

    fn need_update(&self) -> bool {
        false
    }

    fn dump(&self) -> String {
        "All routes have cost 1".to_string()
    }
}

#[derive(Clone, Debug, Default)]
pub struct DefaultRouteCostCalculator;

impl RouteCostCalculatorInterface for DefaultRouteCostCalculator {}

pub type RouteCostCalculator = Box<dyn RouteCostCalculatorInterface>;

#[async_trait::async_trait]
#[auto_impl::auto_impl(Box, Arc)]
pub trait Route {
    async fn open(&self, interface: RouteInterfaceBox) -> Result<u8, ()>;
    async fn close(&self);

    async fn get_next_hop(&self, peer_id: PeerId) -> Option<PeerId>;
    async fn get_next_hop_with_policy(
        &self,
        peer_id: PeerId,
        _policy: NextHopPolicy,
    ) -> Option<PeerId> {
        self.get_next_hop(peer_id).await
    }

    /// Every locally known next hop for `peer_id`: the primary first, then
    /// equal-cost alternates.
    ///
    /// The send path walks this in order and takes the first entry it can
    /// actually hand a packet to right now, so a dead primary can fall through
    /// to an already-established alternate without waiting for the topology to
    /// converge.
    ///
    /// Defaults to just the single next hop, so implementations that do not
    /// compute alternates keep working unchanged.
    async fn get_next_hops_with_policy(
        &self,
        peer_id: PeerId,
        policy: NextHopPolicy,
    ) -> NextHopCandidates {
        self.get_next_hop_with_policy(peer_id, policy)
            .await
            .into_iter()
            .collect()
    }

    async fn list_routes(&self) -> Vec<CoreRoute>;

    // TODO: rewrite route management, remove this
    async fn list_proxy_cidrs(&self) -> BTreeSet<Ipv4Cidr>;

    // TODO: rewrite route management, remove this
    async fn list_proxy_cidrs_v6(&self) -> BTreeSet<Ipv6Cidr>;

    async fn list_public_ipv6_routes(&self) -> BTreeSet<Ipv6Inet> {
        BTreeSet::new()
    }

    async fn get_my_public_ipv6_addr(&self) -> Option<Ipv6Inet> {
        None
    }

    async fn get_public_ipv6_gateway_peer_id(&self) -> Option<PeerId> {
        None
    }

    async fn get_local_public_ipv6_info(&self) -> ListPublicIpv6InfoResponse {
        ListPublicIpv6InfoResponse::default()
    }

    async fn get_peer_id_by_ipv4(&self, _ipv4: &Ipv4Addr) -> Option<PeerId> {
        None
    }

    async fn get_peer_id_by_ipv6(&self, _ipv6: &Ipv6Addr) -> Option<PeerId> {
        None
    }

    /// Longest-prefix proxy match that is exactly `0.0.0.0/0` or `::/0`.
    /// Used only after VIP, more-specific CIDRs, and `exit_nodes` have missed.
    async fn get_peer_id_for_default_route_proxy(&self, _ip: &std::net::IpAddr) -> Option<PeerId> {
        None
    }

    async fn get_peer_id_by_ip(&self, ip: &std::net::IpAddr) -> Option<PeerId> {
        match ip {
            std::net::IpAddr::V4(v4) => self.get_peer_id_by_ipv4(v4).await,
            std::net::IpAddr::V6(v6) => self.get_peer_id_by_ipv6(v6).await,
        }
    }

    /// VIP / more-specific proxy CIDR first, then peer-advertised `/0`/`::/0`.
    ///
    /// Used by ACL / KCP / QUIC / wrapped-TCP service paths that need the owning
    /// peer even when only a default-route proxy matches. Must **not** be used by
    /// the L3 outbound packet router, which keeps `exit_nodes` ahead of peer `/0`.
    async fn get_peer_id_by_ip_allowing_default_proxy(
        &self,
        ip: &std::net::IpAddr,
    ) -> Option<PeerId> {
        if let Some(peer_id) = self.get_peer_id_by_ip(ip).await {
            return Some(peer_id);
        }
        self.get_peer_id_for_default_route_proxy(ip).await
    }

    async fn list_peers_own_foreign_network(
        &self,
        _network_identity: &NetworkIdentity,
    ) -> Vec<PeerId> {
        vec![]
    }

    async fn list_foreign_network_info(&self) -> RouteForeignNetworkInfos {
        Default::default()
    }

    async fn get_foreign_network_summary(&self) -> RouteForeignNetworkSummary {
        Default::default()
    }

    // my peer id in foreign network is different from the one in local network
    // this function is used to get the peer id in local network
    async fn get_origin_my_peer_id(
        &self,
        _network_name: &str,
        _foreign_my_peer_id: PeerId,
    ) -> Option<PeerId> {
        None
    }

    async fn set_route_cost_fn(&self, _cost_fn: RouteCostCalculator) {}

    async fn get_peer_info(&self, peer_id: PeerId) -> Option<RoutePeerInfo>;

    async fn get_peer_info_last_update_time(&self) -> Instant;

    fn get_peer_groups(&self, peer_id: PeerId) -> Arc<Vec<String>>;

    async fn refresh_acl_groups(&self) {}

    async fn get_peer_groups_by_ip(&self, ip: &std::net::IpAddr) -> Arc<Vec<String>> {
        match self.get_peer_id_by_ip_allowing_default_proxy(ip).await {
            Some(peer_id) => self.get_peer_groups(peer_id),
            None => Arc::new(Vec::new()),
        }
    }

    async fn dump(&self) -> String {
        "this route implementation does not support dump".to_string()
    }
}

pub type ArcRoute = Arc<dyn Route + Send + Sync>;

pub(crate) struct DisabledRoute;

#[async_trait::async_trait]
impl Route for DisabledRoute {
    async fn open(&self, _interface: RouteInterfaceBox) -> Result<u8, ()> {
        panic!("mock route")
    }

    async fn close(&self) {
        panic!("mock route")
    }

    async fn get_next_hop(&self, _peer_id: PeerId) -> Option<PeerId> {
        panic!("mock route")
    }

    async fn list_routes(&self) -> Vec<CoreRoute> {
        panic!("mock route")
    }

    // TODO: rewrite route management, remove this
    async fn list_proxy_cidrs(&self) -> BTreeSet<Ipv4Cidr> {
        BTreeSet::new()
    }

    // TODO: rewrite route management, remove this
    async fn list_proxy_cidrs_v6(&self) -> BTreeSet<Ipv6Cidr> {
        BTreeSet::new()
    }

    async fn list_public_ipv6_routes(&self) -> BTreeSet<Ipv6Inet> {
        BTreeSet::new()
    }

    async fn get_my_public_ipv6_addr(&self) -> Option<Ipv6Inet> {
        panic!("mock route")
    }

    async fn get_peer_info(&self, _peer_id: PeerId) -> Option<RoutePeerInfo> {
        panic!("mock route")
    }

    async fn get_peer_info_last_update_time(&self) -> Instant {
        panic!("mock route")
    }

    fn get_peer_groups(&self, _peer_id: PeerId) -> Arc<Vec<String>> {
        panic!("mock route")
    }
}
