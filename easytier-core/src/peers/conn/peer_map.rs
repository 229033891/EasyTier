use std::{
    collections::{BTreeSet, HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

use anyhow::Context;
use dashmap::{DashMap, DashSet};
use parking_lot::Mutex;
use tokio::sync::RwLock;

use crate::{
    config::PeerId,
    packet::ZCPacket,
    peers::{
        context::{ArcPeerContext, NetworkIdentity, PeerEvent},
        error::Error,
        flow_table::FlowTable,
        util::shrink_dashmap,
    },
    proto::{
        core_peer::peer::{PeerConnInfo, Route as CoreRoute},
        peer_rpc::{
            DirectConnectedPeerInfo, PeerIdentityType, PeerInfoForGlobalMap, RoutePeerInfo,
        },
    },
    tunnel::TunnelError,
};

use super::{
    conn_select::{
        OSPF_EDGE_COST_MIN_DELTA, OSPF_EDGE_COST_UNMEASURED, apply_ospf_cost_hysteresis,
        ospf_edge_cost_from_score,
    },
    peer::Peer,
    peer_conn::{PeerConn, PeerConnId},
};
use crate::peers::{
    PacketRecvChan,
    route::{ArcRoute, NextHopPolicy},
};

pub struct PeerMap {
    context: ArcPeerContext,
    my_peer_id: PeerId,
    peer_map: DashMap<PeerId, Arc<Peer>>,
    packet_send: PacketRecvChan,
    routes: RwLock<Vec<ArcRoute>>,
    alive_client_urls: Arc<Mutex<HashMap<url::Url, HashSet<PeerConnId>>>>,
    /// Peers present in `peer_map` that do not yet have a connection attached.
    ///
    /// `PeerConn::start_recv_loop` awaits, so for a moment a freshly created peer
    /// legitimately has an empty `conns`. `clean_peer_without_conn` would read
    /// that as a leak and delete the peer out from under the connection being
    /// established, so this set tells it to leave those alone.
    connecting_peers: DashSet<PeerId>,
    /// Last published OSPF edge cost per peer (publisher hysteresis /
    /// anti-flap).
    ///
    /// **Per peer map, not per process.** The main instance and every foreign
    /// network run their own `PeerMap`, and their peer id sets are disjoint. A
    /// process-wide baseline made one map's `retain` wipe the other's entries
    /// — every foreign-network report therefore cleared the main instance's
    /// baseline, and the anti-flap debounce silently stopped working there.
    ospf_edge_cost_baseline: DashMap<PeerId, i32>,
    /// Per-tunnel IP flow accounting (disabled by default).
    flow_table: Arc<FlowTable>,
}

impl PeerMap {
    pub(crate) fn new(
        packet_send: PacketRecvChan,
        context: ArcPeerContext,
        my_peer_id: PeerId,
        flow_table: Arc<FlowTable>,
    ) -> Self {
        PeerMap {
            context,
            my_peer_id,
            peer_map: DashMap::new(),
            packet_send,
            routes: RwLock::new(Vec::new()),
            alive_client_urls: Arc::new(Mutex::new(HashMap::new())),
            connecting_peers: DashSet::new(),
            ospf_edge_cost_baseline: DashMap::new(),
            flow_table,
        }
    }

    pub async fn add_new_peer_conn(&self, peer_conn: PeerConn) -> Result<(), Error> {
        let _ = self.maintain_alive_client_urls(&peer_conn);
        let peer_id = peer_conn.get_peer_id();

        // CRITICAL: this used to do a `peer_map.get(..).is_none()` check, then
        // `await` inside `Peer::new(..).add_peer_conn(..)`, then a separate
        // `insert`. Two problems in that shape:
        //
        // 1. Two concurrent connections to the same peer could both observe
        //    "absent", each build a *private* `Peer`, attach its own conn, and
        //    then race to `insert` — the loser's connection was silently
        //    discarded, leaving a peer that reconnects forever.
        // 2. The `else` branch re-looked the peer up and `.unwrap()`ed it. The
        //    3s `clean_peer_without_conn` could remove the entry inside that
        //    window, turning a normal race into a panic (and, under
        //    `panic = "abort"`, a process abort).
        //
        // `entry().or_insert_with(..)` makes find-or-create atomic, and the
        // `Arc` is cloned out of the entry so the map guard — and its shard lock
        // — is released before any `.await`.
        let created = std::cell::Cell::new(false);
        let peer = {
            let entry = self.peer_map.entry(peer_id).or_insert_with(|| {
                created.set(true);
                Arc::new(Peer::new(
                    peer_id,
                    self.packet_send.clone(),
                    self.context.clone(),
                    self.flow_table.clone(),
                ))
            });
            entry.clone()
        };
        let created = created.get();
        if created {
            // Protect the conn-less window below from `clean_peer_without_conn`.
            self.connecting_peers.insert(peer_id);
        }

        let add_result = peer.add_peer_conn(peer_conn).await;

        // The peer has a connection (or failed to get one); either way it is no
        // longer in the "under construction" state.
        self.connecting_peers.remove(&peer_id);

        if let Err(e) = add_result {
            // Only undo the entry we created; an existing peer must survive a
            // rejected connection.
            if created {
                self.peer_map.remove(&peer_id);
            }
            return Err(e);
        }

        if created {
            self.context.issue_event(PeerEvent::PeerAdded(peer_id));
        }
        Ok(())
    }

    fn maintain_alive_client_urls(&self, peer_conn: &PeerConn) -> Option<()> {
        let conn_info = peer_conn.get_conn_info();
        if !conn_info.is_client {
            return None;
        }

        let close_notifier = peer_conn.get_close_notifier();
        let alive_conns_weak = Arc::downgrade(&self.alive_client_urls);
        let conn_id = close_notifier.get_conn_id();
        let alive_client_url: url::Url = conn_info.tunnel?.remote_addr?.into();
        self.alive_client_urls
            .lock()
            .entry(alive_client_url.clone())
            .or_default()
            .insert(conn_id);

        tokio::spawn(async move {
            if let Some(mut waiter) = close_notifier.get_waiter().await {
                let _ = waiter.recv().await;
            }
            let Some(alive_conns) = alive_conns_weak.upgrade() else {
                return;
            };
            let mut guard = alive_conns.lock();
            if let Some(conn_ids) = guard.get_mut(&alive_client_url) {
                conn_ids.retain(|id| id != &conn_id);
                if conn_ids.is_empty() {
                    guard.remove(&alive_client_url);
                }
            }
            let alive_conn_count = guard.len();
            drop(guard);
            tracing::debug!(
                ?conn_id,
                "peer conn is closed, current alive conns: {}",
                alive_conn_count
            );
        });

        Some(())
    }

    pub fn is_client_url_alive(&self, url: &url::Url) -> bool {
        self.alive_client_urls.lock().contains_key(url)
    }

    pub fn get_peer_by_id(&self, peer_id: PeerId) -> Option<Arc<Peer>> {
        self.peer_map.get(&peer_id).map(|v| v.clone())
    }

    /// Remote static public key of `peer_id`, or `None` when the peer is gone
    /// or its conns carried no key (plaintext / no-noise).
    ///
    /// `PeerId` is a random `u32` reported by the peer itself, so it is
    /// **not** a stable identity: it changes when the node restarts and the
    /// number can be reused by an unrelated node. The static key is the stable
    /// one — `Peer::add_conn` already refuses two conns for one id whose keys
    /// disagree — so callers that must remember "this exact node" (the
    /// `failover_when = never` pin) key on this.
    ///
    /// An **empty** key normalises to `None`. `Peer::add_conn` stores whatever
    /// `noise_remote_static_pubkey` held, and for a plaintext conn that is an
    /// empty `Vec` rather than an absent one — the same thing the rest of the
    /// repo treats as "no key" via `!…is_empty()`. Passing the empty `Vec` on
    /// would be worse than returning `None`: two different plaintext peers
    /// would compare **equal** by key.
    pub fn peer_public_key(&self, peer_id: PeerId) -> Option<Vec<u8>> {
        self.get_peer_by_id(peer_id)?
            .get_peer_public_key()
            .filter(|key| !key.is_empty())
    }

    pub fn get_directly_connections_by_peer_id(&self, peer_id: PeerId) -> DashSet<uuid::Uuid> {
        if let Some(peer) = self.get_peer_by_id(peer_id) {
            return peer.get_directly_connections();
        }

        DashSet::new()
    }

    pub fn has_peer(&self, peer_id: PeerId) -> bool {
        peer_id == self.my_peer_id || self.peer_map.contains_key(&peer_id)
    }

    /// Whether a packet can actually be handed to `peer_id` right now: either it
    /// is us (the self-send path), or the map entry still holds ≥1 live PeerConn.
    ///
    /// `has_peer` is `contains_key` and stays true after the last conn dies
    /// (the entry is only reaped by the close listener). Forwarding decisions
    /// must NOT use it as an availability check: a dead entry passes the gate,
    /// the send then fails deep inside `Peer::send_msg`, and — because the
    /// branch that holds the dead conn is matched first — the caller never
    /// falls through to the next usable path (relay). That is exactly the case
    /// where a relay leg *should* save the packet (P-AUDIT.4 / audit C3+C6).
    ///
    /// `get_gateway_peer_id` applies this in both places where it commits to an
    /// answer: the LeastHop direct short-circuit, and every candidate the route
    /// table names (see `select_next_hop`). A dead entry is therefore never
    /// selected — but it also never *blocks* a better answer, because the route
    /// table's equal-cost alternates are still consulted afterwards
    /// (roadmap §5.3).
    pub fn has_peer_with_live_conn(&self, peer_id: PeerId) -> bool {
        if peer_id == self.my_peer_id {
            return true;
        }
        // Clone the Arc and drop the map guard *before* touching the peer's own
        // `conns` map — same discipline as `add_new_peer_conn` a few lines up.
        // This runs once per forwarded packet, so it must not hold two DashMap
        // shard locks at the same time even if no current caller inverts the
        // order.
        match self.peer_map.get(&peer_id) {
            Some(peer) => {
                let peer = peer.clone();
                peer.has_live_conns()
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn has_direct_attached_peer(&self, peer_id: PeerId) -> bool {
        self.get_peer_by_id(peer_id)
            .is_some_and(|peer| peer.has_direct_attached_conn())
    }

    pub(crate) fn is_self(&self, peer_id: PeerId) -> bool {
        peer_id == self.my_peer_id
    }

    pub async fn send_msg_directly(&self, msg: ZCPacket, dst_peer_id: PeerId) -> Result<(), Error> {
        if dst_peer_id == self.my_peer_id {
            let packet_send = self.packet_send.clone();
            tokio::spawn(async move {
                let ret = packet_send
                    .send(msg)
                    .await
                    .with_context(|| "send msg to self failed");
                if ret.is_err() {
                    tracing::error!("send msg to self failed: {:?}", ret);
                }
            });
            return Ok(());
        }

        match self.get_peer_by_id(dst_peer_id) {
            Some(peer) => {
                peer.send_msg(msg).await?;
            }
            None => {
                tracing::error!("no peer for dst_peer_id: {}", dst_peer_id);
                return Err(Error::RouteError(Some(format!(
                    "peer map sengmsg directly no connected dst_peer_id: {}",
                    dst_peer_id
                ))));
            }
        }

        Ok(())
    }

    pub async fn get_gateway_peer_id(
        &self,
        dst_peer_id: PeerId,
        policy: NextHopPolicy,
    ) -> Option<PeerId> {
        if dst_peer_id == self.my_peer_id {
            return Some(dst_peer_id);
        }

        // LeastHop short-circuit: a direct conn is the fewest-hop path. It is only
        // a *usable* answer if we can actually hand the packet over. Returning a
        // lingering-but-dead entry would shadow the route table **and its
        // alternates** below, which is the one case where the table could still
        // offer a live path (roadmap §5.3). For a live dst this is unchanged.
        if matches!(policy, NextHopPolicy::LeastHop) && self.has_peer_with_live_conn(dst_peer_id) {
            return Some(dst_peer_id);
        }

        // The candidate list is ordered by the route: primary first, then
        // equal-cost alternates. Take the first one we can actually hand a
        // packet to right now, so a dead primary falls through to an
        // already-established alternate instead of waiting for the topology to
        // converge (roadmap `live-next-hop.md` Phase B/D).
        for route in self.routes.read().await.iter() {
            let candidates = route
                .get_next_hops_with_policy(dst_peer_id, policy.clone())
                .await;
            let chosen = select_next_hop(self.my_peer_id, &candidates, |gateway_peer_id| {
                // NOTIC: for foreign network, gateway_peer_id may not connect to me
                let hop_in_peer_map = self.has_peer(gateway_peer_id);
                let hop_live = self.has_peer_with_live_conn(gateway_peer_id);
                if hop_in_peer_map && !hop_live {
                    // A stale entry must not win: `select_next_hop` tries the next
                    // candidate, and if none is usable falls through to `None` so
                    // the caller fails fast instead of handing the packet to a
                    // dead peer.
                    tracing::debug!(
                        gateway = gateway_peer_id,
                        dst = dst_peer_id,
                        "route names a next hop with no live conn, skipping it"
                    );
                } else if !hop_in_peer_map {
                    // Could be a foreign-network gateway, or a peer reaped since
                    // the route was learnt. Only the caller can tell them apart,
                    // so this candidate may not win unless it is the last resort.
                    tracing::debug!(
                        gateway = gateway_peer_id,
                        dst = dst_peer_id,
                        "route names a next hop with no local peer entry, \
                         deferring to a locally sendable candidate if one exists"
                    );
                }
                (hop_in_peer_map, hop_live)
            });
            if let Some(gateway_peer_id) = chosen {
                return Some(gateway_peer_id);
            }
        }

        None
    }

    pub async fn list_peers_own_foreign_network(
        &self,
        network_identity: &NetworkIdentity,
    ) -> Vec<PeerId> {
        let mut ret = Vec::new();
        for route in self.routes.read().await.iter() {
            let peers = route.list_peers_own_foreign_network(network_identity).await;
            ret.extend(peers);
        }
        ret
    }

    pub async fn send_msg(
        &self,
        msg: ZCPacket,
        dst_peer_id: PeerId,
        policy: NextHopPolicy,
    ) -> Result<(), Error> {
        let Some(gateway_peer_id) = self.get_gateway_peer_id(dst_peer_id, policy).await else {
            return Err(Error::RouteError(Some(format!(
                "peer map sengmsg no gateway for dst_peer_id: {}",
                dst_peer_id
            ))));
        };

        self.send_msg_directly(msg, gateway_peer_id).await?;
        Ok(())
    }

    pub async fn get_peer_id_by_ipv4_allowing_default_proxy(
        &self,
        ipv4: &Ipv4Addr,
    ) -> Option<PeerId> {
        if let Some(peer_id) = self.get_peer_id_by_ipv4(ipv4).await {
            return Some(peer_id);
        }
        let ip = IpAddr::V4(*ipv4);
        for route in self.routes.read().await.iter() {
            if let Some(peer_id) = route.get_peer_id_for_default_route_proxy(&ip).await {
                return Some(peer_id);
            }
        }
        None
    }

    pub async fn get_peer_id_by_ipv4(&self, ipv4: &Ipv4Addr) -> Option<PeerId> {
        for route in self.routes.read().await.iter() {
            let peer_id = route.get_peer_id_by_ipv4(ipv4).await;
            if peer_id.is_some() {
                return peer_id;
            }
        }
        None
    }

    pub async fn get_peer_id_by_ipv6(&self, ipv6: &Ipv6Addr) -> Option<PeerId> {
        for route in self.routes.read().await.iter() {
            let peer_id = route.get_peer_id_by_ipv6(ipv6).await;
            if peer_id.is_some() {
                return peer_id;
            }
        }
        None
    }

    /// Resolve every candidate in list order to a *reachable* `PeerId`.
    ///
    /// Same IP→peer mapping and next-hop check as `resolve_exit_node_peer`, but
    /// returns all matches so `select_by = quality` can rank them (P4).
    pub(crate) async fn resolve_exit_node_peers(&self, exit_nodes: &[IpAddr]) -> Vec<PeerId> {
        let mut out = Vec::with_capacity(exit_nodes.len());
        for exit_node in exit_nodes {
            let peer_id = match exit_node {
                IpAddr::V4(ip) => self.get_peer_id_by_ipv4(ip).await,
                IpAddr::V6(ip) => self.get_peer_id_by_ipv6(ip).await,
            };
            let Some(peer_id) = peer_id else {
                continue;
            };
            if self
                .get_gateway_peer_id(peer_id, NextHopPolicy::LeastHop)
                .await
                .is_some()
            {
                out.push(peer_id);
            }
        }
        out
    }

    pub async fn resolve_exit_node_peer(&self, exit_nodes: &[IpAddr]) -> Option<PeerId> {
        self.resolve_exit_node_peers(exit_nodes)
            .await
            .into_iter()
            .next()
    }

    /// P4: best composite edge cost for `peer_id`, or `None` when it has no
    /// usable connection.
    ///
    /// This is the cost of a **direct** link only. A peer that is reachable
    /// solely through a relay has no direct connection at all, so this returns
    /// `None` for it — use [`Self::peer_path_cost`] when the whole path has to
    /// be priced.
    ///
    /// No publish hysteresis here — the caller keeps its own baseline (the
    /// reconcile loop stores the previous costs in `peer_quality` and applies
    /// `apply_ospf_cost_hysteresis` itself, so OSPF's publisher state is
    /// untouched).
    pub(crate) async fn peer_edge_cost(&self, peer_id: PeerId) -> Option<i32> {
        best_edge_cost(self.list_peer_conns(peer_id).await)
    }

    /// P4: accumulated cost of the cheapest path to `peer_id`, relay hops
    /// included, or `None` when the route table has no path to it yet.
    ///
    /// This is the price a consumer needs when it compares two candidates it
    /// can reach over paths of different lengths: a peer that is only
    /// reachable through a relay still has a price, and it is the price of
    /// the path that will actually be used.
    ///
    /// `None` means "the topology does not know this peer yet" — the caller
    /// may fall back to the direct-link cost for that window.
    pub(crate) async fn peer_path_cost(&self, peer_id: PeerId) -> Option<i32> {
        for route in self.routes.read().await.iter() {
            if let Some(cost) = route
                .get_path_cost_with_policy(peer_id, NextHopPolicy::LeastCost)
                .await
            {
                return Some(cost);
            }
        }
        None
    }

    /// Apply the publisher hysteresis for `peer` and record what was published.
    ///
    /// The baseline lives on this peer map, so a peer map that never sees a
    /// peer cannot clear another one's baseline.
    pub(crate) fn ospf_edge_cost_with_hysteresis(&self, peer: PeerId, raw_cost: i32) -> i32 {
        let last = self.ospf_edge_cost_baseline.get(&peer).map(|v| *v);
        let cost = apply_ospf_cost_hysteresis(raw_cost, last, OSPF_EDGE_COST_MIN_DELTA);
        self.ospf_edge_cost_baseline.insert(peer, cost);
        cost
    }

    /// Drop the hysteresis baseline for the peers this map no longer
    /// advertises. Only this map's baseline is touched.
    pub(crate) fn retain_ospf_edge_cost_baseline(&self, advertised: &BTreeSet<PeerId>) {
        self.ospf_edge_cost_baseline
            .retain(|peer_id, _| advertised.contains(peer_id));
    }

    pub async fn get_route_peer_info(&self, peer_id: PeerId) -> Option<RoutePeerInfo> {
        for route in self.routes.read().await.iter() {
            if let Some(info) = route.get_peer_info(peer_id).await {
                return Some(info);
            }
        }
        None
    }

    pub async fn get_origin_my_peer_id(
        &self,
        network_name: &str,
        foreign_my_peer_id: PeerId,
    ) -> Option<PeerId> {
        for route in self.routes.read().await.iter() {
            let origin_peer_id = route
                .get_origin_my_peer_id(network_name, foreign_my_peer_id)
                .await;
            if origin_peer_id.is_some() {
                return origin_peer_id;
            }
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.peer_map.is_empty()
    }

    pub fn list_peers(&self) -> Vec<PeerId> {
        let mut ret = Vec::new();
        for item in self.peer_map.iter() {
            let peer_id = item.key();
            ret.push(*peer_id);
        }
        ret
    }

    pub async fn list_peers_with_conn(&self) -> Vec<PeerId> {
        let mut ret = Vec::new();
        for item in self.peer_map.iter() {
            if item.value().has_live_conns() {
                ret.push(*item.key());
            }
        }
        ret
    }

    /// Peers we are still connected to that are carrying nothing: ≥1 live conn,
    /// but no live conn that is not *sustainedly* fast-probe degraded.
    ///
    /// These are the peers whose **own outgoing edge** the local SPF must stop
    /// pricing at face value. They are deliberately still advertised to other
    /// nodes — a link that is degraded but not dead should stay in everyone
    /// else's table as a last resort, just with the inflated cost the peer-center
    /// already carries (the alternative, dropping the edge from what we advertise,
    /// deletes a usable path from other nodes' tables).
    pub async fn list_peers_without_usable_conn(&self) -> Vec<PeerId> {
        let mut ret = Vec::new();
        for item in self.peer_map.iter() {
            let peer = item.value();
            if peer.has_live_conns() && !peer.has_usable_conns() {
                ret.push(*item.key());
            }
        }
        ret
    }

    pub async fn list_peer_conns(&self, peer_id: PeerId) -> Option<Vec<PeerConnInfo>> {
        if let Some(p) = self.get_peer_by_id(peer_id) {
            Some(p.list_peer_conns().await)
        } else {
            None
        }
    }

    pub async fn get_peer_default_conn_id(&self, peer_id: PeerId) -> Option<PeerConnId> {
        self.get_peer_by_id(peer_id)
            .map(|p| p.get_default_conn_id())
    }

    pub fn get_peer_identity_type(&self, peer_id: PeerId) -> Option<PeerIdentityType> {
        self.get_peer_by_id(peer_id)
            .and_then(|p| p.get_peer_identity_type())
    }

    pub fn get_peer_public_key(&self, peer_id: PeerId) -> Option<Vec<u8>> {
        self.get_peer_by_id(peer_id)
            .and_then(|p| p.get_peer_public_key())
    }

    pub async fn close_peer_conn(
        &self,
        peer_id: PeerId,
        conn_id: &PeerConnId,
    ) -> Result<(), Error> {
        if let Some(p) = self.get_peer_by_id(peer_id) {
            p.close_peer_conn(conn_id).await
        } else {
            Err(Error::NotFound)
        }
    }

    pub async fn close_peer(&self, peer_id: PeerId) -> Result<(), TunnelError> {
        let remove_ret = self.peer_map.remove(&peer_id);
        shrink_dashmap(&self.peer_map, None);

        self.context.issue_event(PeerEvent::PeerRemoved(peer_id));
        tracing::info!(
            ?peer_id,
            has_old_value = ?remove_ret.is_some(),
            peer_ref_counter = ?remove_ret.map(|v| Arc::strong_count(&v.1)),
            "peer is closed"
        );
        Ok(())
    }

    pub async fn add_route(&self, route: ArcRoute) {
        let mut routes = self.routes.write().await;
        routes.insert(0, route);
    }

    pub(crate) async fn clear_resources(&self) {
        for peer_id in self.list_peers() {
            let _ = self.close_peer(peer_id).await;
        }
        let routes = {
            let mut routes = self.routes.write().await;
            std::mem::take(&mut *routes)
        };
        for route in routes {
            route.close().await;
        }
        self.alive_client_urls.lock().clear();
    }

    pub async fn clean_peer_without_conn(&self) {
        let mut to_remove = vec![];

        for peer_id in self.list_peers() {
            // A peer that is still having its first connection attached has no
            // conns yet; that is transient, not a leak.
            if self.connecting_peers.contains(&peer_id) {
                continue;
            }
            let conns = self.list_peer_conns(peer_id).await;
            if conns.is_none() || conns.as_ref().unwrap().is_empty() {
                to_remove.push(peer_id);
            }
        }

        for peer_id in to_remove {
            self.close_peer(peer_id).await.unwrap();
        }
    }

    pub async fn list_routes(&self) -> DashMap<PeerId, PeerId> {
        let route_map = DashMap::new();
        for route in self.routes.read().await.iter() {
            for item in route.list_routes().await.iter() {
                route_map.insert(item.peer_id, item.next_hop_peer_id);
            }
        }
        route_map
    }

    pub async fn list_route_infos(&self) -> Vec<CoreRoute> {
        if let Some(route) = self.routes.read().await.iter().next() {
            return route.list_routes().await;
        }
        vec![]
    }

    pub async fn need_relay_by_foreign_network(&self, dst_peer_id: PeerId) -> Result<bool, Error> {
        // if gateway_peer_id is not connected to me, means need relay by foreign network
        let gateway_id = self
            .get_gateway_peer_id(dst_peer_id, NextHopPolicy::LeastHop)
            .await
            .ok_or(Error::RouteError(Some(format!(
                "peer map need_relay_by_foreign_network no gateway for dst_peer_id: {}",
                dst_peer_id
            ))))?;

        Ok(!self.has_peer(gateway_id))
    }

    pub fn my_peer_id(&self) -> PeerId {
        self.my_peer_id
    }
}

impl Drop for PeerMap {
    fn drop(&mut self) {
        tracing::debug!(
            self.my_peer_id,
            network = ?self.context.network_identity(),
            "PeerMap is dropped"
        );
    }
}

/// Best (lowest) composite edge cost across a peer's usable connections.
///
/// Same score family as `select_conn`; unverified hole-punch connections
/// (latency 0) are excluded because they are not eligible for `default_conn`
/// and must not be advertised as the best edge for OSPF either.
fn best_edge_cost(conns: Option<Vec<PeerConnInfo>>) -> Option<i32> {
    conns
        .into_iter()
        .flatten()
        .filter(|conn| !conn.unverified_hole_punch)
        .map(|conn| {
            // A connection that has not completed a single ping round trip has
            // a quality score of exactly zero — that means "not measured",
            // not "perfect". The score can only be 0.0 when `latency_us` is 0
            // (the smallest measured RTT is 1µs, i.e. a score of 1e-6), so this
            // cannot swallow a real measurement. The `is_finite` arm also
            // catches a NaN score.
            if !conn.quality_score.is_finite() || conn.quality_score <= 0.0 {
                OSPF_EDGE_COST_UNMEASURED
            } else {
                ospf_edge_cost_from_score(f64::from(conn.quality_score), conn.quality_fused)
            }
        })
        .min()
}

/// Aggregates the directly-connected peers of several peer maps into the
/// peer-center reporting format.
///
/// Edge cost is the best (lowest) quality-encoded cost across PeerConns for
/// that peer — same score family as `select_conn` (RTT + loss + jitter), with
/// a fuse bonus and publisher hysteresis so OSPF / `latency_first` does not
/// flap on noise. The field remains named `latency_ms` for wire compatibility.
pub(crate) async fn direct_peer_info(peer_maps: &[Arc<PeerMap>]) -> PeerInfoForGlobalMap {
    let mut ret = PeerInfoForGlobalMap::default();
    for peer_map in peer_maps {
        // The hysteresis baseline is per peer map, so each map applies and
        // prunes its own. `advertised` is what this map contributes; the
        // baseline of every *other* map is left alone.
        let mut advertised = BTreeSet::new();
        for peer in peer_map.list_peers() {
            // The first peer map that can reach a peer owns its edge cost, so
            // the reported value does not depend on the order of `peer_maps`.
            if ret.direct_peers.contains_key(&peer) {
                continue;
            }
            let conns = peer_map.list_peer_conns(peer).await;
            // `list_peer_conns` attaches `quality_score` / `quality_fused` from the
            // same formula as select_conn; take the cheapest usable edge.
            // Exclude unverified hole-punch connections (latency 0) — they are not
            // eligible for default_conn in select_conn and should not be advertised
            // as the best edge for OSPF either.
            let Some(raw_cost) = best_edge_cost(conns) else {
                continue;
            };
            let cost = peer_map.ospf_edge_cost_with_hysteresis(peer, raw_cost);
            advertised.insert(peer);
            ret.direct_peers
                .insert(peer, DirectConnectedPeerInfo { latency_ms: cost });
        }
        // Drop hysteresis state for peers this map no longer advertises.
        peer_map.retain_ospf_edge_cost_baseline(&advertised);
    }

    ret
}

/// Pick the next hop from an ordered candidate list (primary first, then
/// equal-cost alternates).
///
/// Split out of `PeerMap::get_gateway_peer_id` as a pure function so the rule is
/// testable without building a peer map or a route table. The liveness lookup is
/// a callback rather than a parameter so it stays **lazy**: the loop stops at the
/// first usable candidate, because this runs once per forwarded packet.
///
/// `probe(hop) -> (hop_in_peer_map, hop_live)`.
///
/// # The rule, in order
///
/// 1. `hop == my_id` — the self-send path always works.
/// 2. Entry present **and** ≥1 live PeerConn. `has_peer` is `contains_key` and
///    outlives the last conn, so the live check is the only thing separating a
///    usable hop from a stale one. Without it, a route pointing at a dead entry
///    wins, the send fails inside `Peer::send_msg`, and — because that branch is
///    matched first — the caller never falls through to another usable path
///    (P-AUDIT.4 / audit C3+C6).
/// 3. No local entry — **last resort only**.
///
/// # Why tier 3 may not win outright
///
/// Two unrelated things look identical from inside a `PeerMap`:
///
/// * a **foreign-network gateway**, which has no local entry *by design* and is
///   picked up at send time by `foreign_network_client.has_next_hop` (the `NOTIC`
///   at the call site), and
/// * a route entry naming a peer that has since been **reaped** from the peer map
///   — `clean_peer_without_conn` → `close_peer` removes the entry, while route
///   entries naming it are retained for `REMOVE_UNREACHABLE_PEER_INFO_AFTER`.
///
/// Only the caller can tell them apart. Returning tier 3 before tier 2 was found
/// would let a hop that is guaranteed to fail at send time shadow the equal-cost
/// alternates, i.e. the Phase B/D fallback would be unreachable exactly in the
/// case it exists for — the primary being fully gone, entry and all.
///
/// When tier 3 is all that is left we still return it, so foreign-network
/// routing keeps its pre-existing behaviour, and a stale hop degrades to the
/// pre-existing "drop with a warning" rather than a new failure mode.
///
/// This deliberately does **not** invent a next hop that is not on a path to the
/// destination: that is a misroute (roadmap P1/P2). `None` makes the caller fail
/// fast instead.
fn select_next_hop<P>(my_id: PeerId, candidates: &[PeerId], mut probe: P) -> Option<PeerId>
where
    P: FnMut(PeerId) -> (bool, bool),
{
    let mut no_local_entry = None;
    for &hop in candidates {
        let (hop_in_peer_map, hop_live) = probe(hop);
        if hop == my_id || (hop_in_peer_map && hop_live) {
            return Some(hop);
        }
        if !hop_in_peer_map {
            no_local_entry = no_local_entry.or(Some(hop));
        }
    }
    no_local_entry
}

#[cfg(test)]
mod select_next_hop_tests {
    use super::select_next_hop;

    const ME: u32 = 1;
    const PRIMARY: u32 = 2;
    const ALTERNATE: u32 = 3;
    const REAPED: u32 = 4;

    /// Probe over a `(peer_id, (in_peer_map, live))` table. Peers missing from
    /// the table answer "no local entry", i.e. the tier-3 ambiguity.
    fn table(entries: &[(u32, (bool, bool))]) -> impl FnMut(u32) -> (bool, bool) + '_ {
        move |peer_id| {
            entries
                .iter()
                .find(|(id, _)| *id == peer_id)
                .map_or((false, false), |(_, flags)| *flags)
        }
    }

    #[test]
    fn self_is_always_sendable() {
        assert_eq!(select_next_hop(ME, &[ME], table(&[])), Some(ME));
    }

    #[test]
    fn live_local_next_hop_wins() {
        let hop = select_next_hop(ME, &[PRIMARY], table(&[(PRIMARY, (true, true))]));
        assert_eq!(hop, Some(PRIMARY));
    }

    #[test]
    fn dead_entry_is_skipped_for_the_next_candidate() {
        let hop = select_next_hop(
            ME,
            &[PRIMARY, ALTERNATE],
            table(&[(PRIMARY, (true, false)), (ALTERNATE, (true, true))]),
        );
        assert_eq!(hop, Some(ALTERNATE));
    }

    /// Regression: a reaped primary used to be accepted as "sendable" because it
    /// is absent from the peer map, which shadowed the live alternate and made
    /// the packet get dropped at send time.
    #[test]
    fn reaped_primary_does_not_shadow_a_live_alternate() {
        let hop = select_next_hop(
            ME,
            &[REAPED, ALTERNATE],
            table(&[(REAPED, (false, false)), (ALTERNATE, (true, true))]),
        );
        assert_eq!(hop, Some(ALTERNATE));
    }

    #[test]
    fn reaped_primary_does_not_shadow_a_later_reaped_hop_either() {
        // Nothing is locally sendable, so tier 3 decides — but it must be the
        // *first* such candidate, so behaviour matches the pre-change code.
        let hop = select_next_hop(ME, &[REAPED, ALTERNATE], table(&[]));
        assert_eq!(hop, Some(REAPED));
    }

    #[test]
    fn foreign_network_gateway_is_still_returned_as_a_last_resort() {
        // Absence is not proof of a dead link: the caller confirms it via the
        // foreign-network client. Keeping this tier preserves foreign routing.
        let hop = select_next_hop(ME, &[PRIMARY], table(&[]));
        assert_eq!(hop, Some(PRIMARY));
    }

    #[test]
    fn all_dead_entries_yield_none() {
        let hop = select_next_hop(
            ME,
            &[PRIMARY, ALTERNATE],
            table(&[(PRIMARY, (true, false)), (ALTERNATE, (true, false))]),
        );
        assert_eq!(hop, None);
    }

    #[test]
    fn empty_candidate_list_yields_none() {
        assert_eq!(select_next_hop(ME, &[], table(&[])), None);
    }

    #[test]
    fn probe_short_circuits_on_the_first_usable_candidate() {
        // This runs once per forwarded packet, so probing must not be
        // unconditional (that would cost 2 map lookups per unused candidate).
        let mut probes = 0;
        let hop = select_next_hop(ME, &[PRIMARY, ALTERNATE, REAPED], |peer_id| {
            probes += 1;
            match peer_id {
                PRIMARY => (true, true),
                _ => (true, false),
            }
        });
        assert_eq!(hop, Some(PRIMARY));
        assert_eq!(probes, 1);
    }
}
