use std::time::{Duration, SystemTime};
use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Context;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use quanta::Instant;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use tokio::sync::{
    Mutex, RwLock,
    mpsc::{self, UnboundedReceiver, UnboundedSender},
};
use tokio::task::JoinSet;
use url::Url;

use crate::{
    config::{
        P2pPolicyFlags, PeerId, ProxyNetworkConfig,
        peers::{
            AclRuleConfig, HostRoutingPolicy, PeerGroupIdentity, PeerRuntimeConfig,
            PeerRuntimeSnapshot,
        },
        runtime::{CoreInstanceRuntimeConfig, CoreRuntimeConfigStore},
        toml::ManagedCredentialConfig,
    },
    events::CoreEventSink,
    foundation::task::ExternalTaskSignal,
    host::packet::{HostPacket, HostPacketSender},
    packet::{
        CompressorAlgo, PacketType, ZCPacket,
        compressor::{Compressor as _, DefaultCompressor},
    },
    proto::common::{FlagsInConfig, PeerFeatureFlag, StunInfo, Url as ProtoUrl},
    proto::core_peer::peer::{ListPublicIpv6InfoResponse, PeerConnInfo, Route as CoreRoute},
    tunnel::{
        Tunnel,
        encrypt::{
            Encryptor, NullCipher, create_encryptor, derive_key_128, derive_key_256,
            validate_algorithm,
        },
    },
};

use super::{
    BoxNicPacketFilter, BoxPeerPacketFilter, PacketRecvChanReceiver, PeerConnectionOrigin,
    PeerPacketFilter, PeerPacketIngress,
    acl::AclFilter,
    conn::{
        conn_bond::{
            BondConfig, bond_fill_live_ceiling, bond_fill_needed, bond_fill_needed_with_standby,
            flow_key_from_payload,
        },
        conn_select::{ConnSelectConfig, OSPF_EDGE_COST_MIN_DELTA, apply_ospf_cost_hysteresis},
        peer_conn::{PeerConn, PeerConnId},
        peer_map::{PeerMap, direct_peer_info},
        peer_session::PeerSessionStore,
    },
    context::{
        ArcPeerContext, CorePeerContext, CorePeerContextAdapters, NetworkIdentity, PeerContext,
        PeerPacketPolicy, PeerStunInfoSource,
    },
    credential_manager::{CredentialManager, CredentialStorage},
    error::{Error, take_path_unavailable_warn_token},
    foreign_network::client::ForeignNetworkClient,
    foreign_network::{ForeignNetworkEntryInfo, ForeignNetworkManager, ForeignNetworkRpcRegistrar},
    peer_center::instance::PeerCenterPeerManagerTrait,
    peer_rpc::{PeerRpcManager, PeerRpcManagerTransport},
    pending_packets::{PENDING_TTL, Parked, PendingPackets},
    public_ipv6::{CorePublicIpv6Runtime, PublicIpv6Runtime},
    recv_packet_envelope_from_chan,
    relay_peer_map::RelayPeerMap,
    route::{
        ArcRoute, DisabledRoute, ForeignNetworkRouteInfoMap, NextHopPolicy, Route, RouteInterface,
        local_route_table::{
            AllUnavailable, CostSource, CostedPeer, FailoverWhen, LocalRouteTable,
            MAX_PEERS_PER_RULE, PinMatch, PinnedPeer, PolicyRouteConfig, RouteLookup, RuleState,
            RuleUpdate, SelectBy, build_snapshot, is_degraded_cost, merged_candidate_count,
            merged_candidates, validate_and_build_policy_rules,
            validate_route_algo_can_price_candidates, within_switch_band,
        },
        peer_ospf_route::PeerRoute,
    },
    traffic_metrics::{
        InstanceLabelKind, LogicalTrafficMetrics, TrafficKind, TrafficMetricRecorder,
        route_peer_info_instance_id, traffic_kind,
    },
    util::shrink_dashmap,
};
use crate::foundation::stats::{CounterHandle, LabelSet, LabelType, MetricName, StatsManager};
use crate::peers::flow_table::{FlowTable, FlowTableConfig};
use crate::proto::peer_rpc::{
    ForeignNetworkRouteInfoEntry, ForeignNetworkRouteInfoKey, GetIpListResponse, PeerIdentityType,
    PeerInfoForGlobalMap, RouteForeignNetworkInfos, RouteForeignNetworkSummary,
};

#[derive(Debug, Clone)]
pub struct PeerSnapshot {
    pub peer_id: PeerId,
    pub default_conn_id: Option<PeerConnId>,
    pub directly_connected_conns: Vec<PeerConnId>,
    pub conns: Vec<PeerConnInfo>,
}

#[derive(Debug, Clone)]
pub struct NodeSnapshot {
    pub peer_id: PeerId,
    pub ipv4_addr: Option<cidr::Ipv4Inet>,
    pub ipv6_addr: Option<cidr::Ipv6Inet>,
    pub proxy_networks: Vec<ProxyNetworkConfig>,
    pub hostname: String,
    pub stun_info: StunInfo,
    pub instance_id: uuid::Uuid,
    pub listeners: Vec<Url>,
    pub version: String,
    pub feature_flags: PeerFeatureFlag,
    pub ip_list: GetIpListResponse,
    pub public_ipv6_addr: Option<cidr::Ipv6Inet>,
    pub ipv6_public_addr_prefix: Option<cidr::Ipv6Inet>,
}

struct RpcTransport {
    my_peer_id: PeerId,
    peers: Weak<PeerMap>,

    packet_recv: Mutex<UnboundedReceiver<ZCPacket>>,
    peer_rpc_tspt_sender: UnboundedSender<ZCPacket>,

    encryptor: Arc<dyn Encryptor>,
    is_secure_mode_enabled: bool,
}

impl RpcTransport {
    pub fn new(
        my_peer_id: PeerId,
        peers: Weak<PeerMap>,
        encryptor: Arc<dyn Encryptor>,
        is_secure_mode_enabled: bool,
    ) -> Arc<Self> {
        let (peer_rpc_tspt_sender, peer_rpc_tspt_recv) = mpsc::unbounded_channel();
        Arc::new(Self {
            my_peer_id,
            peers,
            packet_recv: Mutex::new(peer_rpc_tspt_recv),
            peer_rpc_tspt_sender,
            encryptor,
            is_secure_mode_enabled,
        })
    }

    pub fn packet_sender(&self) -> UnboundedSender<ZCPacket> {
        self.peer_rpc_tspt_sender.clone()
    }
}

#[async_trait::async_trait]
impl PeerRpcManagerTransport for RpcTransport {
    fn my_peer_id(&self) -> PeerId {
        self.my_peer_id
    }

    async fn send(&self, mut msg: ZCPacket, dst_peer_id: PeerId) -> anyhow::Result<()> {
        let peers = self
            .peers
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("peer map is gone"))?;
        // NOTE: if route info is not exchanged, this will return None. treat it as public server.
        let is_dst_peer_public_server = peers
            .get_route_peer_info(dst_peer_id)
            .await
            .and_then(|x| x.feature_flag.map(|x| x.is_public_server))
            // if dst is directly connected, it's must not public server
            .unwrap_or(!peers.has_peer(dst_peer_id));
        if !is_dst_peer_public_server && !self.is_secure_mode_enabled {
            self.encryptor
                .encrypt(&mut msg)
                .with_context(|| "encrypt failed")?;
        }
        // send to self and this packet will be forwarded in peer_recv loop
        peers.send_msg_directly(msg, self.my_peer_id).await?;
        Ok(())
    }

    async fn recv(&self) -> anyhow::Result<ZCPacket> {
        if let Some(o) = self.packet_recv.lock().await.recv().await {
            Ok(o)
        } else {
            Err(anyhow::anyhow!("rpc transport is closed"))
        }
    }
}

pub(crate) fn get_next_hop_policy(is_latency_first: bool) -> NextHopPolicy {
    if is_latency_first {
        NextHopPolicy::LeastCost
    } else {
        NextHopPolicy::LeastHop
    }
}

fn random_peer_id() -> PeerId {
    loop {
        let peer_id = rand::random();
        if peer_id != 0 {
            return peer_id;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteAlgoType {
    Ospf,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortablePeerManagerConfig {
    pub snapshot: PeerRuntimeSnapshot,
    pub route_algo: RouteAlgoType,
    pub exit_nodes: Vec<IpAddr>,
    /// Unified routing policy table (P1). Empty disables the feature entirely,
    /// keeping the L3 outbound path on its original code path (zero regression).
    #[serde(default)]
    pub policy_routes: Vec<PolicyRouteConfig>,
    /// `[dns_config]` at build time. Used only to pre-check `[[policy_routes]]`
    /// `domains` against MagicDNS guard rules (P3 / C3); not applied here.
    #[serde(default)]
    pub dns_config: Option<crate::config::DnsConfig>,
    /// Defaults inherited by peer contexts created for foreign networks.
    ///
    /// This is explicit because those contexts participate in the same
    /// handshake as the parent but do not inherit all parent policy flags.
    pub foreign_context_default_flags: FlagsInConfig,
    /// Per-tunnel IP flow accounting (disabled by default).
    ///
    /// See `docs/roadmap/tunnel-ip-flow-observability.md`.
    #[serde(default)]
    pub flow_stats: FlowTableConfig,
}

impl PortablePeerManagerConfig {
    pub fn new(mut runtime: PeerRuntimeConfig) -> Self {
        let policy = &runtime.core.peer_policy;
        let traffic = &runtime.core.traffic;
        let flags = FlagsInConfig {
            enable_encryption: policy.encryption_required,
            encryption_algorithm: crate::config::EncryptionAlgorithm::default().to_string(),
            disable_p2p: !policy.p2p_enabled,
            relay_all_peer_rpc: policy.relay_peer_rpc,
            disable_relay_data: !policy.relay_data,
            latency_first: policy.latency_first,
            data_compress_algo: crate::proto::common::CompressionAlgoPb::None.into(),
            mtu: traffic.mtu.map(u32::from).unwrap_or_default(),
            instance_recv_bps_limit: traffic.instance_recv_bps_limit.unwrap_or_default(),
            foreign_relay_bps_limit: traffic.foreign_relay_bps_limit.unwrap_or_default(),
            ..Default::default()
        };
        runtime.feature_flags.disable_p2p = flags.disable_p2p;
        runtime.feature_flags.avoid_relay_data |= flags.disable_relay_data;
        let foreign_context_default_flags = flags.clone();
        Self {
            snapshot: PeerRuntimeSnapshot::new(runtime, flags),
            route_algo: RouteAlgoType::Ospf,
            exit_nodes: Vec::new(),
            policy_routes: Vec::new(),
            dns_config: None,
            foreign_context_default_flags,
            flow_stats: FlowTableConfig::default(),
        }
    }
}

/// Build the unified routing table from config (P1).
///
/// Returns an **empty** table when no `[[policy_routes]]` are configured, so
/// the L3 outbound path keeps its original code path (zero regression) — the
/// legacy `exit_nodes` list keeps being handled by the old step 4.
///
/// Configuration errors (missing policy fields, unconfirmed `closed` + multi
/// peer, …) are hard failures, never silently defaulted.
#[allow(clippy::too_many_arguments)]
fn build_local_route_table(
    policy_routes: &[PolicyRouteConfig],
    exit_nodes: &[IpAddr],
    dns_config: Option<&crate::config::DnsConfig>,
    tld_dns_zone: &str,
    route_algo: RouteAlgoType,
) -> anyhow::Result<LocalRouteTable> {
    // Zero-regression: exit_nodes-only configs keep the empty table and step-4
    // live list (`update_exit_nodes`). Do **not** bake exit_nodes into sugar
    // rules here — that would freeze hot-patched exit peers on construct-time A.
    if policy_routes.is_empty() {
        return Ok(LocalRouteTable::empty());
    }
    // L5: refuse cost-based selection when there is no route table to price the
    // candidates with. Checked before building so the error names the rule the
    // user wrote, not an internal reconcile symptom.
    validate_route_algo_can_price_candidates(
        policy_routes,
        matches!(route_algo, RouteAlgoType::Ospf),
    )
    .map_err(|error| anyhow::anyhow!("invalid policy_routes: {error}"))?;
    let rules =
        validate_and_build_policy_rules(policy_routes, exit_nodes, dns_config, tld_dns_zone)
            .map_err(|error| anyhow::anyhow!("invalid policy_routes: {error}"))?;
    let table = LocalRouteTable::empty();
    if !rules.is_empty() {
        table.store(build_snapshot(rules));
    }
    Ok(table)
}

/// Whether a rule consults a candidate's **cost** when it ranks them.
///
/// `select_by = quality` ranks by cost, and `failover_when = degraded` drops
/// candidates whose cost exceeds the ceiling; every other combination decides
/// by reachability / list order alone.
///
/// This is the one place that answers the question. It is asked **per rule**, on
/// the reconcile loop's own hot path, and it decides whether that rule records
/// its candidate in `ResolvedRule::costed_peers`. The cost memo is shared across
/// rules, so a `priority` rule can see a number another rule measured; recording
/// it anyway would put a cost on the status page that the rule never ranked by
/// (R4).
///
/// It is deliberately *not* a whole-table pre-pass any more: a config that mixes
/// one `quality` rule with several `priority` ones only pays for the candidates
/// the `quality` rule actually reached.
fn rule_reads_cost(rule: &super::route::local_route_table::RouteRule) -> bool {
    // Both fields are already parsed on `RouteRule` (an unparseable string was
    // rejected at load time), so there is nothing to re-parse here.
    rule.select_by == SelectBy::Quality || rule.failover_when == FailoverWhen::Degraded
}

/// Pick the next `(active_peer, challenger)` for a non-`balance` rule.
///
/// Stages:
///   1. no incumbent            → adopt `best` immediately
///   2. incumbent gone/degraded → fail over to `best` immediately
///   3. incumbent fine but a better challenger exists:
///      - `select_by = priority` → adopt `best` immediately (list order is the
///        policy; otherwise a recovered preferred peer never wins back)
///      - `select_by = quality`  → only after `switch_windows` consecutive ticks
///        outside the cost switch band (margin + window hysteresis)
fn select_reconciled_peer(
    select_by: SelectBy,
    active_peer: Option<PeerId>,
    best: Option<PeerId>,
    cost_of: &dyn Fn(PeerId) -> Option<i32>,
    eligible: &dyn Fn(i32) -> bool,
    challenger: Option<(PeerId, u32)>,
    conn_select: ConnSelectConfig,
) -> (Option<PeerId>, Option<(PeerId, u32)>) {
    match (active_peer, best) {
        (_, None) => (None, None),
        (None, Some(best_peer)) => (Some(best_peer), None),
        (Some(current), Some(best_peer)) if current == best_peer => (Some(current), None),
        (Some(current), Some(best_peer)) => match cost_of(current) {
            Some(current_cost) if eligible(current_cost) => {
                // Priority (and balance, which never reaches here) must not use
                // the quality switch band: every reachable candidate scores 0,
                // so the band would hold forever and a recovered preferred peer
                // would never become active again.
                if select_by != SelectBy::Quality {
                    return (Some(best_peer), None);
                }
                let holds = cost_of(best_peer).is_some_and(|best_cost| {
                    within_switch_band(current_cost, best_cost, conn_select)
                });
                if holds {
                    (Some(current), None)
                } else {
                    let streak = match challenger {
                        Some((peer, ticks)) if peer == best_peer => ticks.saturating_add(1),
                        _ => 1,
                    };
                    if streak >= conn_select.switch_windows {
                        (Some(best_peer), None)
                    } else {
                        (Some(current), Some((best_peer, streak)))
                    }
                }
            }
            // Incumbent unreachable or over the ceiling: fail over now.
            _ => (Some(best_peer), None),
        },
    }
}

/// One reconcile pass: recompute `peer_quality` / `active_peer` / `state` for
/// every rule and publish the result. Runs off the packet hot path (~1 s); the
/// hot path only ever reads the already-selected `active_peer`.
///
/// `select_by = priority` reuses `PeerMap::resolve_exit_node_peers`
/// (list order + reachability); `select_by = quality` (P4) ranks the same
/// candidates by their precomputed composite cost. `failover_when = degraded`
/// additionally drops candidates whose cost exceeds `quality_degraded_cost`.
///
/// The selection awaits, so the pass must **not** publish a snapshot cloned
/// before those awaits: a concurrent `observe_ips` (P3) would be clobbered and
/// the freshly learned domain `/32` lost. Instead the intended updates are
/// applied onto the *latest* snapshot via `LocalRouteTable::apply_peer_updates`
/// (`ArcSwap::rcu`, retried on concurrent writes).
async fn reconcile_local_routes_once(
    peers: &PeerMap,
    table: &LocalRouteTable,
    conn_select: ConnSelectConfig,
    exit_nodes: &[IpAddr],
) {
    let snap = table.load_full();
    if snap.rules.is_empty() {
        return;
    }
    // Composite cost per candidate peer, memoised across rules for this tick.
    let mut quality: HashMap<PeerId, i32> = HashMap::new();
    // L2: which of the two cost families produced each `quality` entry. Both are
    // encoded in the same unit, but one is an accumulated **published** path
    // weight and the other a **local** direct-link measurement, so the previous
    // tick's number is only a valid hysteresis baseline when the source matches.
    let mut cost_source: HashMap<PeerId, CostSource> = HashMap::new();
    // Reachability of candidate VIPs, memoised across rules so shared peers are
    // only resolved once per tick.
    let mut reachable_by_ip: HashMap<IpAddr, Option<PeerId>> = HashMap::new();
    let mut updates: Vec<RuleUpdate> = Vec::new();

    for (idx, r) in snap.rules.iter().enumerate() {
        // B7: a default-route rule may merge in the **live** `exit_nodes`.
        //
        // The merge happens here — at reconcile time — and not when the rule
        // was built, so a hot-patched `exit_nodes` (`config_patch` →
        // `update_exit_nodes`) reaches the candidate list on the next tick.
        // Baking the exit nodes into `rule.peers` at construction time would
        // freeze them for the lifetime of the instance.
        let merged_storage: Option<Vec<IpAddr>> = if r.rule.merge_exit_nodes {
            Some(merged_candidates(&r.rule, exit_nodes))
        } else {
            None
        };
        let candidates: &[IpAddr] = match (r.rule.failover_when, &merged_storage) {
            (FailoverWhen::Never, Some(merged)) => merged.get(..1).unwrap_or(&[]),
            (FailoverWhen::Never, None) => r.rule.peers.get(..1).unwrap_or(&[]),
            (_, Some(merged)) => merged,
            (_, None) => &r.rule.peers,
        };
        let mut reachable: Vec<PeerId> = Vec::with_capacity(candidates.len());
        for ip in candidates {
            let peer_id = match reachable_by_ip.get(ip) {
                Some(cached) => *cached,
                None => {
                    let resolved = peers
                        .resolve_exit_node_peers(std::slice::from_ref(ip))
                        .await;
                    let peer_id = resolved.into_iter().next();
                    reachable_by_ip.insert(*ip, peer_id);
                    peer_id
                }
            };
            if let Some(peer_id) = peer_id
                // Two different addresses (a v4 and its v4-mapped v6, say)
                // can resolve to the **same** peer. Keeping it twice would give
                // that peer two slots in the `balance` distribution and make
                // the spread wrong.
                && !reachable.contains(&peer_id)
            {
                reachable.push(peer_id);
            }
        }

        let mut scored: Vec<(PeerId, i32)> = Vec::with_capacity(reachable.len());
        // L2/R4: the peers **this rule** priced, with the numbers and families
        // it saw, so the status page can show this rule's costs instead of
        // whatever the shared memo happens to hold for those peers (see
        // `ResolvedRule::costed_peers`).
        let mut costed_peers: Vec<CostedPeer> = Vec::new();
        for peer_id in &reachable {
            let peer_id = *peer_id;
            // Whether **this** rule reads a cost for this peer. Both branches
            // below share the memo, and a memo hit means some other rule priced
            // this peer earlier in the tick. What is remembered per rule must
            // still be "this rule consulted a cost", not "a cost exists in the
            // map" — otherwise a `priority` rule whose candidate is also a
            // `quality` rule's candidate would report and display a cost it
            // never ranked by, decided by rule order alone (R4).
            let uses_cost = rule_reads_cost(&r.rule);
            let cost = match quality.get(&peer_id) {
                // Measured earlier in this tick by another rule: real, so it
                // counts for this rule too. The number and its family are the
                // ones from this tick's memo, which is what this rule is
                // ranking against.
                Some(cost) if uses_cost => {
                    costed_peers.push(CostedPeer {
                        peer_id,
                        cost: *cost,
                        source: cost_source
                            .get(&peer_id)
                            .copied()
                            // A memo entry always comes with its source; the
                            // `Path` default only exists so this cannot panic.
                            .unwrap_or_default(),
                    });
                    *cost
                }
                None if uses_cost => {
                    // Price the candidate by the **whole path** to it, relay
                    // hops included. Scoring only the direct link made a
                    // candidate that is reachable solely through a relay look
                    // unreachable (cost `i32::MAX`), so it always ranked last
                    // and `failover_when = degraded` dropped it outright —
                    // even when it was the best exit we had.
                    //
                    // The direct-link cost stays as the fallback for the
                    // window before the route table knows the peer at all.
                    //
                    // It must not be computed unconditionally: `Option::or`
                    // evaluates its argument eagerly, so `.or(...)` would run
                    // the extra `list_peer_conns` even when the path cost is
                    // already known.
                    let (raw, source) = match peers.peer_path_cost(peer_id).await {
                        Some(cost) => (cost, CostSource::Path),
                        None => (
                            peers.peer_edge_cost(peer_id).await.unwrap_or(i32::MAX),
                            CostSource::Edge,
                        ),
                    };
                    // L2: only reuse the previous tick's number as the baseline
                    // while it came from the **same family**. A path cost and an
                    // edge cost are the same unit but not the same measurement:
                    // carrying one over as a baseline for the other would pin
                    // the published cost to a value nobody measured whenever the
                    // route table briefly forgets a peer and re-learns it, and
                    // the 20-unit delta would hide the switch.
                    let previous = snap
                        .peer_cost_source
                        .get(&peer_id)
                        .is_some_and(|prev| *prev == source)
                        .then(|| snap.peer_quality.get(&peer_id).copied())
                        .flatten();
                    if let (Some(prev_source), Some(prev_cost)) = (
                        snap.peer_cost_source.get(&peer_id),
                        snap.peer_quality.get(&peer_id),
                    ) && *prev_source != source
                    {
                        tracing::debug!(
                            peer_id,
                            ?prev_source,
                            ?source,
                            prev_cost,
                            raw,
                            "policy cost source switched; re-baselining hysteresis"
                        );
                    }
                    // Publish debounce: the same MIN_DELTA rule OSPF uses, but
                    // against our own baseline so OSPF's per-peer-map publisher
                    // state (`PeerMap::ospf_edge_cost_baseline`) is left untouched.
                    let cost = apply_ospf_cost_hysteresis(raw, previous, OSPF_EDGE_COST_MIN_DELTA);
                    quality.insert(peer_id, cost);
                    cost_source.insert(peer_id, source);
                    costed_peers.push(CostedPeer {
                        peer_id,
                        cost,
                        source,
                    });
                    cost
                }
                // Priority + unreachable/never: costs are never consulted.
                //
                // `0` is a **placeholder for this rule's own ranking only** — it
                // must never enter the shared `quality` memo. Another rule in
                // the same tick reads that memo, and paying for it once is the
                // whole point; seeding it with a `0` that nobody measured would
                // make a `quality` rule rank that peer first and put a
                // misleading `0` on the status page.
                //
                // This also covers "the memo already had a real number, but
                // *this* rule does not read costs": the number is reused as this
                // rule's ranking placeholder (all candidates tie, list order
                // decides) but is deliberately **not** recorded in
                // `costed_peers`, so it never reaches the status page (R4).
                _ => 0,
            };
            scored.push((peer_id, cost));
        }

        let eligible =
            |cost: i32| !is_degraded_cost(r.rule.failover_when, r.rule.quality_degraded_cost, cost);

        // L3: `never` pins the first candidate that ever resolved and keeps it
        // for the lifetime of the rule.
        //
        // "Never fail over" is a statement about *which* peer carries the
        // traffic, not about whether the rule exists: dropping the rule to
        // `Degraded` when that one peer goes away would hand the flow to the
        // physical gateway (or a peer `/0`) — the very egress the user ruled
        // out.
        //
        // The pin is anchored on the peer's **static public key**, not on the
        // `PeerId` alone. `PeerId` is a random `u32` chosen at the remote's
        // start-up and reported by it in the handshake, so it is neither stable
        // across a restart nor unique over time. Two ways an id-only pin breaks:
        //
        //   - the exit node restarts, comes back with a new id ⇒ an id-only pin
        //     black-holes the route forever for no reason;
        //   - an unrelated node is handed the old id ⇒ an id-only pin sends the
        //     traffic to the wrong node, which is worse than a black hole.
        //
        // Resolution order on every pass:
        //   1. the pinned id still resolves to the pinned key   → keep pinning
        //   2. the pinned id resolves to a *different* key      → drop the pin
        //      (the id was reused; never route to a stranger)
        //   3. the pinned key is now reachable under a new id   → re-pin to it
        //      (the same node after a restart)
        //   4. neither                                            → keep the old
        //      pin: the node is simply down, and `never` says to keep waiting
        //      for *it*, not to fall through to another egress
        let pinned_peer = if r.rule.failover_when == FailoverWhen::Never {
            resolve_pinned_peer(peers, r.pinned_peer.as_ref(), &reachable).await
        } else {
            None
        };

        // P5: `balance` spreads flows across *every* eligible candidate, so it
        // publishes the whole list and lets the hot path pick by flow hash
        // instead of reconciling a single `active_peer`.
        if r.rule.select_by == SelectBy::Balance {
            let resolved: Vec<PeerId> = scored
                .iter()
                .filter(|(_, cost)| eligible(*cost))
                .map(|(peer, _)| *peer)
                .collect();
            let state = if resolved.is_empty() {
                RuleState::Degraded
            } else {
                RuleState::Stable
            };
            // `active_peer` is informational only (first candidate); the hot
            // path `lookup` hashes into `resolved_peers` and ignores
            // `active_peer` (L4). Always write the pair together so status /
            // dumps never disagree with the spread pool.
            let active_peer = resolved.first().copied();
            if r.active_peer != active_peer
                || r.state != state
                || r.challenger.is_some()
                || r.resolved_peers != resolved
                || r.pinned_peer != pinned_peer
                || r.costed_peers != costed_peers
            {
                updates.push(RuleUpdate {
                    idx,
                    active_peer,
                    state,
                    challenger: None,
                    resolved_peers: resolved,
                    pinned_peer,
                    costed_peers,
                });
            }
            continue;
        }

        let cost_of = |peer_id: PeerId| {
            scored
                .iter()
                .find(|(candidate, _)| *candidate == peer_id)
                .map(|(_, cost)| *cost)
        };
        // The candidate we actually *want*: list order for `priority`, lowest
        // cost for `quality`. `None` when every reachable candidate is degraded.
        let best = match r.rule.select_by {
            SelectBy::Priority => scored
                .iter()
                .find(|(_, cost)| eligible(*cost))
                .map(|(peer, _)| *peer),
            SelectBy::Quality => scored
                .iter()
                .filter(|(_, cost)| eligible(*cost))
                .min_by_key(|(_, cost)| *cost)
                .map(|(peer, _)| *peer),
            // `balance` returned above.
            SelectBy::Balance => None,
        };
        let (chosen, challenger) = select_reconciled_peer(
            r.rule.select_by,
            r.active_peer,
            best,
            &cost_of,
            &eligible,
            r.challenger,
            conn_select,
        );
        // `Stable` = the peer we want is the one in use; `Failover` = we are on
        // an alternative (backup, or held by the switch band).
        //
        // L3: `Degraded` stays "no reachable candidate". The hot path does not
        // depend on the state flip for a `never` rule — it reads
        // `ResolvedRule::pinned_peer` — so the rule keeps routing to its pin
        // while the reported state still says the target is down.
        let state = match chosen {
            None => RuleState::Degraded,
            Some(current) if Some(current) == best => RuleState::Stable,
            Some(_) => RuleState::Failover,
        };
        if r.active_peer != chosen
            || r.state != state
            || r.challenger != challenger
            || r.pinned_peer != pinned_peer
            || r.costed_peers != costed_peers
        {
            updates.push(RuleUpdate {
                idx,
                active_peer: chosen,
                state,
                challenger,
                resolved_peers: Vec::new(),
                pinned_peer,
                costed_peers,
            });
        }
    }

    table.apply_peer_updates(&quality, &cost_source, &updates);
}

/// L3: work out what a `failover_when = never` rule should be pinned to now.
///
/// See the call site for why the pin is keyed on the static public key rather
/// than the `PeerId`. `reachable` is this rule's resolved candidate list for the
/// current tick (already in config order); an empty list means the pinned node
/// is currently down.
async fn resolve_pinned_peer(
    peers: &PeerMap,
    current: Option<&PinnedPeer>,
    reachable: &[PeerId],
) -> Option<PinnedPeer> {
    // Snapshot the two facts the decision needs, so the rule itself is a pure
    // function that can be tested without building a PeerMap.
    //
    // Must use live-conn, not mere map presence: when the last PeerConn drops,
    // `Peer` clears its public key while the map entry survives ~3s
    // (`clean_peer_without_conn`). `has_peer` would then feed
    // `pinned_id_live = Some(None)` into the pure decision, which a keyed pin
    // treats as "stranger took the id" and **drops** the pin — contradicting
    // "node merely offline ⇒ keep waiting". `has_peer_with_live_conn` maps that
    // zombie window to `None` so case 4 keeps the pin.
    let pinned_id_live = current
        .filter(|pin| peers.has_peer_with_live_conn(pin.peer_id))
        .map(|pin| peers.peer_public_key(pin.peer_id));
    let keys: Vec<(PeerId, Option<Vec<u8>>)> = reachable
        .iter()
        .map(|peer_id| (*peer_id, peers.peer_public_key(*peer_id)))
        .collect();
    decide_pinned_peer(current, pinned_id_live, &keys)
}

/// Start the pin over from the first reachable candidate.
///
/// Used **only** after [`PinMatch::DifferentNode`] — a stranger with a
/// different static key took the id. Mode-mismatch / unverifiable cases keep
/// the existing pin (see the 2b / 2c arms in [`decide_pinned_peer`]). With a
/// reachable candidate the rule re-pins onto it; without one it stays
/// **unpinned** and falls back to `if_all_unavailable` (`open` → physical
/// gateway, `closed` → drop).
fn restart_pin_from_first_candidate(reachable: &[(PeerId, Option<Vec<u8>>)]) -> Option<PinnedPeer> {
    reachable
        .first()
        .map(|(peer_id, key)| PinnedPeer::new(*peer_id, key.clone()))
}

/// The pure pin decision; see [`resolve_pinned_peer`] for the input contract.
///
/// * `pinned_id_live` — `Some(key)` when the pinned **id** still has a **live
///   conn** (key may itself be `None` for keyless / plaintext conns), `None`
///   when the id is gone *or* the map entry is a zombie with no live conn.
/// * `reachable` — this tick's candidates with their keys, in config order.
///
/// Only [`PinMatch::DifferentNode`] drops the pin. `OccupantUnverifiable` /
/// `PinUnverifiable` are mode / configuration facts (encryption toggled, or
/// the pin was always keyless) — they keep waiting on the same id. Folding
/// them into the stranger path would reset plaintext pins whenever a peer
/// later presents a key, and would treat "peer turned encryption off" as
/// "a stranger took the id".
fn decide_pinned_peer(
    current: Option<&PinnedPeer>,
    pinned_id_live: Option<Option<Vec<u8>>>,
    reachable: &[(PeerId, Option<Vec<u8>>)],
) -> Option<PinnedPeer> {
    let Some(current) = current else {
        // First resolution: pin whatever candidate came first.
        let (peer_id, key) = reachable.first()?;
        return Some(PinnedPeer::new(*peer_id, key.clone()));
    };

    // 1 / 2: what does the pinned id point at *now*?
    if let Some(key_now) = pinned_id_live {
        // The pinned id still has a **live** connection, so its key is
        // meaningful and the identity can be judged.
        return match current.classify(current.peer_id, key_now.as_deref()) {
            PinMatch::Same => {
                // 1: same node, same id.
                Some(current.clone())
            }
            PinMatch::DifferentNode => {
                // 2a: both sides carry a key and they differ — a **different
                // node** holds the id. Never route to a stranger: drop the pin.
                tracing::warn!(
                    pinned_peer_id = current.peer_id,
                    "failover_when = never: the pinned peer id now belongs to a different node \
                     (its static key changed); dropping the pin"
                );
                restart_pin_from_first_candidate(reachable)
            }
            PinMatch::OccupantUnverifiable => {
                // 2b: the pin carries a key, the occupant does not. Nothing
                // proves a stranger (that would be 2a with another key) — more
                // often the far side turned encryption off. Keep the pin; a
                // true stranger in encrypted mode presents a *different* key.
                tracing::warn!(
                    pinned_peer_id = current.peer_id,
                    "failover_when = never: the pinned id is live but has no static key \
                     (plaintext / encryption toggled off); keeping the pin — only a different \
                     static key would prove a stranger"
                );
                Some(current.clone())
            }
            PinMatch::PinUnverifiable => {
                // 2c: the pin itself has no key (plaintext mode). There was
                // never a verifiable identity; the occupant presenting one is
                // a mode change, not proof the id was stolen. Keep waiting —
                // restart follow-along still happens in case 3 when the id
                // leaves the map.
                tracing::warn!(
                    pinned_peer_id = current.peer_id,
                    "failover_when = never: keyless pin (plaintext) against an occupant that \
                     now has a static key; keeping the pin — plaintext never had a verifiable \
                     identity to revoke"
                );
                Some(current.clone())
            }
        };
    }

    // 3: the same node may be back under a new id after a restart.
    if current.public_key.is_some() {
        // Keyed pin: the same key under another id is the same node.
        for (peer_id, key) in reachable {
            if current.matches(*peer_id, key.as_deref()) {
                return Some(PinnedPeer::new(*peer_id, current.public_key.clone()));
            }
        }
    } else {
        // Keyless pin (plaintext / no-noise). `matches` would only ever compare
        // ids here, so it cannot find the node under a *new* id — every
        // plaintext candidate it sees has no key, and "no key equals no key"
        // says nothing about identity. Relying on that equality is exactly the
        // trap this branch exists to avoid: once empty keys are normalised to
        // `None`, case 3 stops matching on its own and a restarted exit node
        // black-holes forever.
        //
        // So the intent is stated outright: with no verifiable identity, a
        // pinned node that vanished while another candidate is available is
        // treated as "the same node came back under a new id" and the pin
        // follows the candidate. This deliberately gives up a promise the
        // plaintext mode cannot keep anyway — it has no way to tell the
        // restarted node from a stranger holding its old id — in exchange for
        // restart survival, which is an everyday operation. See
        // `PinnedPeer::matches` case 2.
        if let Some((peer_id, key)) = reachable.first() {
            tracing::info!(
                pinned_peer_id = current.peer_id,
                new_peer_id = *peer_id,
                "failover_when = never: no verifiable peer identity (plaintext mode); \
                 following the first reachable candidate"
            );
            return Some(PinnedPeer::new(*peer_id, key.clone()));
        }
    }

    // 4: the pinned node is down (or unreachable). Keep waiting for it — that is
    // what "never fail over" asks for.
    Some(current.clone())
}

fn matching_group_memberships(
    declarations: &[PeerGroupIdentity],
    configured_groups: &[String],
) -> Vec<PeerGroupIdentity> {
    let configured = configured_groups.iter().collect::<BTreeSet<_>>();
    declarations
        .iter()
        .filter(|declaration| configured.contains(&declaration.group_name))
        .cloned()
        .collect()
}

fn first_missing_group<'a>(
    memberships: &[PeerGroupIdentity],
    configured_groups: &'a [String],
) -> Option<&'a str> {
    let resolved = memberships
        .iter()
        .map(|membership| membership.group_name.as_str())
        .collect::<BTreeSet<_>>();
    configured_groups
        .iter()
        .map(String::as_str)
        .find(|group| !resolved.contains(group))
}

fn retain_runtime_owned_peer_state(
    current: &CoreInstanceRuntimeConfig,
    next: &mut CoreInstanceRuntimeConfig,
    peer_id: PeerId,
) {
    let next_peer = Arc::make_mut(&mut next.peer);
    next_peer.runtime.core.node.peer_id = Some(peer_id);
    next_peer.runtime.core.node.instance_id = current.peer.runtime.core.node.instance_id;
    next_peer.runtime.stun_info = current.peer.runtime.stun_info.clone();
    if current.services.dhcp_ipv4 && next.services.dhcp_ipv4 {
        next_peer.runtime.core.routes.ipv4 = current.peer.runtime.core.routes.ipv4.clone();
    }
}

fn validate_portable_routes(routes: &crate::config::RouteConfig) -> anyhow::Result<()> {
    if !routes.advertised_routes.is_empty() {
        anyhow::bail!("portable peer manager does not support advertised routes yet");
    }
    if !routes.foreign_networks.is_empty() {
        anyhow::bail!("portable peer manager does not support foreign networks yet");
    }
    if let Some(prefix) = &routes.ipv4
        && (!matches!(prefix.address, IpAddr::V4(_)) || prefix.prefix_len > 32)
    {
        anyhow::bail!("routes.ipv4 must contain a valid IPv4 prefix");
    }
    if let Some(prefix) = &routes.ipv6
        && (!matches!(prefix.address, IpAddr::V6(_)) || prefix.prefix_len > 128)
    {
        anyhow::bail!("routes.ipv6 must contain a valid IPv6 prefix");
    }
    for proxy in &routes.proxy_networks {
        for (field, prefix) in [
            ("real", Some(&proxy.real)),
            ("mapped", proxy.mapped.as_ref()),
        ] {
            let Some(prefix) = prefix else {
                continue;
            };
            let IpAddr::V4(address) = prefix.address else {
                anyhow::bail!("proxy network {field} prefix must be IPv4");
            };
            if cidr::Ipv4Cidr::new(address, prefix.prefix_len).is_err() {
                anyhow::bail!("proxy network {field} must be a valid IPv4 network prefix");
            }
        }
    }
    Ok(())
}

pub(crate) enum RouteAlgoInst {
    Ospf(Arc<PeerRoute>),
    None,
}

impl Clone for RouteAlgoInst {
    fn clone(&self) -> Self {
        match self {
            RouteAlgoInst::Ospf(route) => RouteAlgoInst::Ospf(route.clone()),
            RouteAlgoInst::None => RouteAlgoInst::None,
        }
    }
}

impl RouteAlgoInst {
    pub fn new(
        route_algo: RouteAlgoType,
        my_peer_id: PeerId,
        context: ArcPeerContext,
        public_ipv6_runtime: Arc<dyn PublicIpv6Runtime>,
        peer_rpc_mgr: Arc<super::peer_rpc::PeerRpcManager>,
    ) -> Self {
        match route_algo {
            RouteAlgoType::Ospf => RouteAlgoInst::Ospf(PeerRoute::new(
                my_peer_id,
                context,
                public_ipv6_runtime,
                peer_rpc_mgr,
            )),
            RouteAlgoType::None => RouteAlgoInst::None,
        }
    }

    pub fn ospf_route(&self) -> Option<Arc<PeerRoute>> {
        match self {
            RouteAlgoInst::Ospf(route) => Some(route.clone()),
            RouteAlgoInst::None => None,
        }
    }

    pub fn route_arc(&self) -> ArcRoute {
        match self {
            RouteAlgoInst::Ospf(route) => route.clone(),
            RouteAlgoInst::None => Arc::new(DisabledRoute),
        }
    }
}

fn network_secret_digest_is_empty(network: &NetworkIdentity) -> bool {
    network
        .network_secret_digest
        .as_ref()
        .is_none_or(|d| d.iter().all(|b| *b == 0))
}

pub(crate) async fn add_new_peer_conn(
    peer_map: &PeerMap,
    local_identity: &NetworkIdentity,
    local_secure_mode: bool,
    peer_conn: PeerConn,
) -> Result<PeerId, Error> {
    let peer_identity = peer_conn.get_network_identity();
    let conn_info = peer_conn.get_conn_info();
    let peer_secure_mode = !conn_info.noise_remote_static_pubkey.is_empty();

    if local_secure_mode != peer_secure_mode {
        return Err(Error::SecretKeyError(
            "same-network peers must use the same secure mode".to_string(),
        ));
    }

    // For credential nodes, network_secret_digest is either None or all-zeros
    // (all-zeros when received over the wire via handshake).
    // In this case, only compare network_name.
    let my_digest_empty = network_secret_digest_is_empty(local_identity);
    let peer_digest_empty = network_secret_digest_is_empty(&peer_identity);

    let identity_ok = if my_digest_empty || peer_digest_empty {
        // Credential node: only check network_name
        local_identity.network_name == peer_identity.network_name
    } else {
        local_identity == &peer_identity
    };

    if !identity_ok {
        return Err(Error::SecretKeyError(
            "network identity not match".to_string(),
        ));
    }
    let peer_id = peer_conn.get_peer_id();
    peer_map.add_new_peer_conn(peer_conn).await?;
    Ok(peer_id)
}

pub(crate) async fn close_untrusted_credential_peers<F>(
    peer_map: &PeerMap,
    network_name: &str,
    mut is_pubkey_trusted: F,
) where
    F: FnMut(&[u8], &str) -> bool + Send,
{
    for peer_id in peer_map.list_peers() {
        if !matches!(
            peer_map.get_peer_identity_type(peer_id),
            Some(PeerIdentityType::Credential)
        ) {
            continue;
        }
        let Some(peer) = peer_map.get_peer_by_id(peer_id) else {
            continue;
        };
        let Some(pubkey) = peer.get_peer_public_key() else {
            continue;
        };

        if is_pubkey_trusted(&pubkey, network_name) {
            continue;
        }

        tracing::warn!(?peer_id, "closing untrusted credential peer");
        if let Err(e) = peer_map.close_peer(peer_id).await {
            tracing::warn!(?e, ?peer_id, "failed to close untrusted credential peer");
        }
    }
}

struct NicPacketProcessor {
    nic_channel: HostPacketSender,
    nic_channel_drops: CounterHandle,
}

#[async_trait::async_trait]
impl PeerPacketFilter for NicPacketProcessor {
    async fn try_process_packet_from_peer(&self, packet: ZCPacket) -> Option<ZCPacket> {
        let hdr = packet.peer_manager_header().unwrap();
        if hdr.packet_type == PacketType::Data as u8 && !hdr.is_not_send_to_tun() {
            if hdr.is_encrypted() || hdr.is_compressed() {
                tracing::warn!(
                    from_peer_id = hdr.from_peer_id.get(),
                    to_peer_id = hdr.to_peer_id.get(),
                    encrypted = hdr.is_encrypted(),
                    compressed = hdr.is_compressed(),
                    "dropping packet before nic because it is not fully decoded"
                );
                return None;
            }
            tracing::trace!(?packet, "send packet to nic channel");
            if let Err(error) = self
                .nic_channel
                .try_send(HostPacket::from_core_packet(packet))
            {
                self.nic_channel_drops.inc();
                // `debug!` not `warn!`: this fires per dropped packet, so a
                // warn would flood the log. The `data_plane_nic_channel_drops`
                // counter is the observable signal (P-AUDIT.3).
                tracing::debug!(
                    ?error,
                    "dropping packet because nic channel cannot accept it"
                );
            }
            None
        } else {
            Some(packet)
        }
    }
}

struct PeerRpcPacketProcessor {
    peer_rpc_tspt_sender: UnboundedSender<ZCPacket>,
}

#[async_trait::async_trait]
impl PeerPacketFilter for PeerRpcPacketProcessor {
    async fn try_process_packet_from_peer(&self, packet: ZCPacket) -> Option<ZCPacket> {
        let hdr = packet.peer_manager_header().unwrap();
        if hdr.packet_type == PacketType::TaRpc as u8
            || hdr.packet_type == PacketType::RpcReq as u8
            || hdr.packet_type == PacketType::RpcResp as u8
        {
            self.peer_rpc_tspt_sender.send(packet).unwrap();
            None
        } else {
            Some(packet)
        }
    }
}

#[derive(Clone)]
pub(crate) struct PeerPipelineEntry {
    active: Option<Arc<AtomicBool>>,
    filter: Arc<dyn PeerPacketFilter + Send + Sync>,
}

#[derive(Clone)]
pub(crate) struct NicPipelineEntry {
    active: Option<Arc<AtomicBool>>,
    filter: Arc<dyn super::NicPacketFilter + Send + Sync>,
}

type PeerPacketPipeline = Arc<ArcSwap<Vec<PeerPipelineEntry>>>;
type NicPacketPipeline = Arc<ArcSwap<Vec<NicPipelineEntry>>>;

impl PeerPipelineEntry {
    fn filter_if_active(&self) -> Option<&Arc<dyn PeerPacketFilter + Send + Sync>> {
        if self
            .active
            .as_ref()
            .is_some_and(|active| !active.load(Ordering::Acquire))
        {
            return None;
        }
        Some(&self.filter)
    }
}

impl NicPipelineEntry {
    fn filter_if_active(&self) -> Option<&Arc<dyn super::NicPacketFilter + Send + Sync>> {
        if self
            .active
            .as_ref()
            .is_some_and(|active| !active.load(Ordering::Acquire))
        {
            return None;
        }
        Some(&self.filter)
    }
}

#[derive(Clone)]
pub(crate) struct PipelineRegistrationGuard {
    active: Arc<AtomicBool>,
    unregister: Arc<dyn Fn() + Send + Sync>,
}

impl PipelineRegistrationGuard {
    pub fn close(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            (self.unregister)();
        }
    }
}

impl Drop for PipelineRegistrationGuard {
    fn drop(&mut self) {
        self.close();
    }
}

fn permanent_peer_pipeline_entry(filter: BoxPeerPacketFilter) -> PeerPipelineEntry {
    PeerPipelineEntry {
        active: None,
        filter: Arc::from(filter),
    }
}

fn permanent_nic_pipeline_entry(filter: BoxNicPacketFilter) -> NicPipelineEntry {
    NicPipelineEntry {
        active: None,
        filter: Arc::from(filter),
    }
}

fn managed_peer_pipeline_entry(
    filter: BoxPeerPacketFilter,
    pipeline: &PeerPacketPipeline,
) -> (PeerPipelineEntry, PipelineRegistrationGuard) {
    let active = Arc::new(AtomicBool::new(true));
    let weak_pipeline = Arc::downgrade(pipeline);
    let registration = active.clone();
    (
        PeerPipelineEntry {
            active: Some(active.clone()),
            filter: Arc::from(filter),
        },
        PipelineRegistrationGuard {
            active,
            unregister: Arc::new(move || {
                let Some(pipeline) = weak_pipeline.upgrade() else {
                    return;
                };
                pipeline.rcu(|current| {
                    Arc::new(
                        current
                            .iter()
                            .filter(|entry| {
                                entry
                                    .active
                                    .as_ref()
                                    .is_none_or(|active| !Arc::ptr_eq(active, &registration))
                            })
                            .cloned()
                            .collect(),
                    )
                });
            }),
        },
    )
}

#[cfg(any(feature = "proxy-packet", test))]
fn managed_nic_pipeline_entry(
    filter: BoxNicPacketFilter,
    pipeline: &NicPacketPipeline,
) -> (NicPipelineEntry, PipelineRegistrationGuard) {
    let active = Arc::new(AtomicBool::new(true));
    let weak_pipeline = Arc::downgrade(pipeline);
    let registration = active.clone();
    (
        NicPipelineEntry {
            active: Some(active.clone()),
            filter: Arc::from(filter),
        },
        PipelineRegistrationGuard {
            active,
            unregister: Arc::new(move || {
                let Some(pipeline) = weak_pipeline.upgrade() else {
                    return;
                };
                pipeline.rcu(|current| {
                    Arc::new(
                        current
                            .iter()
                            .filter(|entry| {
                                entry
                                    .active
                                    .as_ref()
                                    .is_none_or(|active| !Arc::ptr_eq(active, &registration))
                            })
                            .cloned()
                            .collect(),
                    )
                });
            }),
        },
    )
}

fn append_peer_pipeline(pipeline: &PeerPacketPipeline, entry: PeerPipelineEntry) {
    pipeline.rcu(|current| {
        let mut next = Vec::with_capacity(current.len() + 1);
        next.extend(
            current
                .iter()
                .filter(|entry| {
                    entry
                        .active
                        .as_ref()
                        .is_none_or(|active| active.load(Ordering::Acquire))
                })
                .cloned(),
        );
        next.push(entry.clone());
        Arc::new(next)
    });
}

fn append_nic_pipeline(pipeline: &NicPacketPipeline, entry: NicPipelineEntry) {
    pipeline.rcu(|current| {
        let mut next = Vec::with_capacity(current.len() + 1);
        next.extend(
            current
                .iter()
                .filter(|entry| {
                    entry
                        .active
                        .as_ref()
                        .is_none_or(|active| active.load(Ordering::Acquire))
                })
                .cloned(),
        );
        next.push(entry.clone());
        Arc::new(next)
    });
}

async fn init_packet_process_pipeline(
    peer_packet_process_pipeline: &PeerPacketPipeline,
    nic_channel: HostPacketSender,
    peer_rpc_tspt_sender: UnboundedSender<ZCPacket>,
    nic_channel_drops: CounterHandle,
) {
    // for tun/tap ip/eth packet.
    append_peer_pipeline(
        peer_packet_process_pipeline,
        permanent_peer_pipeline_entry(Box::new(NicPacketProcessor {
            nic_channel,
            nic_channel_drops,
        })),
    );

    // for peer rpc packet
    append_peer_pipeline(
        peer_packet_process_pipeline,
        permanent_peer_pipeline_entry(Box::new(PeerRpcPacketProcessor {
            peer_rpc_tspt_sender,
        })),
    );
}

async fn add_route<T>(
    peer_packet_process_pipeline: &PeerPacketPipeline,
    peers: Arc<PeerMap>,
    foreign_network_client: Arc<ForeignNetworkClient>,
    foreign_network_manager: Arc<ForeignNetworkManager>,
    my_peer_id: PeerId,
    route: Arc<T>,
) where
    T: Route + PeerPacketFilter + Send + Sync + 'static,
{
    // for route
    append_peer_pipeline(
        peer_packet_process_pipeline,
        permanent_peer_pipeline_entry(Box::new(route.clone())),
    );

    let _route_id = route
        .open(Box::new(PeerManagerRouteInterface {
            my_peer_id,
            peers: Arc::downgrade(&peers),
            foreign_network_client: Arc::downgrade(&foreign_network_client),
            foreign_network_manager: Arc::downgrade(&foreign_network_manager),
        }))
        .await
        .unwrap();

    let arc_route: ArcRoute = route;
    peers.add_route(arc_route).await;
}

pub(crate) struct PeerManagerTrafficCounters {
    pub self_tx_packets: CounterHandle,
    pub self_tx_bytes: CounterHandle,
    pub compress_tx_bytes_before: CounterHandle,
    pub compress_tx_bytes_after: CounterHandle,
    /// P-AUDIT.2: send_msg_by_ip found no next hop / empty dst fanout.
    pub no_dst_peer_drops: CounterHandle,
    /// P-AUDIT.3: inbound NIC channel full.
    pub nic_channel_drops: CounterHandle,
    /// P-AUDIT.3: outbound tunnel ring BufferFull.
    pub tunnel_buffer_full: CounterHandle,
}

fn note_tunnel_buffer_full(counter: &CounterHandle, err: &Error) {
    if matches!(err, Error::Tunnel(crate::tunnel::TunnelError::BufferFull)) {
        counter.inc();
    }
}

/// Whether a packet must carry `not_send_to_tun` + `no_proxy`.
///
/// Both flags exist purely to stop a **self-delivered** packet from looping:
/// with them set, the local `NicPacketProcessor` skips the TUN and the ICMP
/// proxy declines to answer, so a packet that the proxy re-injected for its own
/// node is not bounced back into the host stack.
///
/// They must therefore be gated on `to_peer_id == my_peer_id`. Applying them to
/// a packet bound for *another* node suppresses that node's two reply sources,
/// which silently turns every per-link VIP probe into "unreachable" — exactly the
/// bug this helper was extracted to prevent. It is shared by the user path and
/// the prober paths so the three sites cannot drift apart again.
fn needs_self_delivery_guard(
    local_exit_node_fallback: bool,
    not_send_to_self: bool,
    to_peer_id: PeerId,
    my_peer_id: PeerId,
    is_local_virtual_ip: bool,
) -> bool {
    !local_exit_node_fallback
        && not_send_to_self
        && to_peer_id == my_peer_id
        && !is_local_virtual_ip
}

pub struct PeerManagerCore {
    my_peer_id: PeerId,
    tasks: Mutex<JoinSet<()>>,
    packet_recv: Arc<Mutex<Option<PacketRecvChanReceiver>>>,
    peers: Arc<PeerMap>,
    peer_rpc_mgr: Arc<super::peer_rpc::PeerRpcManager>,
    peer_rpc_tspt: Arc<RpcTransport>,
    peer_packet_process_pipeline: PeerPacketPipeline,
    nic_packet_process_pipeline: NicPacketPipeline,
    nic_channel: HostPacketSender,
    route_algo_inst: RouteAlgoInst,
    foreign_network_client: Arc<ForeignNetworkClient>,
    foreign_network_manager: Arc<ForeignNetworkManager>,
    relay_peer_map: Arc<RelayPeerMap>,
    peer_connection_admission: PeerConnectionAdmission,
    outbound_packet_router: PeerOutboundPacketRouter,
    recent_traffic: RecentTrafficTracker,
    peer_session_store: Arc<PeerSessionStore>,
    encryptor: Arc<dyn Encryptor + 'static>,
    data_compress_algo: CompressorAlgo,
    exit_nodes: Arc<RwLock<Vec<IpAddr>>>,
    local_route_table: Arc<LocalRouteTable>,
    acl_filter: Arc<AclFilter>,
    context: Arc<CorePeerContext>,
    runtime_config: CoreRuntimeConfigStore,
    runtime_config_update: Mutex<()>,
    is_secure_mode_enabled: bool,
    route: ArcRoute,
    traffic_metrics: Arc<TrafficMetricRecorder>,
    /// Per-tunnel IP flow accounting (disabled by default).
    flow_table: Arc<FlowTable>,
    network_name: String,
    counters: PeerManagerTrafficCounters,
}

fn check_resolved_remote_addr_not_from_virtual_network(
    context: &ArcPeerContext,
    resolved_remote_addr: Option<ProtoUrl>,
) -> Result<(), Error> {
    let Some(remote_addr) = resolved_remote_addr.map(Url::from) else {
        return Ok(());
    };
    let Some(ip) = remote_addr.host().and_then(|host| match host {
        url::Host::Ipv4(ip) => Some(IpAddr::V4(ip)),
        url::Host::Ipv6(ip) => Some(IpAddr::V6(ip)),
        url::Host::Domain(host) => host.parse().ok(),
    }) else {
        return Ok(());
    };

    // If no-tun is enabled, the src ip of packet in virtual network is converted to loopback.
    // TCP/QUIC/KCP proxy listeners already filter those connections.
    if ip.is_loopback() || !context.is_ip_in_same_network(&ip) {
        return Ok(());
    }

    Err(anyhow::anyhow!(
        "tunnel src {} is from the same network (ignore this error please)",
        ip
    )
    .into())
}

impl PeerManagerCore {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        mut config: PortablePeerManagerConfig,
        managed_credentials: Vec<ManagedCredentialConfig>,
        runtime_config: CoreRuntimeConfigStore,
        stun_info_source: Arc<dyn PeerStunInfoSource>,
        nic_channel: HostPacketSender,
        public_ipv6_runtime: Arc<CorePublicIpv6Runtime>,
        events: Arc<dyn CoreEventSink>,
        credential_storage: Option<Arc<dyn CredentialStorage>>,
        foreign_rpc_registrar: Arc<dyn ForeignNetworkRpcRegistrar>,
    ) -> anyhow::Result<Self> {
        let initial_acl = runtime_config.snapshot().services.acl.build()?;
        let runtime = &mut config.snapshot.runtime;
        let flags = &config.snapshot.flags;
        let network_name = runtime.network_identity.network_name.clone();
        if network_name.is_empty() {
            anyhow::bail!("network identity name cannot be empty");
        }
        match runtime.core.node.network_name.as_str() {
            "" => runtime.core.node.network_name = network_name.clone(),
            configured if configured != network_name => anyhow::bail!(
                "core node network name {configured:?} does not match identity {network_name:?}"
            ),
            _ => {}
        }
        validate_portable_routes(&runtime.core.routes)?;

        if let (Some(_), Some(expected_digest)) = (
            runtime.network_identity.network_secret.as_ref(),
            runtime.network_identity.network_secret_digest.as_ref(),
        ) {
            let mut identity = runtime.network_identity.clone();
            identity.network_secret_digest = None;
            let derived_digest = identity
                .secret_digest()
                .expect("identity with a secret should derive a digest");
            if &derived_digest != expected_digest {
                anyhow::bail!("network secret does not match the configured digest");
            }
        }
        if runtime.network_identity.network_secret.is_none()
            && runtime
                .network_identity
                .network_secret_digest
                .as_ref()
                .is_some_and(|digest| digest.iter().any(|byte| *byte != 0))
        {
            anyhow::bail!("digest-only local identity requires credential key capabilities");
        }
        let is_secure_mode_enabled = runtime
            .secure_mode
            .as_ref()
            .is_some_and(|secure| secure.enabled);
        let is_credential_peer = runtime.network_identity.network_secret.is_none();
        if is_credential_peer && !is_secure_mode_enabled {
            anyhow::bail!("credential peer identity requires secure mode and a local keypair");
        }
        runtime.feature_flags.is_credential_peer = is_credential_peer;
        if let Some(secure) = runtime.secure_mode.as_ref().filter(|secure| secure.enabled) {
            let private_key = secure.private_key()?;
            let public_key = secure.public_key()?;
            let derived_public = x25519_dalek::PublicKey::from(&private_key);
            if derived_public.as_bytes() != public_key.as_bytes() {
                anyhow::bail!("secure mode public key does not match its private key");
            }
        }
        let data_compress_algo = CompressorAlgo::try_from(flags.data_compress_algo())?;
        data_compress_algo.ensure_available()?;
        if tokio::runtime::Handle::try_current().is_err() {
            anyhow::bail!("portable peer manager construction requires an entered Tokio runtime");
        }

        // Peer IDs identify one live process incarnation. They are never
        // configuration: every new PeerManager gets a fresh runtime identity.
        let my_peer_id = random_peer_id();
        runtime.core.node.peer_id = Some(my_peer_id);
        let instance_id = runtime
            .core
            .node
            .instance_id
            .map(uuid::Uuid::from_bytes)
            .unwrap_or_else(uuid::Uuid::new_v4);
        runtime.core.node.instance_id = Some(*instance_id.as_bytes());

        let secret = runtime
            .network_identity
            .network_secret
            .as_deref()
            .unwrap_or_default();
        let encryptor: Arc<dyn Encryptor> = if flags.enable_encryption {
            validate_algorithm(&flags.encryption_algorithm)?;
            create_encryptor(
                &flags.encryption_algorithm,
                derive_key_128(secret),
                derive_key_256(secret),
            )
        } else {
            Arc::new(NullCipher)
        };
        runtime.feature_flags.disable_p2p = flags.disable_p2p;
        runtime.feature_flags.need_p2p = flags.need_p2p;
        runtime.feature_flags.avoid_relay_data |= flags.disable_relay_data;
        runtime_config.update_peer(Arc::new(config.snapshot.clone()));
        let public_ipv6_state = public_ipv6_runtime.clone();
        let public_ipv6_runtime: Arc<dyn PublicIpv6Runtime> = public_ipv6_runtime;
        let context = Arc::new(CorePeerContext::new(
            runtime_config,
            public_ipv6_state,
            CorePeerContextAdapters {
                stun_info_source: Some(stun_info_source),
                events,
                credential_storage,
            },
        ));
        context
            .credential_manager()
            .install_initial_managed_credentials(&managed_credentials)
            .map_err(anyhow::Error::msg)?;
        let local_route_table = Arc::new(build_local_route_table(
            &config.policy_routes,
            &config.exit_nodes,
            config.dns_config.as_ref(),
            &config.snapshot.flags.tld_dns_zone,
            config.route_algo,
        )?);
        let peer_manager = Self::assemble(
            config.route_algo,
            my_peer_id,
            context,
            public_ipv6_runtime,
            nic_channel,
            encryptor,
            is_secure_mode_enabled,
            data_compress_algo,
            config.exit_nodes,
            local_route_table,
            config.foreign_context_default_flags,
            foreign_rpc_registrar,
            config.flow_stats,
        );
        peer_manager.reload_acl(initial_acl.as_ref());
        Ok(peer_manager)
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        route_algo: RouteAlgoType,
        my_peer_id: PeerId,
        core_context: Arc<CorePeerContext>,
        public_ipv6_runtime: Arc<dyn PublicIpv6Runtime>,
        nic_channel: HostPacketSender,
        encryptor: Arc<dyn Encryptor + 'static>,
        is_secure_mode_enabled: bool,
        data_compress_algo: CompressorAlgo,
        exit_nodes: Vec<IpAddr>,
        local_route_table: Arc<LocalRouteTable>,
        foreign_context_default_flags: FlagsInConfig,
        foreign_rpc_registrar: Arc<dyn ForeignNetworkRpcRegistrar>,
        flow_stats: FlowTableConfig,
    ) -> Self {
        let stats_manager = core_context.stats_manager();
        let acl_filter = Arc::new(AclFilter::new());
        let context: ArcPeerContext = core_context.clone();
        let (packet_send, packet_recv) = super::create_packet_recv_chan();
        // Per-tunnel IP flow accounting. Disabled by default: the table costs
        // one atomic load per packet while disabled and allocates nothing.
        let flow_table = Arc::new(FlowTable::new(flow_stats));
        let peers = Arc::new(PeerMap::new(
            packet_send.clone(),
            context.clone(),
            my_peer_id,
            flow_table.clone(),
        ));
        let peer_session_store = Arc::new(PeerSessionStore::new());

        let rpc_tspt = RpcTransport::new(
            my_peer_id,
            Arc::downgrade(&peers),
            encryptor.clone(),
            is_secure_mode_enabled,
        );
        let peer_rpc_mgr = Arc::new(super::peer_rpc::PeerRpcManager::new_with_stats_manager(
            rpc_tspt.clone(),
            stats_manager.clone(),
        ));

        let route_algo_inst = RouteAlgoInst::new(
            route_algo,
            my_peer_id,
            context.clone(),
            public_ipv6_runtime,
            peer_rpc_mgr.clone(),
        );

        let foreign_network_manager = Arc::new(ForeignNetworkManager::new(
            foreign_rpc_registrar,
            core_context.clone(),
            foreign_context_default_flags,
            peer_session_store.clone(),
            packet_send.clone(),
            Arc::downgrade(&peers),
        ));
        let foreign_network_client = Arc::new(ForeignNetworkClient::new(
            context.clone(),
            packet_send,
            my_peer_id,
        ));

        let network_name = context.network_name();
        let traffic_tx_metrics = Arc::new(LogicalTrafficMetrics::new(
            stats_manager.clone(),
            network_name.clone(),
            MetricName::TrafficBytesTx,
            MetricName::TrafficPacketsTx,
            MetricName::TrafficBytesTxByInstance,
            MetricName::TrafficPacketsTxByInstance,
            InstanceLabelKind::To,
        ));
        let traffic_control_tx_metrics = Arc::new(LogicalTrafficMetrics::new(
            stats_manager.clone(),
            network_name.clone(),
            MetricName::TrafficControlBytesTx,
            MetricName::TrafficControlPacketsTx,
            MetricName::TrafficControlBytesTxByInstance,
            MetricName::TrafficControlPacketsTxByInstance,
            InstanceLabelKind::To,
        ));
        let net_label =
            LabelSet::new().with_label_type(LabelType::NetworkName(network_name.clone()));
        // P-AUDIT.12: the router owns the pending queue, so it owns this
        // counter too — see `PeerOutboundPacketRouterCounters`.
        let no_dst_peer_buffered =
            stats_manager.get_counter(MetricName::DataPlaneNoDstPeerBuffered, net_label.clone());
        let pending_flush_errors =
            stats_manager.get_counter(MetricName::DataPlanePendingFlushErrors, net_label.clone());
        let self_tx_counters = PeerManagerTrafficCounters {
            self_tx_packets: stats_manager
                .get_counter(MetricName::TrafficPacketsSelfTx, net_label.clone()),
            self_tx_bytes: stats_manager
                .get_counter(MetricName::TrafficBytesSelfTx, net_label.clone()),
            compress_tx_bytes_before: stats_manager
                .get_counter(MetricName::CompressionBytesTxBefore, net_label.clone()),
            compress_tx_bytes_after: stats_manager
                .get_counter(MetricName::CompressionBytesTxAfter, net_label.clone()),
            no_dst_peer_drops: stats_manager
                .get_counter(MetricName::DataPlaneNoDstPeerDrops, net_label.clone()),
            nic_channel_drops: stats_manager
                .get_counter(MetricName::DataPlaneNicChannelDrops, net_label.clone()),
            tunnel_buffer_full: stats_manager
                .get_counter(MetricName::DataPlaneTunnelBufferFull, net_label),
        };
        let traffic_rx_metrics = Arc::new(LogicalTrafficMetrics::new(
            stats_manager.clone(),
            network_name.clone(),
            MetricName::TrafficBytesRx,
            MetricName::TrafficPacketsRx,
            MetricName::TrafficBytesRxByInstance,
            MetricName::TrafficPacketsRxByInstance,
            InstanceLabelKind::From,
        ));
        let traffic_control_rx_metrics = Arc::new(LogicalTrafficMetrics::new(
            stats_manager.clone(),
            network_name.clone(),
            MetricName::TrafficControlBytesRx,
            MetricName::TrafficControlPacketsRx,
            MetricName::TrafficControlBytesRxByInstance,
            MetricName::TrafficControlPacketsRxByInstance,
            InstanceLabelKind::From,
        ));
        let route_algo_inst_for_metrics = route_algo_inst.clone();
        let traffic_metrics = Arc::new(TrafficMetricRecorder::new(
            my_peer_id,
            traffic_tx_metrics,
            traffic_control_tx_metrics,
            traffic_rx_metrics,
            traffic_control_rx_metrics,
            move |peer_id| {
                let route_algo_inst = route_algo_inst_for_metrics.clone();
                async move {
                    match route_algo_inst.ospf_route() {
                        Some(route) => route
                            .get_peer_info(peer_id)
                            .await
                            .as_ref()
                            .and_then(route_peer_info_instance_id),
                        None => None,
                    }
                }
            },
        ));
        let peer_packet_process_pipeline = Arc::new(ArcSwap::from_pointee(Vec::new()));
        let nic_packet_process_pipeline = Arc::new(ArcSwap::from_pointee(Vec::new()));
        let exit_nodes = Arc::new(RwLock::new(exit_nodes));
        let relay_peer_map = super::relay_peer_map::new_relay_peer_map(
            peers.clone(),
            Some(foreign_network_client.clone()),
            context.clone(),
            my_peer_id,
            peer_session_store.clone(),
        );
        let recent_traffic = RecentTrafficTracker::new(my_peer_id);
        let peer_connection_admission = PeerConnectionAdmission::new(
            my_peer_id,
            context.clone(),
            peers.clone(),
            foreign_network_client.clone(),
            foreign_network_manager.clone(),
            peer_session_store.clone(),
            recent_traffic.clone(),
        );
        let route = route_algo_inst.route_arc();
        let outbound_packet_router = PeerOutboundPacketRouter::new(
            my_peer_id,
            context.clone(),
            peers.clone(),
            route.clone(),
            foreign_network_client.clone(),
            relay_peer_map.clone(),
            nic_packet_process_pipeline.clone(),
            encryptor.clone(),
            data_compress_algo,
            exit_nodes.clone(),
            local_route_table.clone(),
            recent_traffic.clone(),
            traffic_metrics.clone(),
            acl_filter.clone(),
            is_secure_mode_enabled,
            self_tx_counters.self_tx_packets.clone(),
            self_tx_counters.self_tx_bytes.clone(),
            self_tx_counters.compress_tx_bytes_before.clone(),
            self_tx_counters.compress_tx_bytes_after.clone(),
            self_tx_counters.no_dst_peer_drops.clone(),
            no_dst_peer_buffered,
            self_tx_counters.tunnel_buffer_full.clone(),
            pending_flush_errors,
            flow_table.clone(),
        );

        Self {
            my_peer_id,
            tasks: Mutex::new(JoinSet::new()),
            packet_recv: Arc::new(Mutex::new(Some(packet_recv))),
            peers,
            peer_rpc_mgr,
            peer_rpc_tspt: rpc_tspt,
            peer_packet_process_pipeline,
            nic_packet_process_pipeline,
            nic_channel,
            route_algo_inst,
            foreign_network_client,
            foreign_network_manager,
            relay_peer_map,
            peer_connection_admission,
            outbound_packet_router,
            recent_traffic,
            peer_session_store,
            encryptor,
            data_compress_algo,
            exit_nodes,
            local_route_table,
            acl_filter,
            runtime_config: core_context.runtime_config_store(),
            runtime_config_update: Mutex::new(()),
            context: core_context,
            is_secure_mode_enabled,
            route,
            traffic_metrics,
            flow_table,
            network_name,
            counters: self_tx_counters,
        }
    }

    pub fn my_peer_id(&self) -> PeerId {
        self.my_peer_id
    }

    /// Per-tunnel IP flow accounting table (disabled by default).
    pub fn flow_table(&self) -> &Arc<FlowTable> {
        &self.flow_table
    }

    pub(crate) fn credential_manager(&self) -> Arc<CredentialManager> {
        self.context.credential_manager()
    }

    pub(crate) fn register_ephemeral_credential(
        &self,
        public_key: [u8; 32],
        groups: Vec<String>,
    ) -> anyhow::Result<uuid::Uuid> {
        let credential_id = self
            .credential_manager()
            .register_ephemeral_credential(public_key, groups)
            .map_err(anyhow::Error::msg)?;
        self.notify_credential_changed();
        Ok(credential_id)
    }

    pub(crate) async fn update_ephemeral_credential_groups(
        &self,
        credential_id: uuid::Uuid,
        groups: Vec<String>,
    ) -> Option<bool> {
        let changed = self
            .credential_manager()
            .update_ephemeral_credential_groups(credential_id, groups)?;
        if changed {
            self.notify_credential_changed();
            self.route.refresh_acl_groups().await;
        }
        Some(changed)
    }

    pub(crate) fn revoke_ephemeral_credential(&self, credential_id: uuid::Uuid) -> bool {
        let revoked = self
            .credential_manager()
            .revoke_ephemeral_credential(credential_id);
        if revoked {
            self.notify_credential_changed();
        }
        revoked
    }

    pub(crate) async fn revoke_ephemeral_credential_and_refresh(
        &self,
        credential_id: uuid::Uuid,
    ) -> bool {
        let revoked = self.revoke_ephemeral_credential(credential_id);
        if revoked {
            self.route.refresh_acl_groups().await;
        }
        revoked
    }

    pub fn stats_manager(&self) -> Arc<StatsManager> {
        self.context.stats_manager()
    }

    pub fn can_manage_credentials(&self) -> bool {
        self.context.network_identity().network_secret.is_some()
    }

    pub(crate) fn set_avoid_relay_data_preference(&self, avoid_relay_data: bool) {
        self.context
            .set_avoid_relay_data_preference(avoid_relay_data);
    }

    pub fn notify_credential_changed(&self) {
        self.context.issue_credential_changed();
    }

    pub async fn list_peer_snapshots(&self) -> Vec<PeerSnapshot> {
        let foreign_peer_map = self.foreign_network_client.get_peer_map();
        let mut peers = self.peers.list_peers();
        peers.extend(foreign_peer_map.list_peers());

        let mut snapshots = Vec::with_capacity(peers.len());
        for peer_id in peers {
            let conns = if let Some(conns) = self.peers.list_peer_conns(peer_id).await {
                conns
            } else {
                foreign_peer_map
                    .list_peer_conns(peer_id)
                    .await
                    .unwrap_or_default()
            };
            snapshots.push(PeerSnapshot {
                peer_id,
                default_conn_id: self.peers.get_peer_default_conn_id(peer_id).await,
                directly_connected_conns: self
                    .peers
                    .get_directly_connections_by_peer_id(peer_id)
                    .into_iter()
                    .collect(),
                conns,
            });
        }
        snapshots
    }

    pub(crate) fn instance_id(&self) -> uuid::Uuid {
        self.context.instance_id()
    }

    pub(crate) async fn node_snapshot(&self, listeners: Vec<Url>) -> NodeSnapshot {
        NodeSnapshot {
            peer_id: self.my_peer_id,
            ipv4_addr: self.context.ipv4(),
            ipv6_addr: self.context.ipv6(),
            proxy_networks: self.context.proxy_networks(),
            hostname: self.context.hostname(),
            stun_info: self.context.stun_info(),
            instance_id: self.context.instance_id(),
            listeners,
            version: self.context.easytier_version(),
            feature_flags: self.context.feature_flags(),
            ip_list: GetIpListResponse::default(),
            public_ipv6_addr: self.get_route().get_my_public_ipv6_addr().await,
            ipv6_public_addr_prefix: self.context.advertised_ipv6_public_addr_prefix().map(
                |prefix| {
                    cidr::Ipv6Inet::new(prefix.first_address(), prefix.network_length()).unwrap()
                },
            ),
        }
    }

    pub async fn list_route_snapshots(&self) -> Vec<CoreRoute> {
        self.get_route().list_routes().await
    }

    pub async fn list_public_ipv6_routes(&self) -> BTreeSet<cidr::Ipv6Inet> {
        self.get_route().list_public_ipv6_routes().await
    }

    pub async fn public_ipv6_addr(&self) -> Option<cidr::Ipv6Inet> {
        self.get_route().get_my_public_ipv6_addr().await
    }

    pub async fn dump_route(&self) -> String {
        self.get_route().dump().await
    }

    pub async fn local_public_ipv6_info(&self) -> ListPublicIpv6InfoResponse {
        self.get_route().get_local_public_ipv6_info().await
    }

    pub async fn foreign_network_route_infos(&self) -> RouteForeignNetworkInfos {
        self.get_route().list_foreign_network_info().await
    }

    pub async fn list_foreign_network_infos(
        &self,
        include_trusted_keys: bool,
    ) -> std::collections::HashMap<String, ForeignNetworkEntryInfo> {
        self.foreign_network_manager
            .list_foreign_network_infos(include_trusted_keys)
            .await
    }

    pub async fn foreign_network_route_summary(&self) -> RouteForeignNetworkSummary {
        self.get_route().get_foreign_network_summary().await
    }

    pub fn acl_stats(&self) -> crate::proto::acl::AclStats {
        self.acl_filter.get_stats()
    }

    pub fn acl_filter(&self) -> Arc<AclFilter> {
        self.acl_filter.clone()
    }

    pub(crate) async fn update_runtime_config(
        &self,
        config: CoreInstanceRuntimeConfig,
    ) -> anyhow::Result<Arc<CoreInstanceRuntimeConfig>> {
        let _update = self.runtime_config_update.lock().await;
        let current = self.runtime_config.snapshot();
        let refresh_acl_groups = current.peer.peer_group_memberships
            != config.peer.peer_group_memberships
            || current.peer.acl_group_declarations != config.peer.acl_group_declarations;
        let reload_acl = current.services.acl != config.services.acl;
        let next_acl = reload_acl.then(|| config.services.acl.clone());
        let next_built_acl = next_acl.as_ref().map(AclRuleConfig::build).transpose()?;

        self.set_avoid_relay_data_preference(config.peer.avoid_relay_data_preference);
        let published = self
            .runtime_config
            .replace_with_current(config, |current, next| {
                retain_runtime_owned_peer_state(current, next, self.my_peer_id);
            });
        if let Some(acl) = next_built_acl.as_ref() {
            self.reload_acl(acl.as_ref());
        }
        if refresh_acl_groups {
            self.route.refresh_acl_groups().await;
        }
        Ok(published)
    }

    /// Keeps this manager's ACL rules and group assignments aligned with a
    /// network policy source. The subscription is owned by this manager and is
    /// stopped with its other runtime tasks.
    pub(crate) async fn follow_network_policy(
        self: &Arc<Self>,
        source: CoreRuntimeConfigStore,
        configured_groups: Vec<String>,
    ) -> anyhow::Result<()> {
        let mut peer_changes = source.subscribe_peer_runtime_changes();
        let mut service_changes = source.subscribe_service_runtime_changes();
        let configured_groups: Arc<[String]> = configured_groups.into();
        self.apply_network_policy(&source, &configured_groups)
            .await?;

        let peer_manager = Arc::downgrade(self);
        self.tasks.lock().await.spawn(async move {
            loop {
                let changed = tokio::select! {
                    changed = peer_changes.changed() => changed,
                    changed = service_changes.changed() => changed,
                };
                if changed.is_err() {
                    return;
                }
                let _ = peer_changes.borrow_and_update();
                let _ = service_changes.borrow_and_update();
                let Some(peer_manager) = peer_manager.upgrade() else {
                    return;
                };
                if let Err(error) = peer_manager
                    .apply_network_policy(&source, &configured_groups)
                    .await
                {
                    tracing::warn!(
                        ?error,
                        peer_id = peer_manager.my_peer_id,
                        "failed to apply peer manager network policy"
                    );
                }
            }
        });
        Ok(())
    }

    async fn apply_network_policy(
        &self,
        source: &CoreRuntimeConfigStore,
        configured_groups: &[String],
    ) -> anyhow::Result<()> {
        let source = source.snapshot();
        let credential_peer = self.context.feature_flags().is_credential_peer;
        let acl = if credential_peer {
            source.services.acl.for_credential_peer()
        } else {
            source.services.acl.clone()
        };
        let (declarations, memberships, missing_group) = if credential_peer {
            (Vec::new(), Vec::new(), None)
        } else {
            let declarations = source.peer.acl_group_declarations.clone();
            let memberships = matching_group_memberships(&declarations, configured_groups);
            let missing_group = first_missing_group(&memberships, configured_groups);
            (declarations, memberships, missing_group)
        };
        let current = self.runtime_config.snapshot();
        if current.services.acl == acl
            && current.peer.acl_group_declarations == declarations
            && current.peer.peer_group_memberships == memberships
        {
            return Ok(());
        }

        let mut next = current.as_ref().clone();
        next.services.acl = acl;
        let peer = Arc::make_mut(&mut next.peer);
        peer.acl_group_declarations = declarations;
        peer.peer_group_memberships = memberships;
        self.update_runtime_config(next).await?;
        if let Some(group) = missing_group {
            tracing::warn!(
                peer_id = self.my_peer_id,
                group,
                "configured peer ACL group is no longer declared"
            );
        }
        Ok(())
    }

    pub fn network_name(&self) -> &str {
        &self.network_name
    }

    pub(crate) fn dns_route_identity(
        &self,
    ) -> (String, Option<crate::proto::common::Ipv4Inet>, String) {
        (
            self.context.hostname(),
            self.context.ipv4().map(Into::into),
            self.context.flags().tld_dns_zone,
        )
    }

    pub fn p2p_policy_flags(&self) -> P2pPolicyFlags {
        let flags = self.context.flags();
        P2pPolicyFlags {
            disable_udp_hole_punching: flags.disable_udp_hole_punching,
            disable_sym_hole_punching: flags.disable_sym_hole_punching,
            disable_upnp: flags.disable_upnp,
            // Quality-first: do NOT fold `prefer_peer_relay` into lazy_p2p.
            // That flag is a legacy TOML-only OSPF topology preference; delaying
            // background punch would fight select_conn / multi-path analysis.
            lazy_p2p: flags.lazy_p2p,
            disable_p2p: flags.disable_p2p,
            need_p2p: flags.need_p2p,
        }
    }

    pub fn tcp_hole_punching_disabled(&self) -> bool {
        self.context.flags().disable_tcp_hole_punching
    }

    pub fn get_peer_map(&self) -> Arc<PeerMap> {
        self.peers.clone()
    }

    pub fn get_relay_peer_map(&self) -> Arc<RelayPeerMap> {
        self.relay_peer_map.clone()
    }

    pub fn get_peer_rpc_mgr(&self) -> Arc<super::peer_rpc::PeerRpcManager> {
        self.peer_rpc_mgr.clone()
    }

    pub fn get_peer_session_store(&self) -> Arc<PeerSessionStore> {
        self.peer_session_store.clone()
    }
    /// Packet inject path for NIC/proxy callers; not every feature set uses it.
    #[cfg(feature = "proxy-packet")]
    #[allow(dead_code)]
    pub(crate) fn get_nic_channel(&self) -> HostPacketSender {
        self.nic_channel.clone()
    }

    pub(crate) fn is_local_virtual_ip(&self, ip: &IpAddr) -> bool {
        self.context.is_ip_local_virtual_ip(ip)
    }

    pub fn get_foreign_network_client(&self) -> Arc<ForeignNetworkClient> {
        self.foreign_network_client.clone()
    }

    pub async fn is_easytier_managed_ipv6(&self, ip: &std::net::Ipv6Addr) -> bool {
        if self.context.is_ip_local_ipv6(ip) {
            return true;
        }
        self.route
            .list_public_ipv6_routes()
            .await
            .iter()
            .any(|route| route.address() == *ip)
    }

    pub fn traffic_metrics(&self) -> Arc<TrafficMetricRecorder> {
        self.traffic_metrics.clone()
    }

    /// Whether this instance prices candidates through a route table.
    ///
    /// `route_algo` is a construction-time choice that never changes for the
    /// life of an instance, so this is a plain read. Callers that validate a
    /// candidate `[[policy_routes]]` table (hot patch, GUI save) must pass this
    /// into `validate_config_loader_policy_routes_for_algo` instead of assuming
    /// a route table exists: a `quality` / `degraded` rule cannot be priced
    /// without one, and silently accepting it would make the saved config do
    /// something other than what it says (L5).
    pub fn has_route_table(&self) -> bool {
        self.route_algo_inst.ospf_route().is_some()
    }

    pub fn get_route(&self) -> ArcRoute {
        self.route.clone()
    }

    /// Wake handle for the L2 proxy-CIDR reconciler (`ProxyCidrMonitor`).
    ///
    /// `None` when this instance uses `route_algo = none` (no OSPF service to
    /// observe). Callers must then fall back to plain interval polling — never
    /// treat `None` as "nothing to install".
    pub fn proxy_cidr_wake(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.route_algo_inst
            .ospf_route()
            .map(|route| route.peer_info_wake())
    }

    /// Wake handle for the pending-packet flusher (independent of
    /// [`Self::proxy_cidr_wake`] so `notify_one` permits are not stolen).
    pub fn pending_packet_wake(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.route_algo_inst
            .ospf_route()
            .map(|route| route.pending_packet_wake())
    }

    pub(crate) async fn withdraw_routes_before_stop(&self) -> bool {
        let Some(route) = self.route_algo_inst.ospf_route() else {
            return true;
        };
        route.withdraw_self_conn_info().await
    }

    pub fn mark_recent_traffic(&self, dst_peer_id: PeerId) {
        let flags = self.context.flags();
        self.recent_traffic
            .mark(dst_peer_id, flags.disable_p2p, flags.lazy_p2p, |peer_id| {
                self.has_directly_connected_conn(peer_id)
            });
    }

    pub fn has_recent_traffic(&self, peer_id: PeerId, now: Instant) -> bool {
        self.recent_traffic.has(peer_id, now, |peer_id| {
            self.has_directly_connected_conn(peer_id)
        })
    }

    pub fn clear_recent_traffic(&self, peer_id: PeerId) {
        self.recent_traffic.clear(peer_id);
    }

    pub fn p2p_demand_notify(&self) -> Arc<ExternalTaskSignal> {
        self.recent_traffic.p2p_demand_notify()
    }

    pub fn gc_recent_traffic(&self) {
        self.recent_traffic.gc(Instant::now(), |peer_id| {
            self.has_directly_connected_conn(peer_id)
        });
    }

    pub fn has_directly_connected_conn(&self, peer_id: PeerId) -> bool {
        if let Some(peer) = self.peers.get_peer_by_id(peer_id) {
            peer.has_directly_connected_conn()
        } else {
            self.foreign_network_client.get_peer_map().has_peer(peer_id)
        }
    }

    /// Whether this peer still needs more tunnels for bonding
    /// (`peer_link_bond_count` target not yet reached).
    ///
    /// Keys off the **bond send-set size**, not the live conn count: out-of-band
    /// live paths are hot standby and must not count as fill progress. A bounded
    /// standby ceiling keeps dial attempts from spinning when every dialed path
    /// stays out-of-band. `false` unless bonding is enabled — callers keep
    /// today's behavior then.
    pub fn peer_needs_bond_fill(&self, peer_id: PeerId) -> bool {
        let bond = BondConfig::from_flags(&self.context.flags());
        if !bond.enabled() {
            return false;
        }
        let Some(peer) = self.peers.get_peer_by_id(peer_id) else {
            return false;
        };
        let live = peer.live_conn_count();
        // Cheap exits before touching the bond cache: below target → must fill;
        // at the standby ceiling → stop (everything dialed so far is out-of-band).
        if bond_fill_needed(&bond, live) {
            return true;
        }
        if live >= bond_fill_live_ceiling(&bond) {
            return false;
        }
        // Live conns reach the target, but some may be out-of-band standby and
        // thus not in the send set — keep filling until the set is full.
        bond_fill_needed_with_standby(&bond, live, peer.bond_set_size())
    }

    pub async fn add_client_tunnel(
        &self,
        tunnel: Box<dyn Tunnel>,
        is_directly_connected: bool,
    ) -> Result<(PeerId, PeerConnId), Error> {
        self.peer_connection_admission
            .add_client_tunnel(tunnel, is_directly_connected)
            .await
    }

    pub async fn add_client_tunnel_with_peer_id_hint(
        &self,
        tunnel: Box<dyn Tunnel>,
        is_directly_connected: bool,
        peer_id_hint: Option<PeerId>,
    ) -> Result<(PeerId, PeerConnId), Error> {
        self.peer_connection_admission
            .add_client_tunnel_with_peer_id_hint(tunnel, is_directly_connected, peer_id_hint)
            .await
    }

    pub(crate) async fn add_attached_ring_client_tunnel(
        &self,
        tunnel: Box<dyn Tunnel>,
    ) -> Result<(PeerId, PeerConnId), Error> {
        self.peer_connection_admission
            .add_client_tunnel_with_origin(tunnel, true, None, PeerConnectionOrigin::Attached)
            .await
    }

    pub async fn add_tunnel_as_server(
        &self,
        tunnel: Box<dyn Tunnel>,
        is_directly_connected: bool,
    ) -> Result<(), Error> {
        self.peer_connection_admission
            .add_tunnel_as_server(tunnel, is_directly_connected)
            .await
    }

    pub(crate) async fn add_attached_ring_tunnel_as_server(
        &self,
        tunnel: Box<dyn Tunnel>,
    ) -> Result<(PeerId, PeerConnId), Error> {
        self.peer_connection_admission
            .add_tunnel_as_server_with_origin(tunnel, true, PeerConnectionOrigin::Attached)
            .await
    }

    pub async fn add_packet_process_pipeline(&self, pipeline: BoxPeerPacketFilter) {
        // newest pipeline will be executed first
        append_peer_pipeline(
            &self.peer_packet_process_pipeline,
            permanent_peer_pipeline_entry(pipeline),
        );
    }

    pub async fn add_nic_packet_process_pipeline(&self, pipeline: BoxNicPacketFilter) {
        // newest pipeline will be executed first
        append_nic_pipeline(
            &self.nic_packet_process_pipeline,
            permanent_nic_pipeline_entry(pipeline),
        );
    }

    pub(crate) async fn add_managed_packet_process_pipeline(
        &self,
        pipeline: BoxPeerPacketFilter,
    ) -> PipelineRegistrationGuard {
        let (entry, guard) =
            managed_peer_pipeline_entry(pipeline, &self.peer_packet_process_pipeline);
        append_peer_pipeline(&self.peer_packet_process_pipeline, entry);
        guard
    }

    #[cfg(feature = "proxy-packet")]
    pub(crate) async fn add_managed_nic_packet_process_pipeline(
        &self,
        pipeline: BoxNicPacketFilter,
    ) -> PipelineRegistrationGuard {
        let (entry, guard) =
            managed_nic_pipeline_entry(pipeline, &self.nic_packet_process_pipeline);
        append_nic_pipeline(&self.nic_packet_process_pipeline, entry);
        guard
    }

    #[cfg(feature = "proxy-packet")]
    pub(crate) async fn remove_managed_nic_packet_process_pipeline(
        &self,
        registration: &PipelineRegistrationGuard,
    ) {
        registration.close();
    }

    pub async fn add_route<T>(&self, route: Arc<T>)
    where
        T: Route + PeerPacketFilter + Send + Sync + 'static,
    {
        add_route(
            &self.peer_packet_process_pipeline,
            self.peers.clone(),
            self.foreign_network_client.clone(),
            self.foreign_network_manager.clone(),
            self.my_peer_id,
            route,
        )
        .await;
    }

    pub async fn remove_nic_packet_process_pipeline(&self, id: String) -> Result<(), Error> {
        let snapshot = self.nic_packet_process_pipeline.load_full();
        let Some(target) = snapshot
            .iter()
            .find(|entry| {
                entry
                    .filter_if_active()
                    .is_some_and(|filter| filter.id() == id)
            })
            .map(|entry| entry.filter.clone())
        else {
            return Err(Error::NotFound);
        };
        self.nic_packet_process_pipeline.rcu(|current| {
            Arc::new(
                current
                    .iter()
                    .filter(|entry| !Arc::ptr_eq(&entry.filter, &target))
                    .cloned()
                    .collect(),
            )
        });
        Ok(())
    }

    pub async fn send_msg_for_proxy(
        &self,
        msg: ZCPacket,
        dst_peer_id: PeerId,
    ) -> Result<(), Error> {
        self.outbound_packet_router
            .send_msg_for_proxy(msg, dst_peer_id)
            .await
    }

    pub async fn get_msg_dst_peer(&self, addr: &IpAddr) -> (Vec<PeerId>, bool) {
        let (peers, is_exit_node) = self
            .outbound_packet_router
            .get_msg_dst_peer(addr, None)
            .await;
        (peers.into_vec(), is_exit_node)
    }

    pub async fn get_msg_dst_peer_ipv4(&self, ipv4_addr: &Ipv4Addr) -> (Vec<PeerId>, bool) {
        self.get_msg_dst_peer_ipv4_with_flow(ipv4_addr, None).await
    }

    /// Same as [`Self::get_msg_dst_peer_ipv4`], but forwards the inner-flow hash
    /// so `select_by = balance` rules can spread flows (P5). Callers without a
    /// payload pass `None`, which degrades every flow to the first candidate.
    pub async fn get_msg_dst_peer_ipv4_with_flow(
        &self,
        ipv4_addr: &Ipv4Addr,
        flow_key: Option<u64>,
    ) -> (Vec<PeerId>, bool) {
        let (peers, is_exit_node) = self
            .outbound_packet_router
            .get_msg_dst_peer_ipv4(ipv4_addr, flow_key)
            .await;
        (peers.into_vec(), is_exit_node)
    }

    pub async fn get_msg_dst_peer_ipv6(&self, ipv6_addr: &Ipv6Addr) -> (Vec<PeerId>, bool) {
        let (peers, is_exit_node) = self
            .outbound_packet_router
            .get_msg_dst_peer_ipv6(ipv6_addr)
            .await;
        (peers.into_vec(), is_exit_node)
    }

    pub async fn send_msg_by_ip(
        &self,
        msg: ZCPacket,
        ip_addr: IpAddr,
        not_send_to_self: bool,
    ) -> Result<(), Error> {
        self.outbound_packet_router
            .send_msg_by_ip(msg, ip_addr, not_send_to_self)
            .await
    }

    /// Display-only send used by the VIP reachability prober. See
    /// [`PeerOutboundPacketRouter::send_probe_msg_by_ip`].
    pub async fn send_probe_msg_by_ip(
        &self,
        msg: ZCPacket,
        ip_addr: IpAddr,
        not_send_to_self: bool,
    ) -> Result<(), Error> {
        self.outbound_packet_router
            .send_probe_msg_by_ip(msg, ip_addr, not_send_to_self)
            .await
    }

    /// Per-link variant: send a probe **on one named PeerConn** instead of
    /// letting OSPF/`select_conn` pick a path.
    ///
    /// `dst_ip` is still used for the loop-prevention header flags, so a probe
    /// aimed at a local VIP behaves exactly like data addressed to it — only
    /// the egress conn is forced.
    ///
    /// Those flags must stay gated on `peer_id == my_peer_id`, exactly like the
    /// user path in [`Self::send_msg_by_ip_after_pipeline`]. Setting
    /// `not_send_to_tun` + `no_proxy` on a packet aimed at a **remote** peer
    /// disables both remote reply sources at once (the peer's `NicPacketProcessor`
    /// skips `!is_not_send_to_tun()` and its ICMP proxy passes `is_no_proxy`),
    /// so every per-link probe would time out and report "unreachable".
    ///
    /// Like [`Self::send_probe_msg_by_ip`] this is display-only: no
    /// `mark_recent_traffic`, no user-traffic accounting, no parking.
    pub async fn send_probe_msg_on_conn(
        &self,
        mut msg: ZCPacket,
        conn_id: &PeerConnId,
        dst_ip: IpAddr,
        peer_id: PeerId,
    ) -> Result<(), Error> {
        msg.fill_peer_manager_hdr(self.my_peer_id, 0, PacketType::Data as u8);
        let Some(peer) = self.peers.get_peer_by_id(peer_id) else {
            return Err(Error::NotFound);
        };

        try_compress_and_encrypt(
            self.data_compress_algo,
            &self.encryptor,
            &mut msg,
            self.is_secure_mode_enabled,
        )
        .await?;

        let packet_policy = self.context.packet_policy();
        let host_routing = self.context.host_routing_policy();
        let hdr = msg.mut_peer_manager_header().unwrap();
        hdr.to_peer_id.set(peer_id);
        hdr.set_latency_first(packet_policy.latency_first);
        if needs_self_delivery_guard(
            host_routing.local_exit_node_fallback,
            // A probe is "not sent to self" by definition: the only useful
            // destination is the remote peer, whose reply the prober awaits.
            true,
            peer_id,
            self.my_peer_id,
            self.context.is_ip_local_virtual_ip(&dst_ip),
        ) {
            // Loop prevention for self-delivery only. A per-link probe is
            // normally aimed at *another* node, where these flags would
            // suppress the ICMP reply we are waiting for (see the doc comment).
            hdr.set_not_send_to_tun(true);
            hdr.set_no_proxy(true);
        }

        peer.send_msg_on_conn(conn_id, msg).await
    }

    /// Live PeerConn ids for one peer (used by the per-link prober).
    pub fn peer_live_conn_ids(&self, peer_id: PeerId) -> Vec<PeerConnId> {
        self.peers
            .get_peer_by_id(peer_id)
            .map(|peer| peer.live_conn_ids())
            .unwrap_or_default()
    }

    /// `flags.disable_vip_reachability_probe`: probing is a display feature and
    /// may be turned off for compliance / weak devices (roadmap §3.2).
    pub fn vip_probe_disabled(&self) -> bool {
        self.context.flags().disable_vip_reachability_probe
    }

    /// TUN-less transports change what an ICMP probe means, so the roadmap
    /// (§3.2 / §7) makes them report "unknown + reason" instead of guessing.
    pub fn vip_probe_no_tun(&self) -> bool {
        self.context.flags().no_tun
    }

    pub async fn check_allow_kcp_to_dst(&self, dst_ip: &IpAddr) -> bool {
        self.outbound_packet_router
            .check_allow_kcp_to_dst(dst_ip)
            .await
    }

    /// See [`OutboundPacketRouter::check_allow_kcp_to_dst_with_flow`].
    pub async fn check_allow_kcp_to_dst_with_flow(
        &self,
        dst_ip: &IpAddr,
        flow_key: Option<u64>,
    ) -> bool {
        self.outbound_packet_router
            .check_allow_kcp_to_dst_with_flow(dst_ip, flow_key)
            .await
    }

    pub async fn check_allow_quic_to_dst(&self, dst_ip: &IpAddr) -> bool {
        self.outbound_packet_router
            .check_allow_quic_to_dst(dst_ip)
            .await
    }

    /// See [`OutboundPacketRouter::check_allow_quic_to_dst_with_flow`].
    pub async fn check_allow_quic_to_dst_with_flow(
        &self,
        dst_ip: &IpAddr,
        flow_key: Option<u64>,
    ) -> bool {
        self.outbound_packet_router
            .check_allow_quic_to_dst_with_flow(dst_ip, flow_key)
            .await
    }

    pub async fn update_exit_nodes(&self, exit_nodes: Vec<IpAddr>) {
        // The strategy table is **not** rebuilt on a hot patch, so its rules
        // keep the candidate list they were built with and the new exit nodes
        // are merged in at reconcile time. A new exit node therefore has to
        // satisfy the same invariants the constructor enforced — otherwise a
        // hot patch could slip past the `closed` + multi-candidate
        // confirmation (the multi-exit black-hole guard) or the candidate
        // limit.
        //
        // The `config_patch` path already validates this (the candidate is
        // checked before it is applied), so this is the second line of
        // defence for the programmatic entry points.
        if let Err(reason) = self.exit_nodes_are_acceptable(&exit_nodes) {
            tracing::warn!(
                ?exit_nodes,
                reason,
                "rejecting the exit node update: it would break a policy route invariant"
            );
            return;
        }
        *self.exit_nodes.write().await = exit_nodes;
    }

    /// Check a proposed `exit_nodes` list against the invariants the policy
    /// table's rules were built with.
    ///
    /// Mirrors `validate_route_rule` / `build_rules_from_config` for the part
    /// that depends on the *live* exit list, because the table is not rebuilt
    /// when it changes:
    ///   - the candidate limit (`MAX_PEERS_PER_RULE`), and
    ///   - the `closed` + multi-candidate confirmation (multi-exit black hole).
    ///
    /// A `balance` rule is the case the constructor check cannot see on its
    /// own: its candidates are the whole merged list, so one exit node turns a
    /// single-candidate rule into one that spreads flows over two or more
    /// exits — each of which can be a separate black hole.
    fn exit_nodes_are_acceptable(&self, exit_nodes: &[IpAddr]) -> Result<(), String> {
        let snap = self.local_route_table.load_full();
        for rule in &snap.rules {
            if !rule.rule.merge_exit_nodes {
                continue;
            }
            let merged = merged_candidate_count(&rule.rule, exit_nodes);
            if merged > MAX_PEERS_PER_RULE {
                return Err(format!(
                    "policy route {}: {merged} candidates exceed the peer limit {MAX_PEERS_PER_RULE}",
                    rule.rule.id
                ));
            }
            if merged > 1
                && rule.rule.if_all_unavailable == AllUnavailable::Closed
                && !rule.rule.require_confirmed
            {
                // `merged` is the list the rule can actually walk, and the
                // constructor now refuses a `closed` rule without
                // `require_confirmed` once that list exceeds one — so this is
                // the same guard applied to a hot-patched exit list.
                return Err(format!(
                    "policy route {}: if_all_unavailable=closed with {merged} peers requires \
                     require_confirmed=true",
                    rule.rule.id
                ));
            }
        }
        Ok(())
    }

    pub async fn configured_exit_nodes(&self) -> Vec<IpAddr> {
        self.exit_nodes.read().await.clone()
    }

    /// `true` when a `[[policy_routes]]` rule uses `select_by = balance`.
    ///
    /// Only such a rule picks its peer by the flow key, so a caller that would
    /// have to derive a flow key can skip that work when this is `false`.
    pub fn has_balance_policy_rules(&self) -> bool {
        self.local_route_table.has_balance_rules()
    }

    /// Unified routing table. The L2 proxy-CIDR reconciler reads its
    /// `Stable` / `Failover` CIDRs so both planes derive from **one** snapshot.
    pub(crate) fn local_route_table(&self) -> &LocalRouteTable {
        self.local_route_table.as_ref()
    }

    /// Management-plane view of every `[[policy_routes]]` rule after the live
    /// `exit_nodes` merge and the latest reconcile (L6).
    pub async fn effective_policy_routes(
        &self,
    ) -> Vec<super::route::local_route_table::EffectivePolicyRouteInfo> {
        use super::route::local_route_table::collect_effective_policy_routes;
        let exit_nodes = self.exit_nodes.read().await.clone();
        let snap = self.local_route_table.load_full();
        collect_effective_policy_routes(snap.as_ref(), &exit_nodes)
    }

    /// Record IPv4 addresses observed in a MagicDNS answer for `qname` (P3).
    ///
    /// Called by the DNS resolver wrapper on the answering path. The rule set
    /// is authoritative from local config; only the IP set is learned. No-op
    /// when no `[[policy_routes]]` are configured.
    pub fn observe_dns_answer(&self, qname: &str, ips: &[Ipv4Addr]) {
        self.local_route_table.observe_ips(qname, ips);
    }

    fn reload_acl(&self, acl: Option<&crate::proto::acl::Acl>) {
        self.acl_filter.reload_rules(acl);
    }

    pub async fn wait(&self) {
        while !self.tasks.lock().await.is_empty() {
            crate::foundation::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    pub(crate) async fn stop(&self) {
        let mut tasks = {
            let mut task_slot = self.tasks.lock().await;
            std::mem::replace(&mut *task_slot, JoinSet::new())
        };
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        self.foreign_network_manager.stop().await;
        self.route.close().await;
        self.peer_rpc_mgr.stop().await;
        self.context.stop().await;
        self.context.stats_manager().stop_cleanup_task().await;
        self.acl_filter.stop_cleanup_task().await;
    }

    pub(crate) async fn clear_resources(&self) {
        self.stop().await;
        self.foreign_network_client.stop();
        self.peers.clear_resources().await;
        self.foreign_network_client
            .get_peer_map()
            .clear_resources()
            .await;
        self.peer_packet_process_pipeline
            .store(Arc::new(Vec::new()));
        self.nic_packet_process_pipeline.store(Arc::new(Vec::new()));

        self.peer_rpc_mgr.rpc_server().registry().unregister_all();
    }

    pub async fn close_peer_conn(
        &self,
        peer_id: PeerId,
        conn_id: &PeerConnId,
    ) -> Result<(), Error> {
        close_peer_conn(
            self.peers.as_ref(),
            &self.foreign_network_client,
            &self.foreign_network_manager,
            peer_id,
            conn_id,
        )
        .await
    }

    async fn start_peer_recv(&self) {
        let packet_recv = self.packet_recv.lock().await.take().unwrap();
        let is_credential_node =
            self.context.network_identity().network_secret.is_none() && self.is_secure_mode_enabled;
        let router = PeerPacketRouter::new(
            packet_recv,
            self.my_peer_id,
            self.peers.clone(),
            self.peer_packet_process_pipeline.clone(),
            self.foreign_network_client.clone(),
            self.relay_peer_map.clone(),
            self.foreign_network_manager.clone(),
            self.encryptor.clone(),
            self.data_compress_algo,
            self.acl_filter.clone(),
            self.context.clone(),
            self.is_secure_mode_enabled,
            self.route.clone(),
            is_credential_node,
            self.traffic_metrics.clone(),
            self.context.stats_manager(),
            self.network_name.clone(),
            self.counters.self_tx_packets.clone(),
            self.counters.self_tx_bytes.clone(),
            self.counters.compress_tx_bytes_before.clone(),
            self.counters.compress_tx_bytes_after.clone(),
            self.flow_table.clone(),
        );

        self.tasks.lock().await.spawn(router.run());
    }

    pub(crate) async fn run(&self) -> Result<(), Error> {
        self.context.stats_manager().start_cleanup_task();

        if let Some(route) = self.route_algo_inst.ospf_route() {
            self.add_route(route).await;
        }

        init_packet_process_pipeline(
            &self.peer_packet_process_pipeline,
            self.nic_channel.clone(),
            self.peer_rpc_tspt.packet_sender(),
            self.counters.nic_channel_drops.clone(),
        )
        .await;
        self.peer_rpc_mgr.run();

        self.start_peer_recv().await;
        PeerMaintenanceTasks::new(
            self.peers.clone(),
            self.relay_peer_map.clone(),
            self.recent_traffic.clone(),
            self.foreign_network_client.clone(),
            self.peer_session_store.clone(),
            self.context.clone(),
            self.traffic_metrics.clone(),
            self.flow_table.clone(),
        )
        .spawn_into(&self.tasks)
        .await;

        // P1/P4: ~1 s reconcile loop for the unified routing table. Not spawned
        // at all when no `[[policy_routes]]` are configured, so the feature
        // costs nothing unless it is actually used.
        if !self.local_route_table.load_full().rules.is_empty() {
            let peers = self.peers.clone();
            let table = self.local_route_table.clone();
            let conn_select = ConnSelectConfig::from_flags(&self.context.flags());
            // B7: the exit nodes are read fresh on every tick so a hot-patched
            // list reaches the default-route rule's candidates.
            let exit_nodes = self.exit_nodes.clone();
            self.tasks.lock().await.spawn(async move {
                // Run once immediately so a `closed` rule with already-reachable
                // peers becomes Stable (and installs L2 routes) without waiting
                // a full second — otherwise OS traffic can leak out the physical
                // NIC for the first tick while the table is still Degraded.
                loop {
                    let live_exit_nodes = exit_nodes.read().await.clone();
                    reconcile_local_routes_once(
                        peers.as_ref(),
                        table.as_ref(),
                        conn_select,
                        &live_exit_nodes,
                    )
                    .await;
                    crate::foundation::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
        }

        self.foreign_network_client.run().await;

        Ok(())
    }

    /// Start the pending-packet flusher (P-AUDIT.12).
    ///
    /// Must be called once, after this manager is inside its `Arc`. Idle cost is
    /// zero: the loop parks on a notify and only ticks while packets are held.
    ///
    /// While the queue is non-empty the flusher also listens for
    /// [`Self::pending_packet_wake`] (peer-info / route changes) so a newly
    /// installed route can flush immediately instead of waiting out the 20ms
    /// tick. With `route_algo = none` there is no wake handle and only the tick
    /// applies.
    ///
    /// Invariant: a non-empty queue implies this task is inside the inner loop
    /// (or about to enter it after `wait_for_work`). Rejected pushes that only
    /// reclaim expired slots therefore do not need their own wake — the flusher
    /// is already running and will observe the smaller queue on its next pass.
    ///
    /// The task holds a `Weak` to the manager, so it exits as soon as the
    /// manager is dropped instead of keeping the whole graph alive.
    pub(crate) async fn start_pending_packet_flush(self: &Arc<Self>) {
        let pending = self.outbound_packet_router.pending_packets.clone();
        let this = Arc::downgrade(self);
        self.tasks.lock().await.spawn(async move {
            loop {
                pending.wait_for_work().await;

                loop {
                    let Some(peer_manager) = this.upgrade() else {
                        return;
                    };
                    // Dedicated wake — not `proxy_cidr_wake` — so this waiter
                    // cannot steal the L2 reconciler's `notify_one` permit.
                    let route_wake = peer_manager.pending_packet_wake();
                    // First pass after a new park: give route installation a
                    // short beat, then also react to peer-info wakes.
                    match route_wake.as_deref() {
                        Some(wake) => {
                            tokio::select! {
                                _ = wake.notified() => {}
                                _ = crate::foundation::time::sleep(PENDING_FLUSH_INTERVAL) => {}
                            }
                        }
                        None => {
                            crate::foundation::time::sleep(PENDING_FLUSH_INTERVAL).await;
                        }
                    }
                    peer_manager
                        .outbound_packet_router
                        .flush_pending_once()
                        .await;
                    drop(peer_manager);
                    if pending.is_empty() {
                        break;
                    }
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl PeerCenterPeerManagerTrait for PeerManagerCore {
    async fn list_peers(&self) -> PeerInfoForGlobalMap {
        direct_peer_info(&[
            self.get_peer_map(),
            self.get_foreign_network_client().get_peer_map(),
        ])
        .await
    }

    fn my_peer_id(&self) -> PeerId {
        self.my_peer_id()
    }

    fn network_name(&self) -> String {
        self.network_name().to_owned()
    }

    fn get_rpc_mgr(&self) -> Weak<PeerRpcManager> {
        Arc::downgrade(&self.get_peer_rpc_mgr())
    }

    async fn list_routes(&self) -> Vec<CoreRoute> {
        self.get_route().list_routes().await
    }
}

pub(crate) async fn close_peer_conn(
    peers: &PeerMap,
    foreign_network_client: &ForeignNetworkClient,
    foreign_network_manager: &ForeignNetworkManager,
    peer_id: PeerId,
    conn_id: &PeerConnId,
) -> Result<(), Error> {
    let ret = peers.close_peer_conn(peer_id, conn_id).await;
    tracing::info!("close_peer_conn in peer map: {:?}", ret);
    if ret.is_ok() || !matches!(ret.as_ref().unwrap_err(), Error::NotFound) {
        return ret;
    }

    let ret = foreign_network_client
        .get_peer_map()
        .close_peer_conn(peer_id, conn_id)
        .await;
    tracing::info!("close_peer_conn in foreign network client: {:?}", ret);
    if ret.is_ok() || !matches!(ret.as_ref().unwrap_err(), Error::NotFound) {
        return ret;
    }

    let ret = foreign_network_manager
        .close_peer_conn(peer_id, conn_id)
        .await;
    tracing::info!("close_peer_conn in foreign network manager done: {:?}", ret);
    ret
}

pub(crate) struct PeerConnectionAdmission {
    my_peer_id: PeerId,
    context: ArcPeerContext,
    peers: Arc<PeerMap>,
    foreign_network_client: Arc<ForeignNetworkClient>,
    foreign_network_manager: Arc<ForeignNetworkManager>,
    peer_session_store: Arc<PeerSessionStore>,
    recent_traffic: RecentTrafficTracker,
    reserved_my_peer_id_map: DashMap<String, PeerId>,
}

impl PeerConnectionAdmission {
    fn new(
        my_peer_id: PeerId,
        context: ArcPeerContext,
        peers: Arc<PeerMap>,
        foreign_network_client: Arc<ForeignNetworkClient>,
        foreign_network_manager: Arc<ForeignNetworkManager>,
        peer_session_store: Arc<PeerSessionStore>,
        recent_traffic: RecentTrafficTracker,
    ) -> Self {
        Self {
            my_peer_id,
            context,
            peers,
            foreign_network_client,
            foreign_network_manager,
            peer_session_store,
            recent_traffic,
            reserved_my_peer_id_map: DashMap::new(),
        }
    }

    pub async fn add_client_tunnel(
        &self,
        tunnel: Box<dyn Tunnel>,
        is_directly_connected: bool,
    ) -> Result<(PeerId, PeerConnId), Error> {
        self.add_client_tunnel_with_peer_id_hint(tunnel, is_directly_connected, None)
            .await
    }

    pub async fn add_client_tunnel_with_peer_id_hint(
        &self,
        tunnel: Box<dyn Tunnel>,
        is_directly_connected: bool,
        peer_id_hint: Option<PeerId>,
    ) -> Result<(PeerId, PeerConnId), Error> {
        self.add_client_tunnel_with_origin(
            tunnel,
            is_directly_connected,
            peer_id_hint,
            PeerConnectionOrigin::Network,
        )
        .await
    }

    async fn add_client_tunnel_with_origin(
        &self,
        tunnel: Box<dyn Tunnel>,
        is_directly_connected: bool,
        peer_id_hint: Option<PeerId>,
        origin: PeerConnectionOrigin,
    ) -> Result<(PeerId, PeerConnId), Error> {
        let mut peer = PeerConn::new_with_peer_id_hint_and_origin(
            self.my_peer_id,
            self.context.clone(),
            tunnel,
            peer_id_hint,
            self.peer_session_store.clone(),
            origin,
        );
        peer.set_is_hole_punched(!is_directly_connected);
        apply_hole_punch_ping_policy(&mut peer, is_directly_connected);
        peer.do_handshake_as_client().await?;
        let conn_id = peer.get_conn_id();
        let peer_id = peer.get_peer_id();
        let local_identity = self.context.network_identity();
        let is_local_network =
            peer.get_network_identity().network_name == local_identity.network_name;
        if origin == PeerConnectionOrigin::Attached && !is_local_network {
            return Err(Error::SecretKeyError(
                "attached ring peer must belong to the local network".to_string(),
            ));
        }
        if is_local_network {
            let local_secure_mode = self
                .context
                .secure_mode()
                .as_ref()
                .map(|cfg| cfg.enabled)
                .unwrap_or(false);
            let peer_id = add_new_peer_conn(
                self.peers.as_ref(),
                &local_identity,
                local_secure_mode,
                peer,
            )
            .await?;
            self.recent_traffic.clear(peer_id);
        } else {
            self.foreign_network_client.add_new_peer_conn(peer).await?;
        }
        Ok((peer_id, conn_id))
    }

    fn release_reserved_peer_id(&self, network_name: &str) {
        self.reserved_my_peer_id_map.remove(network_name);
        shrink_dashmap(&self.reserved_my_peer_id_map, None);
    }

    #[tracing::instrument(ret, skip(self, tunnel))]
    pub async fn add_tunnel_as_server(
        &self,
        tunnel: Box<dyn Tunnel>,
        is_directly_connected: bool,
    ) -> Result<(), Error> {
        self.add_tunnel_as_server_with_origin(
            tunnel,
            is_directly_connected,
            PeerConnectionOrigin::Network,
        )
        .await
        .map(|_| ())
    }

    async fn add_tunnel_as_server_with_origin(
        &self,
        tunnel: Box<dyn Tunnel>,
        is_directly_connected: bool,
        origin: PeerConnectionOrigin,
    ) -> Result<(PeerId, PeerConnId), Error> {
        tracing::info!("add tunnel as server start");
        let resolved_remote_addr = tunnel.info().and_then(|info| info.resolved_remote_addr);
        check_resolved_remote_addr_not_from_virtual_network(&self.context, resolved_remote_addr)?;

        let mut conn = PeerConn::new_with_peer_id_hint_and_origin(
            self.my_peer_id,
            self.context.clone(),
            tunnel,
            None,
            self.peer_session_store.clone(),
            origin,
        );
        let mut reserved_peer_id_network_name = None;
        let handshake_ret = conn
            .do_handshake_as_server_ext(|peer, network_name: &str| {
                if network_name == self.context.network_identity().network_name {
                    return Ok(());
                }

                let mut peer_id = self
                    .foreign_network_manager
                    .get_network_peer_id(network_name);
                if peer_id.is_none() {
                    reserved_peer_id_network_name = Some(network_name.to_string());
                    peer_id = Some(
                        *self
                            .reserved_my_peer_id_map
                            .entry(network_name.to_string())
                            .or_insert_with(random_peer_id)
                            .value(),
                    );
                }
                peer.set_peer_id(peer_id.unwrap());

                tracing::info!(
                    ?peer_id,
                    ?network_name,
                    "handshake as server with foreign network, new peer id: {}, peer id in foreign manager: {:?}",
                    peer.get_my_peer_id(), peer_id
                );

                Ok(())
            })
            .await;

        if let Err(err) = handshake_ret {
            if let Some(network_name) = reserved_peer_id_network_name {
                self.release_reserved_peer_id(&network_name);
            }
            return Err(err);
        }

        let peer_identity = conn.get_network_identity();
        let peer_network_name = peer_identity.network_name.clone();
        let local_identity = self.context.network_identity();
        let is_local_network = peer_network_name == local_identity.network_name;
        if origin == PeerConnectionOrigin::Attached && !is_local_network {
            self.release_reserved_peer_id(&peer_network_name);
            return Err(Error::SecretKeyError(
                "attached ring peer must belong to the local network".to_string(),
            ));
        }
        let peer_id = conn.get_peer_id();
        let conn_id = conn.get_conn_id();
        let trusted_foreign_credential =
            matches!(conn.get_peer_identity_type(), PeerIdentityType::Credential)
                && self
                    .foreign_network_manager
                    .is_existing_credential_pubkey_trusted(
                        &peer_network_name,
                        &conn.get_conn_info().noise_remote_static_pubkey,
                    );
        let foreign_network_allowed =
            conn.matches_local_network_secret() || trusted_foreign_credential;

        if !is_local_network && self.context.flags().private_mode && !foreign_network_allowed {
            self.release_reserved_peer_id(&peer_network_name);
            return Err(Error::SecretKeyError(
                "private mode is turned on, foreign network secret mismatch".to_string(),
            ));
        }

        conn.set_is_hole_punched(!is_directly_connected);
        apply_hole_punch_ping_policy(&mut conn, is_directly_connected);

        let add_peer_ret = if is_local_network {
            let local_secure_mode = self
                .context
                .secure_mode()
                .as_ref()
                .map(|cfg| cfg.enabled)
                .unwrap_or(false);
            match add_new_peer_conn(
                self.peers.as_ref(),
                &local_identity,
                local_secure_mode,
                conn,
            )
            .await
            {
                Ok(peer_id) => {
                    self.recent_traffic.clear(peer_id);
                    Ok(())
                }
                Err(err) => Err(err),
            }
        } else {
            self.foreign_network_manager.add_peer_conn(conn).await
        };

        if let Err(err) = add_peer_ret {
            self.release_reserved_peer_id(&peer_network_name);
            return Err(err);
        }

        self.release_reserved_peer_id(&peer_network_name);

        tracing::info!("add tunnel as server done");
        Ok((peer_id, conn_id))
    }
}

/// TCP and UDP hole punch both set `is_hole_punched`; only non-UDP needs a 1s
/// ping cap so NAT mappings stay alive. Detect via tunnel_type without expanding
/// `PeerConnectionOrigin` or changing `HolePunchTunnelSink`.
///
/// Only hole-punch admissions pass `is_directly_connected=false`.
/// Control-ping ceiling for a hole-punched TCP / FakeTCP conn.
///
/// 1s: intermediate devices cut idle punched TCP flows (#2632). See
/// `connection-stability-todo.md` S4.
const HOLE_PUNCH_TCP_PING_MAX: Duration = Duration::from_secs(1);

/// Control-ping ceiling for a hole-punched **UDP** conn.
///
/// The adaptive ping backs off to `flags.ping_interval_max_sec` (now also 10s
/// by default; previously 32s) once a link goes idle. NAT UDP state aging is
/// commonly ~30s, so a 32s gap let the mapping expire: data blackholed until
/// the *next* ping failed, five consecutive failures tore the conn down, and
/// recovery needed a full re-punch plus handshake. 10s keeps 3x margin against
/// a 30s aging timer for the cost of a few 9-byte control packets per conn.
///
/// `PeerConn::max_ping_interval` returns `min(cap, flags_max)`, so an operator
/// who configures a stricter `ping_interval_max_sec` still wins; only a laxer
/// one is tightened here, because the NAT keepalive is a correctness
/// requirement rather than a tuning knob.
const HOLE_PUNCH_UDP_PING_MAX: Duration = Duration::from_secs(10);

/// Control-ping ceiling for a hole-punched conn, or `None` when it should keep
/// the global `flags.ping_interval_max_sec` (directly-connected conns).
fn hole_punch_ping_max(tunnel_type: Option<&str>, is_directly_connected: bool) -> Option<Duration> {
    if is_directly_connected {
        return None;
    }
    // UDP hole punch always reports tunnel_type "udp". TCP / FakeTCP may use
    // plain "tcp"/"faketcp" or an opaque host transport_label (e.g.
    // "faketcp_test-driver", "host-tcp") — treat everything except UDP as TCP-like.
    if tunnel_type.is_some_and(is_udp_hole_punch_tunnel_type) {
        Some(HOLE_PUNCH_UDP_PING_MAX)
    } else {
        Some(HOLE_PUNCH_TCP_PING_MAX)
    }
}

fn apply_hole_punch_ping_policy(conn: &mut PeerConn, is_directly_connected: bool) {
    if let Some(cap) = hole_punch_ping_max(conn.tunnel_type(), is_directly_connected) {
        conn.set_ping_max_interval(Some(cap));
    }
}

fn is_udp_hole_punch_tunnel_type(tunnel_type: &str) -> bool {
    let scheme = tunnel_type
        .split_once("://")
        .map(|(s, _)| s)
        .unwrap_or(tunnel_type);
    scheme == "udp" || scheme.ends_with("-udp")
}

#[cfg(test)]
mod hole_punch_ping_policy_tests {
    use super::{
        HOLE_PUNCH_TCP_PING_MAX, HOLE_PUNCH_UDP_PING_MAX, hole_punch_ping_max,
        is_udp_hole_punch_tunnel_type,
    };
    use std::time::Duration;

    #[test]
    fn classifies_udp_vs_tcp_hole_punch_transports() {
        assert!(is_udp_hole_punch_tunnel_type("udp"));
        assert!(is_udp_hole_punch_tunnel_type("txt-udp"));
        assert!(!is_udp_hole_punch_tunnel_type("tcp"));
        assert!(!is_udp_hole_punch_tunnel_type("faketcp"));
        assert!(!is_udp_hole_punch_tunnel_type("host-tcp"));
        assert!(!is_udp_hole_punch_tunnel_type("host-accepted"));
        assert!(!is_udp_hole_punch_tunnel_type("faketcp_test-driver"));
        assert!(!is_udp_hole_punch_tunnel_type("faketcp://127.0.0.1:1"));
    }

    #[test]
    fn udp_hole_punch_gets_nat_safe_ping_ceiling() {
        // The point of the fix: a 32s ceiling can outlive a ~30s NAT UDP
        // mapping and blackhole the conn before anything notices. The global
        // default is now 10s as well, so the UDP ceiling must be no *looser*
        // than it.
        assert_eq!(
            hole_punch_ping_max(Some("udp"), false),
            Some(HOLE_PUNCH_UDP_PING_MAX)
        );
        assert_eq!(
            hole_punch_ping_max(Some("txt-udp"), false),
            Some(HOLE_PUNCH_UDP_PING_MAX)
        );
        assert!(
            HOLE_PUNCH_UDP_PING_MAX
                <= Duration::from_secs(crate::config::toml::DEFAULT_PING_INTERVAL_MAX_SEC as u64),
            "UDP ceiling must not be looser than the default ping_interval_max_sec"
        );
        // Still far stricter than the ~30s NAT aging timer it protects.
        assert!(HOLE_PUNCH_UDP_PING_MAX * 3 <= Duration::from_secs(30));
    }

    #[test]
    fn tcp_like_hole_punch_keeps_aggressive_ceiling() {
        for ty in ["tcp", "faketcp", "host-tcp", "faketcp_test-driver"] {
            assert_eq!(
                hole_punch_ping_max(Some(ty), false),
                Some(HOLE_PUNCH_TCP_PING_MAX),
                "{ty} must keep the 1s cap"
            );
        }
        // Unknown/absent tunnel type is treated as TCP-like, matching
        // `is_udp_hole_punch_tunnel_type` returning false.
        assert_eq!(
            hole_punch_ping_max(None, false),
            Some(HOLE_PUNCH_TCP_PING_MAX)
        );
    }

    #[test]
    fn directly_connected_conns_keep_the_global_default() {
        assert_eq!(hole_punch_ping_max(Some("udp"), true), None);
        assert_eq!(hole_punch_ping_max(Some("tcp"), true), None);
        assert_eq!(hole_punch_ping_max(None, true), None);
    }

    #[test]
    fn per_conn_cap_never_relaxes_the_operator_setting() {
        // `PeerConn::max_ping_interval` = min(cap, flags_max): a stricter
        // operator value must survive, and only a laxer one gets tightened.
        let flags_max =
            Duration::from_secs(crate::config::toml::DEFAULT_PING_INTERVAL_MAX_SEC as u64);
        assert_eq!(
            HOLE_PUNCH_UDP_PING_MAX.min(flags_max),
            HOLE_PUNCH_UDP_PING_MAX
        );
        let stricter = Duration::from_secs(3);
        assert_eq!(HOLE_PUNCH_UDP_PING_MAX.min(stricter), stricter);
    }
}

// Keep lazy-p2p demand alive across the 5s task rescan interval and a full on-demand
// connect attempt, without retaining extra per-task state in the hot path.
pub(crate) const RECENT_HAVE_TRAFFIC_TTL: Duration = Duration::from_secs(30);

pub(crate) fn should_mark_recent_traffic_for_fanout(total_dst_peers: usize) -> bool {
    total_dst_peers <= 1
}

fn gc_recent_traffic_entries<F>(
    recent_have_traffic: &DashMap<PeerId, Instant>,
    now: Instant,
    mut has_directly_connected_conn: F,
) where
    F: FnMut(PeerId) -> bool,
{
    let mut to_remove = Vec::new();
    for entry in recent_have_traffic.iter() {
        let peer_id = *entry.key();
        let expired = now.saturating_duration_since(*entry.value()) > RECENT_HAVE_TRAFFIC_TTL;
        if expired || has_directly_connected_conn(peer_id) {
            to_remove.push(peer_id);
        }
    }

    if !to_remove.is_empty() {
        for peer_id in to_remove {
            recent_have_traffic.remove(&peer_id);
        }
        shrink_dashmap(recent_have_traffic, None);
    }
}

#[derive(Clone)]
pub(crate) struct RecentTrafficTracker {
    my_peer_id: PeerId,
    recent_have_traffic: Arc<DashMap<PeerId, Instant>>,
    p2p_demand_notify: Arc<ExternalTaskSignal>,
}

impl RecentTrafficTracker {
    pub fn new(my_peer_id: PeerId) -> Self {
        Self {
            my_peer_id,
            recent_have_traffic: Arc::new(DashMap::new()),
            p2p_demand_notify: Arc::new(ExternalTaskSignal::new()),
        }
    }

    pub fn mark<F>(
        &self,
        dst_peer_id: PeerId,
        disable_p2p: bool,
        lazy_p2p: bool,
        mut has_directly_connected_conn: F,
    ) where
        F: FnMut(PeerId) -> bool,
    {
        if dst_peer_id == self.my_peer_id {
            return;
        }

        if disable_p2p || !lazy_p2p || has_directly_connected_conn(dst_peer_id) {
            return;
        }

        let now = Instant::now();
        if let Some(mut last_seen) = self.recent_have_traffic.get_mut(&dst_peer_id) {
            let should_notify = now.saturating_duration_since(*last_seen) > RECENT_HAVE_TRAFFIC_TTL;
            *last_seen = now;
            if !should_notify {
                return;
            }
        } else {
            self.recent_have_traffic.insert(dst_peer_id, now);
        }
        self.p2p_demand_notify.notify();
    }

    pub fn has<F>(&self, peer_id: PeerId, now: Instant, mut has_directly_connected_conn: F) -> bool
    where
        F: FnMut(PeerId) -> bool,
    {
        if has_directly_connected_conn(peer_id) {
            return false;
        }

        self.recent_have_traffic
            .get(&peer_id)
            .map(|last_seen| now.saturating_duration_since(*last_seen) <= RECENT_HAVE_TRAFFIC_TTL)
            .unwrap_or(false)
    }

    pub fn clear(&self, peer_id: PeerId) {
        self.recent_have_traffic.remove(&peer_id);
    }

    pub fn gc<F>(&self, now: Instant, has_directly_connected_conn: F)
    where
        F: FnMut(PeerId) -> bool,
    {
        gc_recent_traffic_entries(
            self.recent_have_traffic.as_ref(),
            now,
            has_directly_connected_conn,
        );
    }

    pub fn p2p_demand_notify(&self) -> Arc<ExternalTaskSignal> {
        self.p2p_demand_notify.clone()
    }
}

pub(crate) struct PeerMaintenanceTasks {
    peer_map: Arc<PeerMap>,
    relay_peer_map: Arc<RelayPeerMap>,
    recent_traffic: RecentTrafficTracker,
    foreign_network_client: Arc<ForeignNetworkClient>,
    peer_session_store: Arc<PeerSessionStore>,
    context: ArcPeerContext,
    traffic_metrics: Arc<TrafficMetricRecorder>,
    /// Per-tunnel IP flow accounting (disabled by default).
    flow_table: Arc<FlowTable>,
}

impl PeerMaintenanceTasks {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        peer_map: Arc<PeerMap>,
        relay_peer_map: Arc<RelayPeerMap>,
        recent_traffic: RecentTrafficTracker,
        foreign_network_client: Arc<ForeignNetworkClient>,
        peer_session_store: Arc<PeerSessionStore>,
        context: ArcPeerContext,
        traffic_metrics: Arc<TrafficMetricRecorder>,
        flow_table: Arc<FlowTable>,
    ) -> Self {
        Self {
            peer_map,
            relay_peer_map,
            recent_traffic,
            foreign_network_client,
            peer_session_store,
            context,
            traffic_metrics,
            flow_table,
        }
    }

    pub async fn spawn_into(self, tasks: &Mutex<JoinSet<()>>) {
        self.spawn_clean_peer_without_conn_routine(tasks).await;
        self.spawn_relay_session_gc_routine(tasks).await;
        self.spawn_recent_traffic_gc_routine(tasks).await;
        self.spawn_peer_session_gc_routine(tasks).await;
        self.spawn_credential_gc_routine(tasks).await;
        self.spawn_traffic_metrics_gc_routine(tasks).await;
        self.spawn_flow_stats_routines(tasks).await;
    }

    /// Rate window, GC and aggregate log for the per-tunnel IP flow accounting.
    ///
    /// The rate window is closed at a fixed cadence (1s) so the reported rate
    /// does not depend on how often a consumer polls the table.
    ///
    /// **Nothing is spawned when accounting is off (F-4).** Decision D5 promises
    /// zero overhead on low-end devices, and three timers waking up every
    /// 1s / 5s / 30s is a real cost even with an empty table.
    ///
    /// This early return is only sound because `FlowTable::configure` is private
    /// (F-3): `enabled` cannot change after construction, so "off at
    /// construction" means "off for the lifetime of this instance". If
    /// `configure` is ever made public again, this must become a
    /// notification-driven start instead — otherwise a table enabled later
    /// never rotates its rate window (rate stuck at 0) and never runs `gc`
    /// (unbounded growth).
    async fn spawn_flow_stats_routines(&self, tasks: &Mutex<JoinSet<()>>) {
        if !self.flow_table.is_enabled() {
            return;
        }

        let flow_table = self.flow_table.clone();
        tasks.lock().await.spawn(async move {
            loop {
                flow_table.rotate_rate_window();
                crate::foundation::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });

        let flow_table = self.flow_table.clone();
        tasks.lock().await.spawn(async move {
            loop {
                flow_table.gc(crate::peers::flow_table::now_unix_millis());
                crate::foundation::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });

        // Decision D2: a single aggregate line every 5s instead of per-packet
        // tracing. Silent when the table is empty. The `is_enabled` check below
        // is belt-and-braces: the whole routine set is not spawned at all when
        // accounting is off (F-4).
        let flow_table = self.flow_table.clone();
        tasks.lock().await.spawn(async move {
            loop {
                crate::foundation::time::sleep(std::time::Duration::from_secs(5)).await;
                if !flow_table.is_enabled() {
                    continue;
                }
                let summary = flow_table.summary();
                if summary.entries == 0 {
                    continue;
                }
                tracing::info!(
                    target: "CORE::FLOW",
                    conns = summary.conns,
                    flows = summary.entries,
                    tx_bytes = summary.tx_bytes,
                    rx_bytes = summary.rx_bytes,
                    rate_bytes_per_sec = summary.rate_bytes_per_sec,
                    "flow aggregate"
                );
            }
        });
    }

    async fn spawn_clean_peer_without_conn_routine(&self, tasks: &Mutex<JoinSet<()>>) {
        let peer_map = self.peer_map.clone();
        tasks.lock().await.spawn(async move {
            loop {
                peer_map.clean_peer_without_conn().await;
                crate::foundation::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        });
    }

    async fn spawn_relay_session_gc_routine(&self, tasks: &Mutex<JoinSet<()>>) {
        let relay_peer_map = self.relay_peer_map.clone();
        tasks.lock().await.spawn(async move {
            loop {
                relay_peer_map.evict_idle_sessions(std::time::Duration::from_secs(60));
                crate::foundation::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
    }

    async fn spawn_recent_traffic_gc_routine(&self, tasks: &Mutex<JoinSet<()>>) {
        let recent_traffic = self.recent_traffic.clone();
        let peers = self.peer_map.clone();
        let foreign_network_client = self.foreign_network_client.clone();
        tasks.lock().await.spawn(async move {
            loop {
                recent_traffic.gc(Instant::now(), |peer_id| {
                    if let Some(peer) = peers.get_peer_by_id(peer_id) {
                        peer.has_directly_connected_conn()
                    } else {
                        foreign_network_client.get_peer_map().has_peer(peer_id)
                    }
                });
                crate::foundation::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
    }

    async fn spawn_peer_session_gc_routine(&self, tasks: &Mutex<JoinSet<()>>) {
        let peer_session_store = self.peer_session_store.clone();
        tasks.lock().await.spawn(async move {
            loop {
                crate::foundation::time::sleep(std::time::Duration::from_secs(60)).await;
                peer_session_store.evict_unused_sessions();
            }
        });
    }

    async fn spawn_credential_gc_routine(&self, tasks: &Mutex<JoinSet<()>>) {
        let context = self.context.clone();
        let peer_map = self.peer_map.clone();
        tasks.lock().await.spawn(async move {
            loop {
                if context.network_identity().network_secret.is_some() {
                    if context.remove_expired_credentials() {
                        context.issue_credential_changed();
                    }

                    let network_name = context.network_name();
                    close_untrusted_credential_peers(
                        peer_map.as_ref(),
                        &network_name,
                        |pubkey, network_name| context.is_pubkey_trusted(pubkey, network_name),
                    )
                    .await;
                }
                crate::foundation::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
    }

    async fn spawn_traffic_metrics_gc_routine(&self, tasks: &Mutex<JoinSet<()>>) {
        let Some(mut event_receiver) = self.context.subscribe_peer_events() else {
            return;
        };
        let context = self.context.clone();
        let traffic_metrics = self.traffic_metrics.clone();
        tasks.lock().await.spawn(async move {
            loop {
                match event_receiver.recv().await {
                    Ok(super::context::PeerContextEvent::PeerRemoved(peer_id)) => {
                        traffic_metrics.remove_peer(peer_id);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            skipped,
                            "traffic metrics GC receiver lagged; clearing peer cache to avoid stale metric attribution"
                        );
                        traffic_metrics.clear_peer_cache();
                        let Some(new_receiver) = context.subscribe_peer_events() else {
                            break;
                        };
                        event_receiver = new_receiver;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

pub(crate) async fn try_compress_and_encrypt(
    compress_algo: CompressorAlgo,
    encryptor: &Arc<dyn Encryptor + 'static>,
    msg: &mut ZCPacket,
    secure_mode_enabled: bool,
) -> Result<(), Error> {
    let compressor = DefaultCompressor {};
    compressor
        .compress(msg, compress_algo)
        .await
        .with_context(|| "compress failed")?;
    if !secure_mode_enabled {
        encryptor.encrypt(msg).with_context(|| "encrypt failed")?;
    }
    Ok(())
}

struct PeerOutboundPacketRouterCounters {
    self_tx_packets: CounterHandle,
    self_tx_bytes: CounterHandle,
    compress_tx_bytes_before: CounterHandle,
    compress_tx_bytes_after: CounterHandle,
    no_dst_peer_drops: CounterHandle,
    /// P-AUDIT.12: the packet found no route but was parked for a short retry
    /// instead of being dropped (route installation was likely in flight).
    no_dst_peer_buffered: CounterHandle,
    tunnel_buffer_full: CounterHandle,
    /// A parked packet's flush attempt errored and was abandoned rather than
    /// re-parked. Separate from `no_dst_peer_drops` so the buffering signal
    /// stays clean.
    pending_flush_errors: CounterHandle,
}

pub(crate) struct PeerOutboundPacketRouter {
    my_peer_id: PeerId,
    context: ArcPeerContext,
    host_routing: HostRoutingPolicy,
    peers: Arc<PeerMap>,
    route: ArcRoute,
    foreign_network_client: Arc<ForeignNetworkClient>,
    relay_peer_map: Arc<RelayPeerMap>,
    nic_packet_process_pipeline: NicPacketPipeline,
    encryptor: Arc<dyn Encryptor>,
    data_compress_algo: CompressorAlgo,
    exit_nodes: Arc<RwLock<Vec<IpAddr>>>,
    local_route_table: Arc<LocalRouteTable>,
    recent_traffic: RecentTrafficTracker,
    traffic_metrics: Arc<TrafficMetricRecorder>,
    acl_filter: Arc<AclFilter>,
    is_secure_mode_enabled: bool,
    counters: PeerOutboundPacketRouterCounters,
    /// Packets parked briefly because their destination had no route yet.
    /// See `peers::pending_packets` (P-AUDIT.12).
    pending_packets: Arc<PendingPackets>,
    /// Per-tunnel IP flow accounting (disabled by default). Used to decide
    /// whether the inner destination IP has to be attached to the packet
    /// before it is encrypted.
    flow_table: Arc<FlowTable>,
}

impl PeerOutboundPacketRouter {
    #[allow(clippy::too_many_arguments)]
    fn new(
        my_peer_id: PeerId,
        context: ArcPeerContext,
        peers: Arc<PeerMap>,
        route: ArcRoute,
        foreign_network_client: Arc<ForeignNetworkClient>,
        relay_peer_map: Arc<RelayPeerMap>,
        nic_packet_process_pipeline: NicPacketPipeline,
        encryptor: Arc<dyn Encryptor>,
        data_compress_algo: CompressorAlgo,
        exit_nodes: Arc<RwLock<Vec<IpAddr>>>,
        local_route_table: Arc<LocalRouteTable>,
        recent_traffic: RecentTrafficTracker,
        traffic_metrics: Arc<TrafficMetricRecorder>,
        acl_filter: Arc<AclFilter>,
        is_secure_mode_enabled: bool,
        self_tx_packets: CounterHandle,
        self_tx_bytes: CounterHandle,
        compress_tx_bytes_before: CounterHandle,
        compress_tx_bytes_after: CounterHandle,
        no_dst_peer_drops: CounterHandle,
        no_dst_peer_buffered: CounterHandle,
        tunnel_buffer_full: CounterHandle,
        pending_flush_errors: CounterHandle,
        flow_table: Arc<FlowTable>,
    ) -> Self {
        let host_routing = context.host_routing_policy();
        Self {
            my_peer_id,
            context,
            host_routing,
            peers,
            route,
            foreign_network_client,
            relay_peer_map,
            nic_packet_process_pipeline,
            encryptor,
            data_compress_algo,
            exit_nodes,
            local_route_table,
            recent_traffic,
            traffic_metrics,
            acl_filter,
            is_secure_mode_enabled,
            counters: PeerOutboundPacketRouterCounters {
                self_tx_packets,
                self_tx_bytes,
                compress_tx_bytes_before,
                compress_tx_bytes_after,
                no_dst_peer_drops,
                no_dst_peer_buffered,
                tunnel_buffer_full,
                pending_flush_errors,
            },
            pending_packets: Arc::new(PendingPackets::new()),
            flow_table,
        }
    }

    fn has_directly_connected_conn(&self, peer_id: PeerId) -> bool {
        if let Some(peer) = self.peers.get_peer_by_id(peer_id) {
            peer.has_directly_connected_conn()
        } else {
            self.foreign_network_client.get_peer_map().has_peer(peer_id)
        }
    }

    fn mark_recent_traffic_with_policy(
        &self,
        dst_peer_id: PeerId,
        packet_policy: PeerPacketPolicy,
    ) {
        self.recent_traffic.mark(
            dst_peer_id,
            packet_policy.disable_p2p,
            packet_policy.lazy_p2p,
            |peer_id| self.has_directly_connected_conn(peer_id),
        );
    }

    async fn run_nic_packet_process_pipeline(&self, data: &mut ZCPacket) -> bool {
        // Enforce ACL for outbound (NIC-originated) packets. If ACL denies, stop processing.
        if !self.acl_filter.process_packet_with_acl(
            data,
            false,
            None,
            |_| false,
            self.route.as_ref(),
        ) {
            return false;
        }

        let pipelines = self.nic_packet_process_pipeline.load_full();
        for pipeline in pipelines.iter().rev() {
            if let Some(filter) = pipeline.filter_if_active() {
                let _ = filter.try_process_packet_from_nic(data).await;
            }
        }

        true
    }

    fn check_p2p_only_before_send(
        &self,
        dst_peer_id: PeerId,
        packet_policy: PeerPacketPolicy,
    ) -> Result<(), Error> {
        if packet_policy.p2p_only && !self.peers.has_peer(dst_peer_id) {
            return Err(Error::RouteError(None));
        }
        Ok(())
    }

    async fn check_allow_wrapped_proxy_to_dst(
        &self,
        dst_ip: &IpAddr,
        flow_key: Option<u64>,
        dst_allows_input: impl Fn(crate::proto::common::PeerFeatureFlag) -> bool,
        next_hop_disables_relay: impl Fn(crate::proto::common::PeerFeatureFlag) -> bool,
    ) -> bool {
        // Unified routing table first: `Route::get_peer_id_by_ip_allowing_default_proxy`
        // cannot see the local policy table, so without this the KCP / QUIC /
        // wrapped-TCP optimizations would be disabled for those flows (C9).
        let owner = match dst_ip {
            IpAddr::V4(v4) => {
                // A `balance` rule picks the peer by the flow key. Without a
                // flow key we cannot know which candidate will carry the flow,
                // and `lookup` would fall back to slot 0 — answering for a peer
                // that may not be the one. Refuse the bypass rather than judge
                // the wrong peer: wrongly allowing it would hand the flow to a
                // peer that forbids it, while wrongly refusing it only costs
                // the optimization.
                if flow_key.is_none() && self.local_route_table.is_balance_ambiguous(v4) {
                    return false;
                }
                match self.local_route_table.lookup(v4, flow_key) {
                    RouteLookup::Hit { peer } => Some(peer),
                    // A blocked rule must not fall back to the route layer either —
                    // that could silently re-enable the bypass it just refused.
                    RouteLookup::Block => return false,
                    RouteLookup::Miss => None,
                }
            }
            IpAddr::V6(_) => None, // P1: IPv4 only
        };
        let owner = match owner {
            Some(peer_id) => Some(peer_id),
            None => {
                self.route
                    .get_peer_id_by_ip_allowing_default_proxy(dst_ip)
                    .await
            }
        };
        let Some(dst_peer_id) = owner else {
            return false;
        };
        let Some(peer_info) = self.route.get_peer_info(dst_peer_id).await else {
            return false;
        };

        if !peer_info
            .feature_flag
            .map(dst_allows_input)
            .unwrap_or(false)
        {
            return false;
        }

        let next_hop_policy = get_next_hop_policy(self.context.flags().latency_first);
        let Some(next_hop_id) = self
            .route
            .get_next_hop_with_policy(dst_peer_id, next_hop_policy)
            .await
        else {
            return false;
        };

        if next_hop_id == dst_peer_id {
            return true;
        }

        let Some(next_hop_info) = self.route.get_peer_info(next_hop_id).await else {
            return false;
        };

        !next_hop_info
            .feature_flag
            .map(next_hop_disables_relay)
            .unwrap_or(false)
    }

    pub async fn check_allow_kcp_to_dst(&self, dst_ip: &IpAddr) -> bool {
        self.check_allow_kcp_to_dst_with_flow(dst_ip, None).await
    }

    /// Same as [`Self::check_allow_kcp_to_dst`], but with the flow key of the
    /// packet that is about to be sent.
    ///
    /// A `balance` rule picks the peer by the flow key, so a caller that knows
    /// the flow is judged against the peer that will really carry it. A caller
    /// that does not know the flow gets the conservative answer (see
    /// `check_allow_wrapped_proxy_to_dst`).
    pub async fn check_allow_kcp_to_dst_with_flow(
        &self,
        dst_ip: &IpAddr,
        flow_key: Option<u64>,
    ) -> bool {
        self.check_allow_wrapped_proxy_to_dst(
            dst_ip,
            flow_key,
            |feature_flag| feature_flag.kcp_input,
            |feature_flag| feature_flag.no_relay_kcp,
        )
        .await
    }

    pub async fn check_allow_quic_to_dst(&self, dst_ip: &IpAddr) -> bool {
        self.check_allow_quic_to_dst_with_flow(dst_ip, None).await
    }

    /// See [`Self::check_allow_kcp_to_dst_with_flow`].
    pub async fn check_allow_quic_to_dst_with_flow(
        &self,
        dst_ip: &IpAddr,
        flow_key: Option<u64>,
    ) -> bool {
        self.check_allow_wrapped_proxy_to_dst(
            dst_ip,
            flow_key,
            |feature_flag| feature_flag.quic_input,
            |feature_flag| feature_flag.no_relay_quic,
        )
        .await
    }

    pub async fn send_msg_for_proxy(
        &self,
        mut msg: ZCPacket,
        dst_peer_id: PeerId,
    ) -> Result<(), Error> {
        let packet_policy = self.context.packet_policy();
        self.mark_recent_traffic_with_policy(dst_peer_id, packet_policy);
        self.check_p2p_only_before_send(dst_peer_id, packet_policy)?;

        // Sticky key must be taken from plaintext before compress/encrypt.
        msg.set_bond_flow_key(flow_key_from_payload(msg.payload()));

        self.counters
            .compress_tx_bytes_before
            .add(msg.buf_len() as u64);

        try_compress_and_encrypt(
            self.data_compress_algo,
            &self.encryptor,
            &mut msg,
            self.is_secure_mode_enabled,
        )
        .await?;

        self.counters
            .compress_tx_bytes_after
            .add(msg.buf_len() as u64);

        let msg_len = msg.buf_len() as u64;
        let result = send_msg_internal(
            self.peers.as_ref(),
            &self.foreign_network_client,
            &self.relay_peer_map,
            Some(&self.traffic_metrics),
            msg,
            dst_peer_id,
        )
        .await;
        if result.is_ok() {
            self.counters.self_tx_bytes.add(msg_len);
            self.counters.self_tx_packets.inc();
        } else if let Err(ref e) = result {
            note_tunnel_buffer_full(&self.counters.tunnel_buffer_full, e);
        }
        result
    }

    pub async fn get_msg_dst_peer(
        &self,
        addr: &IpAddr,
        flow_key: Option<u64>,
    ) -> (SmallVec<[PeerId; 1]>, bool) {
        match addr {
            IpAddr::V4(ipv4_addr) => self.get_msg_dst_peer_ipv4(ipv4_addr, flow_key).await,
            IpAddr::V6(ipv6_addr) => self.get_msg_dst_peer_ipv6(ipv6_addr).await,
        }
    }

    fn is_all_peers_broadcast_ipv4(&self, ipv4_addr: &Ipv4Addr) -> bool {
        let network_length = self
            .context
            .ipv4()
            .map(|x| x.network_length())
            .unwrap_or(24);
        let ipv4_inet = cidr::Ipv4Inet::new(*ipv4_addr, network_length).unwrap();
        ipv4_addr.is_broadcast()
            || ipv4_addr.is_multicast()
            || *ipv4_addr == ipv4_inet.last_address()
    }

    fn is_all_peers_broadcast_ipv6(&self, ipv6_addr: &Ipv6Addr) -> bool {
        let network_length = self
            .context
            .ipv6()
            .map(|x| x.network_length())
            .unwrap_or(64);
        let ipv6_inet = cidr::Ipv6Inet::new(*ipv6_addr, network_length).unwrap();
        ipv6_addr.is_multicast() || *ipv6_addr == ipv6_inet.last_address()
    }

    fn select_ipv4_broadcast_peers<'a>(
        routes: impl IntoIterator<Item = &'a CoreRoute>,
        my_peer_id: PeerId,
    ) -> Vec<PeerId> {
        routes
            .into_iter()
            .filter_map(|route| {
                (route.peer_id != my_peer_id && route.ipv4_addr.is_some()).then_some(route.peer_id)
            })
            .collect()
    }

    async fn resolve_exit_node_peer(&self) -> Option<PeerId> {
        let exit_nodes = self.exit_nodes.read().await;
        self.peers.resolve_exit_node_peer(&exit_nodes).await
    }

    pub async fn get_msg_dst_peer_ipv4(
        &self,
        ipv4_addr: &Ipv4Addr,
        flow_key: Option<u64>,
    ) -> (SmallVec<[PeerId; 1]>, bool) {
        let mut is_exit_node = false;
        // Single-destination hot path stays stack-allocated; broadcast may spill.
        let mut dst_peers = SmallVec::<[PeerId; 1]>::new();
        if self.is_all_peers_broadcast_ipv4(ipv4_addr) {
            dst_peers.extend(Self::select_ipv4_broadcast_peers(
                &self.peers.list_route_infos().await,
                self.my_peer_id,
            ));
        } else if let Some(peer_id) = self.peers.get_peer_id_by_ipv4(ipv4_addr).await {
            dst_peers.push(peer_id);
        } else if !self
            .context
            .is_ip_in_same_network(&std::net::IpAddr::V4(*ipv4_addr))
        {
            // Step 3.5: unified routing table. Sync, lock-free, no alloc; the
            // snapshot already holds the reconciled `active_peer`. Only reached
            // after VIP + peer-proxy CIDR missed (subnet proxy wins).
            match self.local_route_table.lookup(ipv4_addr, flow_key) {
                RouteLookup::Hit { peer } => {
                    // A policy route always hands off to a gateway peer that
                    // NATs the packet out (§4.1 "B 做网关"), so the exit flag is
                    // always set. The gateway still prefers its own
                    // `proxy_cidrs` mapping when it has one.
                    dst_peers.push(peer);
                    is_exit_node = true;
                }
                RouteLookup::Block => {
                    // `if_all_unavailable = closed` with every candidate down:
                    // drop instead of leaking out the physical gateway. The
                    // Linux blackhole route (Q6) is a later optimisation — an
                    // L3 refusal works on every platform.
                    // Counted via send_msg_by_ip empty-dst path (no_dst_peer_drops);
                    // `debug!` not `warn!` — per-packet, would flood the log.
                    tracing::debug!(%ipv4_addr, "policy route closed: dropping packet");
                    return (dst_peers, false);
                }
                RouteLookup::Miss => {
                    if let Some(peer_id) = self.resolve_exit_node_peer().await {
                        dst_peers.push(peer_id);
                        is_exit_node = true;
                    } else if let Some(peer_id) = self
                        .route
                        .get_peer_id_for_default_route_proxy(&IpAddr::V4(*ipv4_addr))
                        .await
                    {
                        dst_peers.push(peer_id);
                    }
                }
            }
        }
        if self.host_routing.local_exit_node_fallback
            && dst_peers.is_empty()
            && !self
                .context
                .is_ip_in_same_network(&std::net::IpAddr::V4(*ipv4_addr))
        {
            tracing::trace!(
                %ipv4_addr,
                "no peer route for external IPv4; use local exit-node fallback"
            );
            dst_peers.push(self.my_peer_id);
            is_exit_node = true;
        }
        (dst_peers, is_exit_node)
    }

    pub async fn get_msg_dst_peer_ipv6(
        &self,
        ipv6_addr: &Ipv6Addr,
    ) -> (SmallVec<[PeerId; 1]>, bool) {
        let mut is_exit_node = false;
        let mut dst_peers = SmallVec::<[PeerId; 1]>::new();
        if self.is_all_peers_broadcast_ipv6(ipv6_addr) {
            dst_peers.extend(self.peers.list_routes().await.iter().map(|x| *x.key()));
        } else if let Some(peer_id) = self.peers.get_peer_id_by_ipv6(ipv6_addr).await {
            dst_peers.push(peer_id);
        } else if !ipv6_addr.is_unicast_link_local() {
            // NOTE: never route link local address to exit node.
            if let Some(peer_id) = self.resolve_exit_node_peer().await {
                dst_peers.push(peer_id);
                is_exit_node = true;
            } else if let Some(peer_id) = self.route.get_public_ipv6_gateway_peer_id().await {
                dst_peers.push(peer_id);
            } else if let Some(peer_id) = self
                .route
                .get_peer_id_for_default_route_proxy(&IpAddr::V6(*ipv6_addr))
                .await
            {
                dst_peers.push(peer_id);
            }
        }

        (dst_peers, is_exit_node)
    }

    pub async fn send_msg_by_ip(
        &self,
        mut msg: ZCPacket,
        ip_addr: IpAddr,
        not_send_to_self: bool,
    ) -> Result<(), Error> {
        msg.fill_peer_manager_hdr(self.my_peer_id, 0, PacketType::Data as u8);
        if !self.run_nic_packet_process_pipeline(&mut msg).await {
            return Ok(());
        }
        self.send_msg_by_ip_after_pipeline(msg, ip_addr, not_send_to_self, true)
            .await
            .map(|_| ())
    }

    /// Display-only sibling of [`Self::send_msg_by_ip`] for VIP reachability
    /// probes.
    ///
    /// Same route lookup and same tunnel send, but deliberately **not** the
    /// user-traffic path: no `mark_recent_traffic` (that is the P2P / lazy-p2p
    /// control-plane signal), no `self_tx_packets` / `compress_tx_bytes`
    /// accounting, no parking of unroutable packets, and no ACL pipeline.
    ///
    /// The roadmap contract (§2.4 / §3.3) says a probe is "display only": it must
    /// not influence routing, must not count as user traffic, and must not be
    /// buffered past its own lifetime. Keeping a separate entry point is how that
    /// stays obvious at the call site instead of being a boolean argument nobody
    /// remembers.
    pub async fn send_probe_msg_by_ip(
        &self,
        mut msg: ZCPacket,
        ip_addr: IpAddr,
        not_send_to_self: bool,
    ) -> Result<(), Error> {
        msg.fill_peer_manager_hdr(self.my_peer_id, 0, PacketType::Data as u8);

        // Unlike the user path there is no NIC pipeline here — no ACL, no DNS
        // interception, no proxy filters. Those all act on *host* traffic; a
        // prober packet is not, and running it through them would either drop it
        // for unrelated reasons or make proxying side effects visible.

        let flow_key = msg.bond_flow_key();
        let (dst_peers, is_exit_node) = match ip_addr {
            IpAddr::V4(ipv4_addr) => self.get_msg_dst_peer_ipv4(&ipv4_addr, flow_key).await,
            IpAddr::V6(ipv6_addr) => self.get_msg_dst_peer_ipv6(&ipv6_addr).await,
        };
        if dst_peers.is_empty() {
            // Not parked and not counted: the prober turns this into
            // `send_failed`, and the packet must never outlive the probe.
            return Err(Error::RouteError(None));
        }

        try_compress_and_encrypt(
            self.data_compress_algo,
            &self.encryptor,
            &mut msg,
            self.is_secure_mode_enabled,
        )
        .await?;

        let packet_policy = self.context.packet_policy();
        msg.mut_peer_manager_header()
            .unwrap()
            .set_latency_first(packet_policy.latency_first)
            .set_exit_node(is_exit_node);

        let mut errs: Vec<Error> = Vec::new();
        let mut msg = Some(msg);
        let total = dst_peers.len();
        for (i, peer_id) in dst_peers.iter().enumerate() {
            let mut msg = if i == total - 1 {
                msg.take().expect("last packet")
            } else {
                msg.clone().expect("fanout packet")
            };
            let hdr = msg.mut_peer_manager_header().unwrap();
            hdr.to_peer_id.set(*peer_id);
            if needs_self_delivery_guard(
                self.host_routing.local_exit_node_fallback,
                not_send_to_self,
                *peer_id,
                self.my_peer_id,
                self.context.is_ip_local_virtual_ip(&ip_addr),
            ) {
                hdr.set_not_send_to_tun(true);
                hdr.set_no_proxy(true);
            }

            // `direct_tx_metrics: None` — probe bytes are not user traffic.
            if let Err(e) = send_msg_internal(
                self.peers.as_ref(),
                &self.foreign_network_client,
                &self.relay_peer_map,
                None,
                msg,
                *peer_id,
            )
            .await
            {
                note_tunnel_buffer_full(&self.counters.tunnel_buffer_full, &e);
                errs.push(e);
            }
        }

        if errs.is_empty() {
            Ok(())
        } else {
            Err(Error::RouteError(None))
        }
    }

    /// Everything [`Self::send_msg_by_ip`] does once the NIC packet pipeline has
    /// run.
    ///
    /// `allow_buffer` is `false` for the pending-packet flusher: those packets
    /// already went through the pipeline, so re-running it would double-apply
    /// ACL / DNS filters, and re-buffering a failure would extend the packet's
    /// life past its TTL. The flusher also needs [`SendByIpStatus::StillUnroutable`]
    /// (no drop counted) so it can `put_back` without a separate pre-lookup.
    async fn send_msg_by_ip_after_pipeline(
        &self,
        mut msg: ZCPacket,
        ip_addr: IpAddr,
        not_send_to_self: bool,
        allow_buffer: bool,
    ) -> Result<SendByIpStatus, Error> {
        tracing::trace!(
            "do send_msg in peer manager, msg: {:?}, ip_addr: {}",
            msg,
            ip_addr
        );

        // Sticky key from plaintext IP before compress/encrypt (classic AEAD
        // would otherwise make per-packet ciphertext hash → bond spray).
        msg.set_bond_flow_key(flow_key_from_payload(msg.payload()));
        // Attach the inner destination IP (and its length) for the per-tunnel
        // flow accounting. This must happen *before* the payload is compressed
        // and encrypted, afterwards the inner IP header is unreadable.
        if self.flow_table.is_enabled() {
            msg.set_flow_meta(crate::packet::FlowMeta {
                dst_ip: ip_addr,
                inner_len: msg.payload().len() as u32,
            });
        }
        let packet_policy = self.context.packet_policy();
        let cur_to_peer_id = msg.peer_manager_header().unwrap().to_peer_id.into();
        if cur_to_peer_id != 0 {
            self.mark_recent_traffic_with_policy(cur_to_peer_id, packet_policy);
            let result = send_msg_internal(
                self.peers.as_ref(),
                &self.foreign_network_client,
                &self.relay_peer_map,
                Some(&self.traffic_metrics),
                msg,
                cur_to_peer_id,
            )
            .await;
            if let Err(ref e) = result {
                note_tunnel_buffer_full(&self.counters.tunnel_buffer_full, e);
            }
            return result.map(|_| SendByIpStatus::Done);
        }

        // P5: reuse the sticky flow key computed above so `select_by = balance`
        // rules spread whole flows (one stream → one egress) rather than packets.
        let flow_key = msg.bond_flow_key();
        let (dst_peers, is_exit_node) = match ip_addr {
            IpAddr::V4(ipv4_addr) => self.get_msg_dst_peer_ipv4(&ipv4_addr, flow_key).await,
            IpAddr::V6(ipv6_addr) => self.get_msg_dst_peer_ipv6(&ipv6_addr).await,
        };

        if dst_peers.is_empty() {
            if allow_buffer && self.try_park_unroutable(&msg, ip_addr, not_send_to_self) {
                return Ok(SendByIpStatus::Done);
            }
            if !allow_buffer {
                // Flusher owns the packet: do not count a drop here — return it
                // so the caller can put_back until the original TTL.
                return Ok(SendByIpStatus::StillUnroutable(msg));
            }
            self.counters.no_dst_peer_drops.inc();
            // `debug!` not `warn!` — per-packet, would flood the log. The
            // `data_plane_no_dst_peer_drops` counter is the signal (P-AUDIT.2).
            tracing::debug!(%ip_addr, "no peer id for ip; dropping packet");
            return Ok(SendByIpStatus::Done);
        }

        self.counters
            .compress_tx_bytes_before
            .add(msg.buf_len() as u64);

        try_compress_and_encrypt(
            self.data_compress_algo,
            &self.encryptor,
            &mut msg,
            self.is_secure_mode_enabled,
        )
        .await?;

        self.counters
            .compress_tx_bytes_after
            .add(msg.buf_len() as u64);

        msg.mut_peer_manager_header()
            .unwrap()
            .set_latency_first(packet_policy.latency_first)
            .set_exit_node(is_exit_node);

        let mut errs: Vec<Error> = vec![];
        let mut msg = Some(msg);
        let total_dst_peers = dst_peers.len();
        let should_mark_recent_traffic = should_mark_recent_traffic_for_fanout(total_dst_peers);
        for (i, peer_id) in dst_peers.iter().enumerate() {
            if should_mark_recent_traffic {
                self.mark_recent_traffic_with_policy(*peer_id, packet_policy);
            }
            if let Err(e) = self.check_p2p_only_before_send(*peer_id, packet_policy) {
                errs.push(e);
                continue;
            }

            let mut msg = if i == total_dst_peers - 1 {
                msg.take().unwrap()
            } else {
                msg.clone().unwrap()
            };

            let hdr = msg.mut_peer_manager_header().unwrap();
            hdr.to_peer_id.set(*peer_id);

            if needs_self_delivery_guard(
                self.host_routing.local_exit_node_fallback,
                not_send_to_self,
                *peer_id,
                self.my_peer_id,
                self.context.is_ip_local_virtual_ip(&ip_addr),
            ) {
                // Keep the loop-prevention flags for proxy-induced self-delivery where
                // the destination is not this node's own EasyTier-managed IP.
                hdr.set_not_send_to_tun(true);
                hdr.set_no_proxy(true);
            }

            self.counters.self_tx_bytes.add(msg.buf_len() as u64);
            self.counters.self_tx_packets.inc();

            if let Err(e) = send_msg_internal(
                self.peers.as_ref(),
                &self.foreign_network_client,
                &self.relay_peer_map,
                Some(&self.traffic_metrics),
                msg,
                *peer_id,
            )
            .await
            {
                note_tunnel_buffer_full(&self.counters.tunnel_buffer_full, &e);
                errs.push(e);
            }
        }

        tracing::trace!(?dst_peers, "do send_msg in peer manager done");

        if errs.is_empty() {
            Ok(SendByIpStatus::Done)
        } else {
            tracing::error!(?errs, "send_msg has error");
            Err(anyhow::anyhow!("send_msg has error: {:?}", errs).into())
        }
    }

    /// Park a packet whose destination has no route yet (P-AUDIT.12).
    ///
    /// Returns `false` when the caller must fall back to a counted drop.
    /// Uses [`PendingPackets::push_with`] so a full queue never pays for a
    /// `ZCPacket` deep copy that would then be discarded.
    fn try_park_unroutable(&self, msg: &ZCPacket, ip_addr: IpAddr, not_send_to_self: bool) -> bool {
        // Policy `Block` is a deliberate "drop rather than leak" decision.
        // Parking it would soften that boundary, and re-delivering later could
        // emit traffic the policy forbade at capture time.
        if let IpAddr::V4(ip) = ip_addr
            && matches!(
                self.local_route_table.lookup(&ip, msg.bond_flow_key()),
                RouteLookup::Block
            )
        {
            return false;
        }

        // An empty fanout for broadcast / multicast means "nobody to send to",
        // not "route missing" — parking those is pure waste.
        match ip_addr {
            IpAddr::V4(ip) if self.is_all_peers_broadcast_ipv4(&ip) => return false,
            IpAddr::V6(ip) if self.is_all_peers_broadcast_ipv6(&ip) => return false,
            _ => {}
        }

        let expires_at = Instant::now() + PENDING_TTL;
        let outcome = self
            .pending_packets
            .push_with(ip_addr, expires_at, not_send_to_self, || msg.clone());
        if outcome.dropped > 0 {
            self.counters.no_dst_peer_drops.add(outcome.dropped as u64);
        }
        if outcome.accepted {
            self.counters.no_dst_peer_buffered.inc();
            true
        } else {
            false
        }
    }

    /// One flush pass over the parked packets (P-AUDIT.12).
    ///
    /// Retries everything still inside its TTL, re-parks what is *still*
    /// unroutable, and counts what finally expired as a real drop. Driven by
    /// [`PeerManagerCore::start_pending_packet_flush`].
    ///
    /// Uses a single route lookup inside `send_msg_by_ip_after_pipeline`
    /// (`StillUnroutable` → put_back) instead of a pre-check + send.
    async fn flush_pending_once(&self) {
        let taken = self.pending_packets.take_all(Instant::now());
        if taken.expired > 0 {
            self.counters.no_dst_peer_drops.add(taken.expired as u64);
        }

        for parked in taken.ready {
            let dst = parked.dst;
            let expires_at = parked.expires_at;
            let not_send_to_self = parked.not_send_to_self;
            match self
                .send_msg_by_ip_after_pipeline(parked.packet, dst, not_send_to_self, false)
                .await
            {
                Ok(SendByIpStatus::Done) => {}
                Ok(SendByIpStatus::StillUnroutable(packet)) => {
                    if !self.pending_packets.put_back(Parked {
                        dst,
                        packet,
                        expires_at,
                        not_send_to_self,
                    }) {
                        self.counters.no_dst_peer_drops.inc();
                    }
                }
                Err(error) => {
                    // Compress/encrypt failed, or every peer in the fanout
                    // errored. Neither is transient-with-a-route-pending, so
                    // re-parking would just burn the TTL and retry a failure.
                    // Counted separately from `no_dst_peer_drops` so the
                    // "no route" signal the buffering feature is judged by
                    // stays clean.
                    self.counters.pending_flush_errors.inc();
                    tracing::debug!(%dst, %error, "pending packet flush failed");
                }
            }
        }
    }
}

/// Outcome of [`PeerOutboundPacketRouter::send_msg_by_ip_after_pipeline`].
enum SendByIpStatus {
    /// Sent, buffered, or counted as a drop — caller is done with the packet.
    Done,
    /// No route and buffering was not allowed; drop was **not** counted. The
    /// packet is returned so the flusher can put it back until its original TTL.
    StillUnroutable(ZCPacket),
}

/// How long the flusher waits before re-testing a parked packet's route.
///
/// Small enough to be invisible next to the route-sync round trip it is
/// waiting on, large enough that a parked packet does not spin the loop.
const PENDING_FLUSH_INTERVAL: Duration = Duration::from_millis(20);

struct PeerPacketRouterCounters {
    self_tx_packets: CounterHandle,
    self_tx_bytes: CounterHandle,
    self_rx_packets: CounterHandle,
    self_rx_bytes: CounterHandle,
    forward_data_tx_packets: CounterHandle,
    forward_data_tx_bytes: CounterHandle,
    forward_control_tx_packets: CounterHandle,
    forward_control_tx_bytes: CounterHandle,
    compress_tx_bytes_before: CounterHandle,
    compress_tx_bytes_after: CounterHandle,
    compress_rx_bytes_before: CounterHandle,
    compress_rx_bytes_after: CounterHandle,
    /// Outbound tunnel send hit `BufferFull` (ring full / silent loss path).
    /// Covers the relay-forwarding leg too — this is where a relay node drops.
    tunnel_buffer_full: CounterHandle,
}

pub(crate) struct PeerPacketRouter {
    packet_recv: PacketRecvChanReceiver,
    my_peer_id: PeerId,
    peers: Arc<PeerMap>,
    peer_packet_process_pipeline: PeerPacketPipeline,
    foreign_client: Arc<ForeignNetworkClient>,
    relay_peer_map: Arc<RelayPeerMap>,
    foreign_network_manager: Arc<ForeignNetworkManager>,
    encryptor: Arc<dyn Encryptor>,
    compress_algo: CompressorAlgo,
    acl_filter: Arc<AclFilter>,
    context: ArcPeerContext,
    secure_mode_enabled: bool,
    route: ArcRoute,
    is_credential_node: bool,
    traffic_metrics: Arc<TrafficMetricRecorder>,
    stats_mgr: Arc<StatsManager>,
    counters: PeerPacketRouterCounters,
    /// Per-tunnel IP flow accounting (disabled by default).
    flow_table: Arc<FlowTable>,
}

impl PeerPacketRouter {
    #[allow(clippy::too_many_arguments)]
    fn new(
        packet_recv: PacketRecvChanReceiver,
        my_peer_id: PeerId,
        peers: Arc<PeerMap>,
        peer_packet_process_pipeline: PeerPacketPipeline,
        foreign_client: Arc<ForeignNetworkClient>,
        relay_peer_map: Arc<RelayPeerMap>,
        foreign_network_manager: Arc<ForeignNetworkManager>,
        encryptor: Arc<dyn Encryptor>,
        compress_algo: CompressorAlgo,
        acl_filter: Arc<AclFilter>,
        context: ArcPeerContext,
        secure_mode_enabled: bool,
        route: ArcRoute,
        is_credential_node: bool,
        traffic_metrics: Arc<TrafficMetricRecorder>,
        stats_mgr: Arc<StatsManager>,
        network_name: String,
        self_tx_packets: CounterHandle,
        self_tx_bytes: CounterHandle,
        compress_tx_bytes_before: CounterHandle,
        compress_tx_bytes_after: CounterHandle,
        flow_table: Arc<FlowTable>,
    ) -> Self {
        let label_set = LabelSet::new().with_label_type(LabelType::NetworkName(network_name));
        Self {
            packet_recv,
            my_peer_id,
            peers,
            peer_packet_process_pipeline,
            foreign_client,
            relay_peer_map,
            foreign_network_manager,
            encryptor,
            compress_algo,
            acl_filter,
            context,
            secure_mode_enabled,
            route,
            is_credential_node,
            traffic_metrics,
            stats_mgr: stats_mgr.clone(),
            counters: PeerPacketRouterCounters {
                self_tx_packets,
                self_tx_bytes,
                self_rx_bytes: stats_mgr
                    .get_counter(MetricName::TrafficBytesSelfRx, label_set.clone()),
                self_rx_packets: stats_mgr
                    .get_counter(MetricName::TrafficPacketsSelfRx, label_set.clone()),
                forward_data_tx_bytes: stats_mgr
                    .get_counter(MetricName::TrafficBytesForwarded, label_set.clone()),
                forward_data_tx_packets: stats_mgr
                    .get_counter(MetricName::TrafficPacketsForwarded, label_set.clone()),
                forward_control_tx_bytes: stats_mgr
                    .get_counter(MetricName::TrafficControlBytesForwarded, label_set.clone()),
                forward_control_tx_packets: stats_mgr.get_counter(
                    MetricName::TrafficControlPacketsForwarded,
                    label_set.clone(),
                ),
                compress_tx_bytes_before,
                compress_tx_bytes_after,
                compress_rx_bytes_before: stats_mgr
                    .get_counter(MetricName::CompressionBytesRxBefore, label_set.clone()),
                compress_rx_bytes_after: stats_mgr
                    .get_counter(MetricName::CompressionBytesRxAfter, label_set.clone()),
                tunnel_buffer_full: stats_mgr
                    .get_counter(MetricName::DataPlaneTunnelBufferFull, label_set),
            },
            flow_table,
        }
    }

    /// Book a received packet on the tunnel it arrived on.
    ///
    /// The payload is plaintext at this point, so the source IP of the inner
    /// packet is available. Packets injected locally and packets that are
    /// only forwarded (they are never decrypted here) are not counted.
    fn record_rx_flow(&self, ingress: &PeerPacketIngress, packet_type: u8, packet: &ZCPacket) {
        if !self.flow_table.is_enabled() {
            return;
        }
        // 只记 IP 数据包：KCP/QUIC 等传输层载荷是裸数据报，首字节任意，
        // parse_inner_ips 可能把随机字节误判成 IPv4/IPv6 头（0x4X/0x6X），
        // 产生垃圾流条目污染 flow 表。TX 侧仅对有 flow_meta 的 IP 包记账，
        // 这里对称：只对 PacketType::Data 记 RX 流。
        if packet_type != PacketType::Data as u8 {
            return;
        }
        let PeerPacketIngress::Peer {
            peer_id, conn_id, ..
        } = *ingress
        else {
            return;
        };
        self.flow_table.record_rx(
            Some((conn_id, peer_id)),
            packet.payload(),
            crate::peers::flow_table::now_unix_millis(),
        );
    }

    pub async fn run(mut self) {
        tracing::trace!("start_peer_recv");
        loop {
            // CRITICAL (CR-15): this used to be
            // `while let Ok(..) = .. { .. }` followed by `panic!("done_peer_recv")`.
            // Under `panic = "abort"` an ordinary shutdown therefore aborted the
            // whole process. Exit quietly instead.
            let Some(envelope) = recv_packet_envelope_from_chan(&mut self.packet_recv).await else {
                tracing::debug!("peer recv channel closed, stopping ingress loop");
                break;
            };

            // Avoid-only relay policy: a node that disallows relay is excluded
            // at route-computation time (AVOID_RELAY_COST). Packets that still
            // arrive for forwarding (stale routes, convergence windows,
            // sole-path topologies) are forwarded, never hard-dropped — drops
            // turn transient misroutes into severe loss.
            // The ingress carries the tunnel the packet arrived on; it is
            // needed to attribute the traffic to that tunnel (flow accounting).
            let (ret, ingress) = envelope.into_parts();
            let Err(ret) = try_handle_foreign_network_packet(
                ret,
                self.my_peer_id,
                &self.peers,
                &self.foreign_network_manager,
                self.stats_mgr.as_ref(),
            )
            .await
            else {
                continue;
            };

            self.handle_packet(ret, ingress).await;
        }
    }

    async fn handle_packet(&self, mut ret: ZCPacket, ingress: PeerPacketIngress) {
        let buf_len = ret.buf_len();
        let Some(hdr) = ret.mut_peer_manager_header() else {
            tracing::warn!(?ret, "invalid packet, skip");
            return;
        };

        tracing::trace!(?hdr, "peer recv a packet...");
        let from_peer_id = hdr.from_peer_id.get();
        let to_peer_id = hdr.to_peer_id.get();
        let packet_type = hdr.packet_type;
        let is_encrypted = hdr.is_encrypted();
        if to_peer_id != self.my_peer_id {
            if hdr.forward_counter > 7 {
                tracing::warn!(?hdr, "forward counter exceed, drop packet");
                return;
            }

            // Step 10b: credential nodes don't forward handshake packets
            if self.is_credential_node
                && (packet_type == PacketType::HandShake as u8
                    || packet_type == PacketType::NoiseHandshakeMsg1 as u8
                    || packet_type == PacketType::NoiseHandshakeMsg2 as u8
                    || packet_type == PacketType::NoiseHandshakeMsg3 as u8)
            {
                tracing::debug!("credential node dropping forwarded handshake packet");
                return;
            }

            if hdr.forward_counter > 2 && hdr.is_latency_first() {
                tracing::trace!(?hdr, "set_latency_first false because too many hop");
                hdr.set_latency_first(false);
            }

            hdr.forward_counter += 1;

            if from_peer_id == self.my_peer_id {
                self.counters.compress_tx_bytes_before.add(buf_len as u64);

                if packet_type == PacketType::Data as u8
                    || packet_type == PacketType::KcpSrc as u8
                    || packet_type == PacketType::KcpDst as u8
                {
                    if ret.bond_flow_key().is_none() {
                        ret.set_bond_flow_key(flow_key_from_payload(ret.payload()));
                    }
                    let _ = try_compress_and_encrypt(
                        self.compress_algo,
                        &self.encryptor,
                        &mut ret,
                        self.secure_mode_enabled,
                    )
                    .await;
                }

                self.counters
                    .compress_tx_bytes_after
                    .add(ret.buf_len() as u64);
                self.counters.self_tx_bytes.add(ret.buf_len() as u64);
                self.counters.self_tx_packets.inc();
            } else {
                match traffic_kind(packet_type) {
                    TrafficKind::Data => {
                        self.counters.forward_data_tx_bytes.add(buf_len as u64);
                        self.counters.forward_data_tx_packets.inc();
                    }
                    TrafficKind::Control => {
                        self.counters.forward_control_tx_bytes.add(buf_len as u64);
                        self.counters.forward_control_tx_packets.inc();
                    }
                }
            }

            tracing::trace!(?to_peer_id, my_peer_id = ?self.my_peer_id, "need forward");
            let tx_metrics = if from_peer_id == self.my_peer_id {
                Some(&self.traffic_metrics)
            } else {
                None
            };
            let ret = send_msg_internal(
                self.peers.as_ref(),
                &self.foreign_client,
                &self.relay_peer_map,
                tx_metrics,
                ret,
                to_peer_id,
            )
            .await;
            if let Err(ref e) = ret {
                // Count the silent-loss path on the forwarded leg too: for a
                // relay node this is the only place BufferFull shows up.
                note_tunnel_buffer_full(&self.counters.tunnel_buffer_full, e);
                if e.is_expected_path_unavailable() {
                    if take_path_unavailable_warn_token(to_peer_id) {
                        tracing::warn!(?ret, ?to_peer_id, ?from_peer_id, "forward packet error");
                    } else {
                        tracing::debug!(?ret, ?to_peer_id, ?from_peer_id, "forward packet error");
                    }
                } else {
                    tracing::error!(?ret, ?to_peer_id, ?from_peer_id, "forward packet error");
                }
            }
        } else {
            if packet_type == PacketType::RelayHandshake as u8
                || packet_type == PacketType::RelayHandshakeAck as u8
            {
                let _ = self.relay_peer_map.handle_handshake_packet(ret).await;
                return;
            }
            if !self.secure_mode_enabled {
                if let Err(e) = self.encryptor.decrypt(&mut ret) {
                    tracing::error!(?e, "decrypt failed");
                    return;
                }
            } else if is_encrypted {
                match self.relay_peer_map.decrypt_if_needed(&mut ret).await {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::error!("secure session not found");
                        return;
                    }
                    Err(e) => {
                        tracing::error!(?e, "secure decrypt failed");
                        return;
                    }
                }
            }

            self.counters.self_rx_bytes.add(buf_len as u64);
            self.counters.self_rx_packets.inc();
            self.traffic_metrics
                .record_rx(from_peer_id, packet_type, buf_len as u64)
                .await;
            self.counters.compress_rx_bytes_before.add(buf_len as u64);

            let compressor = DefaultCompressor {};
            if let Err(e) = compressor.decompress(&mut ret).await {
                tracing::error!(?e, "decompress failed");
                return;
            }

            self.counters
                .compress_rx_bytes_after
                .add(ret.buf_len() as u64);

            // Per-tunnel flow accounting (RX): the payload is plaintext here,
            // so the source IP of the inner packet is available. Only packets
            // that arrived on a peer connection are attributed; forwarded
            // packets are not decrypted and therefore not counted.
            self.record_rx_flow(&ingress, packet_type, &ret);

            if !self.acl_filter.process_packet_with_acl(
                &ret,
                true,
                self.context.ipv4().map(|x| x.address()),
                |dst| self.context.is_ip_local_ipv6(&dst),
                self.route.as_ref(),
            ) {
                return;
            }

            let mut processed = false;
            let mut zc_packet = Some(ret);
            tracing::trace!(?zc_packet, "try_process_packet_from_peer");
            let pipelines = self.peer_packet_process_pipeline.load_full();
            for pipeline in pipelines.iter().rev() {
                if let Some(filter) = pipeline.filter_if_active() {
                    zc_packet = filter
                        .try_process_packet_from_peer(zc_packet.unwrap())
                        .await;
                }
                if zc_packet.is_none() {
                    processed = true;
                    break;
                }
            }
            if !processed {
                tracing::error!(?zc_packet, "unhandled packet");
            }
        }
    }
}

pub(crate) async fn try_handle_foreign_network_packet(
    mut packet: ZCPacket,
    my_peer_id: PeerId,
    peer_map: &PeerMap,
    foreign_network_manager: &ForeignNetworkManager,
    stats_manager: &StatsManager,
) -> Result<(), ZCPacket> {
    let pm_header = packet.peer_manager_header().unwrap();
    if pm_header.packet_type != PacketType::ForeignNetworkPacket as u8 {
        return Err(packet);
    }

    let from_peer_id = pm_header.from_peer_id.get();
    let to_peer_id = pm_header.to_peer_id.get();

    let foreign_hdr = packet.foreign_network_hdr().unwrap();
    let foreign_network_name = foreign_hdr.get_network_name(packet.payload());
    let foreign_peer_id = foreign_hdr.get_dst_peer_id();

    let foreign_network_my_peer_id =
        foreign_network_manager.get_network_peer_id(&foreign_network_name);

    let buf_len = packet.buf_len();
    let label_set =
        LabelSet::new().with_label_type(LabelType::NetworkName(foreign_network_name.clone()));
    let add_counter = move |bytes_metric, packets_metric| {
        stats_manager
            .get_counter(bytes_metric, label_set.clone())
            .add(buf_len as u64);
        stats_manager.get_counter(packets_metric, label_set).inc();
    };

    // NOTICE: the to peer id is modified by the src from foreign network my peer id to the origin my peer id
    if to_peer_id == my_peer_id {
        // packet sent from other peer to me, extract the inner packet and forward it
        add_counter(
            MetricName::TrafficBytesForeignForwardRx,
            MetricName::TrafficPacketsForeignForwardRx,
        );
        if let Err(e) = foreign_network_manager
            .forward_foreign_network_packet(
                &foreign_network_name,
                foreign_peer_id,
                packet.foreign_network_packet(),
            )
            .await
        {
            tracing::debug!(
                ?e,
                ?foreign_network_name,
                ?foreign_peer_id,
                "foreign network mgr send_msg_to_peer failed"
            );
        }
        Ok(())
    } else if Some(from_peer_id) == foreign_network_my_peer_id {
        // to_peer_id is my peer id for the foreign network, need to convert to the origin my_peer_id of dst
        let Some(to_peer_id) = peer_map
            .get_origin_my_peer_id(&foreign_network_name, to_peer_id)
            .await
        else {
            tracing::debug!(
                ?foreign_network_name,
                ?to_peer_id,
                "cannot find origin my peer id for foreign network."
            );
            return Err(packet);
        };

        add_counter(
            MetricName::TrafficBytesForeignForwardTx,
            MetricName::TrafficPacketsForeignForwardTx,
        );

        // modify the to_peer id from foreign network my peer id to the origin my peer id
        packet
            .mut_peer_manager_header()
            .unwrap()
            .to_peer_id
            .set(to_peer_id);

        // packet is generated from foreign network mgr and should be forward to other peer
        if let Err(e) = peer_map
            .send_msg(packet, to_peer_id, NextHopPolicy::LeastHop)
            .await
        {
            tracing::debug!(
                ?e,
                ?to_peer_id,
                "send_msg_directly failed when forward local generated foreign network packet"
            );
        }
        Ok(())
    } else {
        // target is not me, forward it. try get origin peer id
        add_counter(
            MetricName::TrafficBytesForeignForwardForwarded,
            MetricName::TrafficPacketsForeignForwardForwarded,
        );
        Err(packet)
    }
}

struct PeerManagerRouteInterface {
    my_peer_id: PeerId,
    peers: Weak<PeerMap>,
    foreign_network_client: Weak<ForeignNetworkClient>,
    foreign_network_manager: Weak<ForeignNetworkManager>,
}

#[async_trait::async_trait]
impl RouteInterface for PeerManagerRouteInterface {
    async fn list_peers(&self) -> Vec<PeerId> {
        let Some(foreign_client) = self.foreign_network_client.upgrade() else {
            return vec![];
        };

        let Some(peer_map) = self.peers.upgrade() else {
            return vec![];
        };

        let mut peers = foreign_client.list_public_peers().await;
        peers.extend(peer_map.list_peers_with_conn().await);
        peers
    }

    async fn list_peers_without_usable_conn(&self) -> Vec<PeerId> {
        let Some(peer_map) = self.peers.upgrade() else {
            return vec![];
        };

        peer_map.list_peers_without_usable_conn().await
    }

    fn my_peer_id(&self) -> PeerId {
        self.my_peer_id
    }

    async fn close_peer(&self, peer_id: PeerId) {
        if let Some(peer_map) = self.peers.upgrade() {
            let _ = peer_map.close_peer(peer_id).await;
        }

        if let Some(foreign_client) = self.foreign_network_client.upgrade() {
            let _ = foreign_client.get_peer_map().close_peer(peer_id).await;
        }
    }

    async fn get_peer_public_key(&self, peer_id: PeerId) -> Option<Vec<u8>> {
        let peer_map = self.peers.upgrade()?;
        peer_map.get_peer_public_key(peer_id)
    }

    async fn get_peer_identity_type(&self, peer_id: PeerId) -> Option<PeerIdentityType> {
        let peer_map = self.peers.upgrade()?;
        peer_map.get_peer_identity_type(peer_id)
    }

    async fn list_foreign_networks(&self) -> ForeignNetworkRouteInfoMap {
        let ret = ForeignNetworkRouteInfoMap::new();
        let Some(foreign_network_manager) = self.foreign_network_manager.upgrade() else {
            return ret;
        };

        let networks = foreign_network_manager
            .list_foreign_network_route_infos()
            .await;
        for info in networks {
            if info.peer_ids.is_empty() {
                continue;
            }

            let last_update = foreign_network_manager
                .get_foreign_network_last_update(&info.network_name)
                .unwrap_or(SystemTime::now());
            ret.insert(
                ForeignNetworkRouteInfoKey {
                    peer_id: self.my_peer_id,
                    network_name: info.network_name,
                },
                ForeignNetworkRouteInfoEntry {
                    foreign_peer_ids: info.peer_ids,
                    last_update: Some(last_update.into()),
                    version: 0,
                    network_secret_digest: info.network_secret_digest,
                    my_peer_id_for_this_network: info.my_peer_id_for_this_network,
                },
            );
        }
        ret
    }
}

pub(crate) async fn send_msg_internal(
    peers: &PeerMap,
    foreign_network_client: &Arc<ForeignNetworkClient>,
    relay_peer_map: &Arc<RelayPeerMap>,
    direct_tx_metrics: Option<&Arc<TrafficMetricRecorder>>,
    msg: ZCPacket,
    dst_peer_id: PeerId,
) -> Result<(), Error> {
    let policy = get_next_hop_policy(msg.peer_manager_header().unwrap().is_latency_first());
    let is_latency_first = msg.peer_manager_header().unwrap().is_latency_first();
    let packet_type = msg.peer_manager_header().unwrap().packet_type;
    let msg_len = msg.buf_len() as u64;
    let latency_first_gateway = if is_latency_first {
        peers
            .get_gateway_peer_id(dst_peer_id, policy.clone())
            .await
            .filter(|gateway| *gateway != dst_peer_id)
    } else {
        None
    };
    let send_result = if let Some(gateway) = latency_first_gateway
        && (peers.has_peer_with_live_conn(gateway) || foreign_network_client.has_next_hop(gateway))
    {
        relay_peer_map.send_msg(msg, dst_peer_id, policy).await
    } else if let Some(peer) = peers.get_peer_by_id(dst_peer_id)
        && peer.has_live_conns()
    {
        peer.send_msg(msg).await
    } else if peers.is_self(dst_peer_id) {
        peers.send_msg_directly(msg, dst_peer_id).await
    } else if foreign_network_client.has_next_hop(dst_peer_id) {
        foreign_network_client.send_msg(msg, dst_peer_id).await
    } else if let Some(gateway) = peers.get_gateway_peer_id(dst_peer_id, policy.clone()).await {
        if peers.has_peer_with_live_conn(gateway) || foreign_network_client.has_next_hop(gateway) {
            relay_peer_map.send_msg(msg, dst_peer_id, policy).await
        } else {
            tracing::warn!(
                ?gateway,
                ?dst_peer_id,
                "cannot send msg to peer through gateway"
            );
            Err(Error::RouteError(None))
        }
    } else if foreign_network_client.has_next_hop(dst_peer_id) {
        // check foreign network again. so in happy path we can avoid extra check
        foreign_network_client.send_msg(msg, dst_peer_id).await
    } else {
        tracing::debug!(?dst_peer_id, "no gateway for peer");
        Err(Error::RouteError(None))
    };

    if send_result.is_ok()
        && let Some(metrics) = direct_tx_metrics
    {
        metrics.record_tx(dst_peer_id, packet_type, msg_len).await;
    }

    send_result
}

#[cfg(test)]
#[path = "peer_manager_tests.rs"]
mod tests;
