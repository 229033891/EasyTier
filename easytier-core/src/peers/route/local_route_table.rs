//! Local policy route table (unified routing P1 / P2 / P3).
//!
//! Hot path: sync `lookup` — ArcSwap load + `by_ip` / bucketed LPM + read
//! `active_peer`. No locks, no alloc, no async.
//!
//! Rules may carry `cidr`, `domains`, or both. Domain rules learn their IPs
//! from observed MagicDNS answers (`observe_ips`, P3): the rule set is
//! authoritative from local config, the IP set is observation.
//!
//! TOML key is `[[policy_routes]]` (the existing `routes = [CIDR…]` key stays
//! the manual proxy list).
//!
//! The L2 proxy-CIDR reconciler reads `l2_contributing_cidrs()` from the same
//! `ArcSwap` snapshot, so both planes derive from one source of truth.
//!
//! `revision` bumps whenever the published snapshot changes (including DNS
//! `observe_ips`). The L2 monitor must watch this — peer-info timestamps alone
//! do not move when only observed `/32`s change.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    net::{IpAddr, Ipv4Addr},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use arc_swap::ArcSwap;
use cidr::Ipv4Cidr;
use serde::{Deserialize, Serialize};

use crate::config::{DnsConfig, PeerId};
use crate::peers::conn::conn_select::ConnSelectConfig;

pub type RuleIdx = usize;

/// One explicit CIDR in the LPM index (P6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CidrEntry {
    /// Network address as `u32`; ordering matches `Ipv4Addr` ordering.
    pub network: u32,
    pub cidr: Ipv4Cidr,
    pub idx: RuleIdx,
}

pub const MAX_DOMAINS_PER_RULE: usize = 64;
pub const MAX_PEERS_PER_RULE: usize = 16;
pub const MAX_POLICY_ROUTES: usize = 1024;
/// Upper bound on IPs learned from DNS observation per rule (Q13 scheme C:
/// keep, do not revoke on TTL; evict oldest first once full).
pub const MAX_OBSERVED_IPS_PER_RULE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectBy {
    Priority,
    Quality,
    Balance,
}

impl SelectBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::Quality => "quality",
            Self::Balance => "balance",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailoverWhen {
    Unreachable,
    Degraded,
    Never,
}

impl FailoverWhen {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Degraded => "degraded",
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AllUnavailable {
    Open,
    Closed,
}

impl AllUnavailable {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainPattern {
    Exact(String),
    Wildcard(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRule {
    pub id: u32,
    pub cidr: Option<Ipv4Cidr>,
    pub domains: Vec<DomainPattern>,
    pub peers: Vec<IpAddr>,
    /// B7: the candidate list of this rule also contains the **live**
    /// `exit_nodes`, appended after the explicit candidates.
    ///
    /// The merge is applied at **reconcile** time, not when the rule is
    /// built, so a hot-patched `exit_nodes` (`config_patch` →
    /// `update_exit_nodes`) reaches the candidate list on the next tick.
    /// Baking the exit nodes into `peers` at construction time would freeze
    /// them for the lifetime of the instance — the very thing
    /// `build_local_route_table` warns about ("that would freeze hot-patched
    /// exit peers on construct-time A").
    ///
    /// Only set for an explicit default-route rule that carries no `domains`
    /// — see `build_rules_from_config`.
    pub merge_exit_nodes: bool,
    pub select_by: SelectBy,
    pub failover_when: FailoverWhen,
    pub if_all_unavailable: AllUnavailable,
    pub require_confirmed: bool,
    /// P4: absolute composite-cost ceiling. A candidate whose precomputed cost
    /// exceeds it counts as *degraded* (`failover_when = degraded`). `None`
    /// means no ceiling — required exactly when `failover_when = degraded`.
    ///
    /// The cost is a **composite** score (§3.9), so the threshold means "this
    /// link is bad overall", never "loss is high".
    pub quality_degraded_cost: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleState {
    Stable,
    Failover,
    Degraded,
}

impl RuleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Failover => "failover",
            Self::Degraded => "degraded",
        }
    }
}

/// The stable identity a `failover_when = never` rule is pinned to.
///
/// A raw `PeerId` is **not** a stable identity in EasyTier: it is a random
/// `u32` picked at instance start (`random_peer_id`) and reported by the peer
/// itself in the handshake, so it changes on every restart and the number is
/// free to be reused by an unrelated node. Pinning the number alone would
/// therefore either (a) send traffic to whoever later claims that number, or
/// (b) black-hole the route forever after the exit node merely restarts.
///
/// The peer's static public key is the identity that survives a restart
/// (`Peer::add_conn` even refuses two conns for one `PeerId` whose keys
/// disagree), so the pin carries both: the key says *who*, the id says *which
/// of my current entries is that who*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedPeer {
    /// Peer id at pin time. Used to find the entry; refreshed when the same
    /// key reappears under a new id after a restart.
    pub peer_id: PeerId,
    /// The remote static public key. `None` when the conn carried no key
    /// (plaintext / no-noise mode), in which case the pin degrades to the
    /// id-only behaviour it had before.
    pub public_key: Option<Vec<u8>>,
}

impl PinnedPeer {
    pub fn new(peer_id: PeerId, public_key: Option<Vec<u8>>) -> Self {
        Self {
            peer_id,
            public_key,
        }
    }

    /// `true` when `candidate` is the same node this pin names.
    ///
    /// With a key on both sides the key decides; the id is only a hint. Without
    /// a key we can only compare ids, which is exactly the weak check — but it
    /// is no weaker than the pin had before, and it keeps `noise`-less setups
    /// working.
    pub fn matches(&self, peer_id: PeerId, public_key: Option<&[u8]>) -> bool {
        match (self.public_key.as_deref(), public_key) {
            (Some(pinned), Some(current)) => pinned == current,
            _ => self.peer_id == peer_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedRule {
    pub rule: RouteRule,
    pub compiled_cidrs: BTreeSet<Ipv4Cidr>,
    /// IPs learned from observed MagicDNS answers (P3), oldest first. Kept even
    /// when they stop being returned; FIFO-evicted past the per-rule cap.
    pub observed_ips: VecDeque<Ipv4Addr>,
    /// Eligible reachable candidates, in config order. Populated by the
    /// reconcile loop for `select_by = balance` (P5); the hot path reads it to
    /// spread flows by `flow_key`. Empty for every other mode.
    pub resolved_peers: Vec<PeerId>,
    pub state: RuleState,
    /// Selected egress peer for `priority` / `quality`. For `balance` this is
    /// **informational only** (always the first entry of `resolved_peers`);
    /// the hot-path `lookup` ignores it and hashes into `resolved_peers`.
    /// Reconcile always writes the pair together; `lookup` asserts that in debug.
    pub active_peer: Option<PeerId>,
    /// P4: consecutive reconcile ticks the challenger has beaten the incumbent
    /// by more than the switch band. Switch only once it reaches
    /// `ConnSelectConfig::switch_windows`; reset whenever the incumbent holds.
    pub challenger: Option<(PeerId, u32)>,
    /// L3: the peer a `failover_when = never` rule pinned to, chosen from the
    /// **first** candidate that ever resolved.
    ///
    /// `never` takes only `peers[0]` (which `validate_route_rule` restricts to
    /// `select_by = priority`), and its meaning is "never move to another
    /// candidate **of this rule**" — not "the rule evaporates once that peer
    /// drops". So the identity of that peer is remembered here when the
    /// reconcile loop first sees it, and `lookup` keeps handing packets to it
    /// while it is unreachable. The packets then go down the ordinary peer-map
    /// path, which holds / re-establishes the tunnel instead of the rule
    /// flipping to `Degraded` and falling through to a different egress.
    ///
    /// `None` for every other `failover_when`: they are free to pick a
    /// different candidate and must follow reachability.
    pub pinned_peer: Option<PinnedPeer>,
    /// L2/R4: the peers **this rule** actually priced on the last reconcile
    /// pass, in candidate order, each with its own cost and source.
    ///
    /// The snapshot's `peer_quality` is shared by every rule and is a memo, not
    /// a per-rule record: a peer another rule reached first is already in it.
    /// Reading the map by peer id would show a `priority` rule a number it
    /// never consulted (or a never-measured placeholder) depending on the order
    /// the rules happen to be reconciled in. Recording the numbers here keeps
    /// the report a property of the rule. Empty when the rule reads no cost at
    /// all (`priority` + `unreachable` / `never`).
    pub costed_peers: Vec<CostedPeer>,
}

/// Management-plane view of one rule after live `exit_nodes` merge + reconcile.
///
/// Config-layer `PolicyRouteConfig` alone cannot show sugar `/0` rules or the
/// candidate list the data plane actually uses (L6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePolicyRouteInfo {
    pub rule_id: u32,
    pub cidr: Option<String>,
    pub domains: Vec<String>,
    pub config_peers: Vec<String>,
    pub effective_candidates: Vec<String>,
    pub merge_exit_nodes: bool,
    pub select_by: &'static str,
    pub failover_when: &'static str,
    pub if_all_unavailable: &'static str,
    pub state: &'static str,
    pub active_peer_id: Option<PeerId>,
    pub resolved_peer_ids: Vec<PeerId>,
    /// L3: the peer a `failover_when = never` rule pinned to. Still set (and
    /// still carried by the hot path) while it is unreachable — see
    /// [`ResolvedRule::pinned_peer`].
    pub pinned_peer_id: Option<PeerId>,
    /// L2: per-candidate composite cost and where it came from, for rules that
    /// actually consult a cost (`quality` / `degraded`). Empty otherwise.
    ///
    /// Two costs in the same unit can still come from different measurements —
    /// see [`CostSource`] — and a threshold (`quality_degraded_cost`) only means
    /// what it says when the reader knows which one it is looking at.
    pub peer_costs: Vec<PeerCostInfo>,
    pub sugar: bool,
}

/// L2: one candidate's cost plus its source, as shown on the management plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCostInfo {
    pub peer_id: PeerId,
    pub cost: i32,
    pub source: CostSource,
}

/// Build the management-plane view for every rule in `snap`.
pub fn collect_effective_policy_routes(
    snap: &LocalRouteSnapshot,
    exit_nodes: &[IpAddr],
) -> Vec<EffectivePolicyRouteInfo> {
    snap.rules
        .iter()
        .map(|r| {
            let candidates = merged_candidates(&r.rule, exit_nodes);
            let sugar = r.rule.id == u32::MAX;
            EffectivePolicyRouteInfo {
                rule_id: r.rule.id,
                cidr: r.rule.cidr.map(|c| c.to_string()),
                domains: r
                    .rule
                    .domains
                    .iter()
                    .map(|d| match d {
                        DomainPattern::Exact(name) => name.clone(),
                        DomainPattern::Wildcard(parent) => format!("*.{parent}"),
                    })
                    .collect(),
                config_peers: r.rule.peers.iter().map(ToString::to_string).collect(),
                effective_candidates: candidates.iter().map(ToString::to_string).collect(),
                merge_exit_nodes: r.rule.merge_exit_nodes,
                select_by: r.rule.select_by.as_str(),
                failover_when: r.rule.failover_when.as_str(),
                if_all_unavailable: r.rule.if_all_unavailable.as_str(),
                state: r.state.as_str(),
                active_peer_id: r.active_peer,
                resolved_peer_ids: r.resolved_peers.clone(),
                pinned_peer_id: r.pinned_peer.as_ref().map(|pin| pin.peer_id),
                // Only the peers **this rule** priced, with the numbers it saw at
                // that moment. Reading the shared `peer_quality` map instead
                // would show another rule's candidates and, worse, a cost this
                // rule never consulted (R4).
                peer_costs: r
                    .costed_peers
                    .iter()
                    .map(|costed| PeerCostInfo {
                        peer_id: costed.peer_id,
                        cost: costed.cost,
                        source: costed.source,
                    })
                    .collect(),
                sugar,
            }
        })
        .collect()
}

/// L2: where a candidate's composite cost came from.
///
/// Both variants are encoded with `ospf_edge_cost_from_score`, so they share a
/// unit — but they do **not** share a source, and that matters when the two are
/// compared or when one replaces the other between reconcile ticks:
///
/// - [`CostSource::Path`] — the accumulated weight of the whole path from the
///   OSPF least-cost table. It is the sum of per-hop edge costs as **published**
///   by each peer, so it carries whichever relay penalty those peers applied.
/// - [`CostSource::Edge`] — the local direct-link measurement of the first hop
///   only (`best_edge_cost` over `list_peer_conns`).
///
/// The reconcile loop prefers `Path` and falls back to `Edge` while the route
/// table does not know the peer yet. Keeping the tag lets the switch between
/// the two be visible instead of silently re-baselining the hysteresis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CostSource {
    #[default]
    Path,
    Edge,
}

impl CostSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Edge => "edge",
        }
    }
}

/// R4: one candidate this rule actually priced, together with the number and
/// the family it was measured in.
///
/// The per-rule list has to carry the numbers itself instead of naming peers
/// that the shared `peer_quality` memo is then read for. A `priority` rule that
/// shares a candidate with a `quality` rule hits the memo and would otherwise
/// inherit — and display — a cost it never consulted, decided purely by which
/// rule the reconcile loop happened to visit first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostedPeer {
    pub peer_id: PeerId,
    pub cost: i32,
    pub source: CostSource,
}

#[derive(Debug, Clone, Default)]
pub struct LocalRouteSnapshot {
    pub rules: Vec<ResolvedRule>,
    /// Explicit CIDRs bucketed by prefix length (index `0..=32`). Each bucket is
    /// sorted by network address and holds at most one entry per network, so a
    /// lookup walks the buckets longest-prefix first and binary-searches inside
    /// each: a miss costs `O(33 · log n)` instead of a full `O(n)` scan.
    pub cidr_buckets: Vec<Vec<CidrEntry>>,
    /// Exact `/32` → winning rule index. Only one rule can own an exact
    /// address (same specificity ⇒ `closed` wins ⇒ lower rule index), so this
    /// is a flat map, not a list — an exact `/32` always beats any LPM entry.
    pub by_ip: HashMap<Ipv4Addr, RuleIdx>,
    /// Per-peer composite cost, precomputed by the reconcile loop (P4). Never
    /// read on the packet hot path — `active_peer` already carries the verdict.
    pub peer_quality: HashMap<PeerId, i32>,
    /// L2: which of the two cost families produced each `peer_quality` entry.
    /// Diagnostic only — used to explain the number and to report a source
    /// switch, never to rank candidates.
    pub peer_cost_source: HashMap<PeerId, CostSource>,
    /// `true` when at least one rule uses `select_by = balance`.
    ///
    /// Only a `balance` rule cares about the flow key, so a caller that would
    /// have to *derive* one (parse the inner IP header and hash it) can ask
    /// this first and skip that work entirely.
    pub has_balance_rules: bool,
}

pub struct LocalRouteTable {
    snapshot: ArcSwap<LocalRouteSnapshot>,
    /// Monotonic generation for L2 / observers. Not on the packet hot path.
    revision: AtomicU64,
}

/// One rule's reconcile verdict (P4/P5), applied by `apply_peer_updates`.
#[derive(Debug, Clone)]
pub struct RuleUpdate {
    pub idx: RuleIdx,
    pub active_peer: Option<PeerId>,
    pub state: RuleState,
    pub challenger: Option<(PeerId, u32)>,
    /// P5: eligible reachable candidates for `select_by = balance`. Empty for
    /// other modes (the hot path then reads `active_peer` instead).
    pub resolved_peers: Vec<PeerId>,
    /// L3: sticky target of a `failover_when = never` rule (see
    /// [`ResolvedRule::pinned_peer`]). `None` for every other rule.
    pub pinned_peer: Option<PinnedPeer>,
    /// L2/R4: peers this rule priced on this pass, with their cost and source
    /// (see [`ResolvedRule::costed_peers`]).
    pub costed_peers: Vec<CostedPeer>,
}

/// Outcome of a hot-path lookup. `Miss` must fall through to the original L3
/// order; `Block` must drop the packet instead of leaking out the physical
/// gateway (`if_all_unavailable = closed` with every candidate down).
///
/// Every `Hit` is an **egress** hand-off: a `[[policy_routes]]` rule always
/// points at a gateway peer that NATs the packet out (§4.1 "B 做网关"), so the
/// `exit_node` flag is always set. The gateway still prefers its own
/// `proxy_cidrs` mapping when it has one, so subnet-proxy behaviour is
/// preserved without a separate flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteLookup {
    Hit { peer: PeerId },
    Block,
    Miss,
}

impl Default for LocalRouteTable {
    fn default() -> Self {
        Self::empty()
    }
}

impl LocalRouteTable {
    pub fn empty() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(LocalRouteSnapshot::default()),
            revision: AtomicU64::new(0),
        }
    }

    /// Generation counter for L2 reconcile skip gates (`cidr_monitor`).
    #[inline]
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    fn bump_revision(&self) {
        // Inequality-only signal for the 1s L2 poll; Relaxed is enough.
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    pub fn store(&self, snap: LocalRouteSnapshot) {
        self.snapshot.store(Arc::new(snap));
        self.bump_revision();
    }

    pub fn load_full(&self) -> Arc<LocalRouteSnapshot> {
        self.snapshot.load_full()
    }

    /// `true` when any rule carries domains (so MagicDNS observation is useful).
    ///
    /// Only the `proxy-packet` MagicDNS observe path calls this (skip parsing
    /// when there is nothing to learn). Gate with the same feature so default
    /// `easytier-core` builds do not treat it as dead_code under `-D warnings`.
    #[cfg(feature = "proxy-packet")]
    pub fn has_domain_rules(&self) -> bool {
        self.snapshot
            .load()
            .rules
            .iter()
            .any(|r| !r.rule.domains.is_empty())
    }

    /// Hot-path lookup: sync, lock-free, no allocation beyond ArcSwap guard.
    ///
    /// An empty table (no `[[policy_routes]]`) short-circuits after a single
    /// `ArcSwap` load, so the original L3 path keeps its exact cost.
    ///
    /// `flow_key` is only consulted by `select_by = balance` rules (P5); every
    /// other mode hands the packet to the reconciled `active_peer` and ignores
    /// it. Callers that have no payload pass `None`.
    #[inline]
    pub fn lookup(&self, ip: &Ipv4Addr, flow_key: Option<u64>) -> RouteLookup {
        let snap = self.snapshot.load();
        if snap.rules.is_empty() {
            return RouteLookup::Miss;
        }
        let Some(idx) = match_rule(&snap, ip) else {
            return RouteLookup::Miss;
        };
        let Some(r) = snap.rules.get(idx) else {
            return RouteLookup::Miss;
        };
        match r.state {
            // L3: a `never` rule that has already pinned a peer keeps sending
            // to it even after it became unreachable. "Never fail over" means
            // "never move to another candidate", not "stop routing when the one
            // I chose goes down" — falling through here would hand the flow to
            // the physical gateway (or a peer `/0`), i.e. exactly the egress the
            // user said not to use.
            //
            // The pinned peer may be gone from the peer map; the ordinary send
            // path then holds or re-establishes the tunnel. That is the same
            // place a stale-but-live peer ends up, so no new failure mode.
            //
            // Identity is verified at **reconcile** time, not here: the hot
            // path is lock-free and cannot read another peer's public key. The
            // reconcile loop is the only writer of `pinned_peer`, and it never
            // publishes a pin whose key no longer matches the entry behind that
            // id (a reused id gets its pin dropped — see
            // `PeerMap::peer_public_key`), so a `Hit` here always names the
            // node the user pinned.
            RuleState::Degraded => {
                if let Some(pinned) = r.pinned_peer.as_ref() {
                    return RouteLookup::Hit {
                        peer: pinned.peer_id,
                    };
                }
                match r.rule.if_all_unavailable {
                    AllUnavailable::Closed => RouteLookup::Block,
                    AllUnavailable::Open => RouteLookup::Miss,
                }
            }
            // Defensive: a consistent reconcile always pairs `None` with
            // `Degraded`. If a writer race ever left Stable/Failover without a
            // peer, honour `closed` instead of silently falling through.
            RuleState::Stable | RuleState::Failover => {
                // P5: balance spreads flows over the candidate list; everything
                // else uses the single reconciled `active_peer`.
                //
                // L4: `active_peer` is informational for balance (always the
                // first resolved candidate). Lookup deliberately ignores it —
                // assert the pair stayed in lockstep so a future writer that
                // forks them cannot go unnoticed in debug builds.
                let peer = if r.rule.select_by == SelectBy::Balance {
                    debug_assert!(
                        r.active_peer.is_none()
                            || r.active_peer == r.resolved_peers.first().copied(),
                        "balance active_peer drifted from resolved_peers[0]"
                    );
                    pick_balance(flow_key, &r.resolved_peers)
                } else {
                    r.active_peer
                };
                match peer {
                    Some(peer) => RouteLookup::Hit { peer },
                    None => match r.rule.if_all_unavailable {
                        AllUnavailable::Closed => RouteLookup::Block,
                        AllUnavailable::Open => RouteLookup::Miss,
                    },
                }
            }
        }
    }

    /// `true` when `ip` matches a `balance` rule that has **more than one**
    /// eligible candidate.
    ///
    /// Such a rule picks the peer by the flow key, so a caller that cannot
    /// supply one cannot know which peer will really carry the flow —
    /// `lookup` would fall back to slot 0 and answer for a peer that may not
    /// be the one. Such a caller must not assume a specific peer owns the
    /// destination.
    pub fn is_balance_ambiguous(&self, ip: &Ipv4Addr) -> bool {
        let snap = self.snapshot.load();
        let Some(idx) = match_rule(&snap, ip) else {
            return false;
        };
        let Some(r) = snap.rules.get(idx) else {
            return false;
        };
        if r.rule.select_by != SelectBy::Balance {
            return false;
        }
        // `resolved_peers` is an **observed** state: it is empty until the
        // reconcile loop has resolved the candidates (and stays empty while
        // none of them is reachable). Answering "not ambiguous" from an empty
        // list would be answering a configuration question with an
        // observation — a balance rule that has not been reconciled yet may
        // well have several candidates. So an empty list counts as
        // *possibly* ambiguous, which makes the caller refuse the bypass.
        r.resolved_peers.len() != 1
    }

    /// `true` when at least one rule uses `select_by = balance`.
    ///
    /// Only a `balance` rule picks its peer by the flow key, so a caller that
    /// would have to *derive* one (parse the inner IP header, hash it) can ask
    /// this first and skip that work on the packet path.
    pub fn has_balance_rules(&self) -> bool {
        self.snapshot.load().has_balance_rules
    }

    /// CIDRs this table contributes to the L2 desired set.
    ///
    /// Only `Stable` / `Failover` rules count: a `Degraded` rule must **not**
    /// pull traffic into the TUN (that is exactly the black-hole case §7.3
    /// warns about).
    ///
    /// L3 exception: a `never` rule that still holds a [`ResolvedRule::pinned_peer`]
    /// contributes **even when `Degraded`**. The pin is the user's explicit
    /// "route this through *that* peer and nowhere else", so pulling the L2
    /// route would silently send the traffic out the physical gateway — the one
    /// thing `never` exists to prevent. The hot path still answers `Hit` for it
    /// via the same pin.
    pub fn l2_contributing_cidrs(&self) -> BTreeSet<Ipv4Cidr> {
        let snap = self.snapshot.load();
        let mut out = BTreeSet::new();
        for r in &snap.rules {
            if matches!(r.state, RuleState::Stable | RuleState::Failover) || r.pinned_peer.is_some()
            {
                out.extend(r.compiled_cidrs.iter().copied());
            }
        }
        out
    }

    /// Record IPs observed in a MagicDNS answer for `qname` (P3).
    ///
    /// The rule set stays authoritative from local config; only the IP set is
    /// learned. Old IPs are **kept** — a TTL never revokes a route (§9.8.5) —
    /// and the oldest are FIFO-evicted past `MAX_OBSERVED_IPS_PER_RULE`.
    ///
    /// Uses `ArcSwap::rcu` so concurrent reconcile updates to `active_peer` /
    /// `state` are not lost, and `compiled_cidrs` (L2) + `by_ip` (L3) still
    /// swap in one store.
    ///
    /// Returns `true` when the snapshot changed.
    pub fn observe_ips(&self, qname: &str, ips: &[Ipv4Addr]) -> bool {
        if ips.is_empty() {
            return false;
        }
        let mut changed = false;
        self.snapshot.rcu(|old| {
            changed = false;
            if old.rules.is_empty() {
                return Arc::clone(old);
            }
            let Some(idx) = match_domain_rule(old, qname) else {
                return Arc::clone(old);
            };
            // Skip the clone when every IP is already known.
            if ips
                .iter()
                .all(|ip| old.rules[idx].observed_ips.contains(ip))
            {
                return Arc::clone(old);
            }

            // Past the early returns we always mutate `observed_ips`. Publish
            // the new snapshot even when every new IP was already present as an
            // explicit `/32` (insert is a no-op) — otherwise the deque update
            // and any FIFO eviction of *other* /32s are discarded.
            let mut new = (**old).clone();
            let rule = &mut new.rules[idx];
            let mut index_dirty = false;
            for ip in ips {
                if rule.observed_ips.contains(ip) {
                    continue;
                }
                rule.observed_ips.push_back(*ip);
                while rule.observed_ips.len() > MAX_OBSERVED_IPS_PER_RULE {
                    let evicted = rule.observed_ips.pop_front().expect("non-empty");
                    if let Ok(cidr) = Ipv4Cidr::new(evicted, 32) {
                        // FIFO only manages *observed* /32s. An explicit
                        // `cidr = "x.x.x.x/32"` in the rule config must survive
                        // eviction even when the same address was also learned
                        // from DNS and sat at the front of the deque.
                        if rule.rule.cidr != Some(cidr) && rule.compiled_cidrs.remove(&cidr) {
                            index_dirty = true;
                        }
                    }
                }
                if let Ok(cidr) = Ipv4Cidr::new(*ip, 32)
                    && rule.compiled_cidrs.insert(cidr)
                {
                    index_dirty = true;
                }
            }
            if index_dirty {
                rebuild_index(&mut new);
            }
            changed = true;
            Arc::new(new)
        });
        if changed {
            self.bump_revision();
        }
        changed
    }

    /// Apply reconcile results (`peer_quality` + `active_peer` / `state`) via RCU.
    ///
    /// Async peer selection happens *before* this call; the closure only patches
    /// these fields so concurrent `observe_ips` writes survive. `peer_quality` is
    /// stored even when no rule flipped, so the switch-debounce band reads a
    /// fresh baseline next tick.
    ///
    /// `peer_cost_source` (L2) is metadata for that baseline: the hysteresis
    /// below must not compare a path cost with an edge cost, so the loop uses
    /// the previous tick's source to decide whether the previous number is a
    /// usable baseline at all.
    pub fn apply_peer_updates(
        &self,
        peer_quality: &HashMap<PeerId, i32>,
        peer_cost_source: &HashMap<PeerId, CostSource>,
        updates: &[RuleUpdate],
    ) {
        if updates.is_empty() && peer_quality.is_empty() {
            return;
        }
        let mut l2_state_changed = false;
        self.snapshot.rcu(|old| {
            l2_state_changed = false;
            let mut any =
                old.peer_quality != *peer_quality || old.peer_cost_source != *peer_cost_source;
            for update in updates {
                let Some(r) = old.rules.get(update.idx) else {
                    continue;
                };
                if r.active_peer != update.active_peer
                    || r.state != update.state
                    || r.challenger != update.challenger
                    || r.resolved_peers != update.resolved_peers
                    || r.pinned_peer != update.pinned_peer
                    || r.costed_peers != update.costed_peers
                {
                    any = true;
                    if r.state != update.state {
                        // Only RuleState flips change `l2_contributing_cidrs`.
                        l2_state_changed = true;
                    }
                    // Keep scanning for state flips even after `any` is set.
                }
            }
            if !any {
                return Arc::clone(old);
            }
            let mut new = (**old).clone();
            new.peer_quality = peer_quality.clone();
            new.peer_cost_source = peer_cost_source.clone();
            for update in updates {
                let Some(r) = new.rules.get_mut(update.idx) else {
                    continue;
                };
                r.active_peer = update.active_peer;
                r.state = update.state;
                r.challenger = update.challenger;
                r.resolved_peers.clone_from(&update.resolved_peers);
                r.pinned_peer.clone_from(&update.pinned_peer);
                r.costed_peers.clone_from(&update.costed_peers);
            }
            Arc::new(new)
        });
        if l2_state_changed {
            self.bump_revision();
        }
    }
}

fn match_rule(snap: &LocalRouteSnapshot, ip: &Ipv4Addr) -> Option<RuleIdx> {
    if let Some(idx) = snap.by_ip.get(ip) {
        return Some(*idx);
    }
    let addr = u32::from(*ip);
    // Longest prefix first. Inside a bucket entries are sorted by network
    // address and deduplicated, so the only candidate is the greatest entry
    // whose network is `<= addr` — check it for containment.
    for bucket in snap.cidr_buckets.iter().rev() {
        let pos = bucket.partition_point(|entry| entry.network <= addr);
        let Some(entry) = pos.checked_sub(1).and_then(|i| bucket.get(i)) else {
            continue;
        };
        if entry.cidr.contains(ip) {
            return Some(entry.idx);
        }
    }
    None
}

/// Build an indexed snapshot from validated rules (active_peer initially unset /
/// Degraded until reconcile fills them).
pub fn build_snapshot(rules: Vec<RouteRule>) -> LocalRouteSnapshot {
    let resolved: Vec<ResolvedRule> = rules
        .into_iter()
        .map(|rule| {
            let mut compiled = BTreeSet::new();
            if let Some(cidr) = rule.cidr {
                compiled.insert(cidr);
            }
            ResolvedRule {
                rule,
                compiled_cidrs: compiled,
                observed_ips: VecDeque::new(),
                resolved_peers: Vec::new(),
                state: RuleState::Degraded,
                active_peer: None,
                challenger: None,
                pinned_peer: None,
                costed_peers: Vec::new(),
            }
        })
        .collect();

    let mut snap = LocalRouteSnapshot {
        rules: resolved,
        ..Default::default()
    };
    snap.has_balance_rules = snap
        .rules
        .iter()
        .any(|r| r.rule.select_by == SelectBy::Balance);
    rebuild_index(&mut snap);
    snap
}

/// P5: pick the candidate for `flow_key` by stable modulo.
///
/// The **only** arithmetic `balance` adds on the packet hot path — still no
/// lock, no async, no allocation. The same flow key always lands on the same
/// candidate, so one TCP stream keeps one egress until the candidate list
/// changes (a switch does **not** migrate established connections, §12.3).
/// A missing flow key degrades to slot 0.
pub fn pick_balance(flow_key: Option<u64>, peers: &[PeerId]) -> Option<PeerId> {
    if peers.is_empty() {
        return None;
    }
    let slot = flow_key.unwrap_or(0) % peers.len() as u64;
    peers.get(slot as usize).copied()
}

/// P4: whether a precomputed composite cost counts as *degraded* for this rule.
///
/// Only `failover_when = degraded` carries a ceiling; the other modes accept
/// any reachable candidate.
pub fn is_degraded_cost(failover_when: FailoverWhen, ceiling: Option<i32>, cost: i32) -> bool {
    failover_when == FailoverWhen::Degraded && ceiling.is_some_and(|limit| cost > limit)
}

/// P4 switch debounce: `true` when the incumbent cost is close enough to the
/// best cost (lower = better) to stay put.
///
/// Mirrors `conn_bond::in_quality_band` but on integer costs: relative
/// (`switch_margin`) **or** absolute (`switch_abs_margin`, expressed in score
/// units → ×1000 to reach cost units). Keeps cost noise from flapping routes.
pub fn within_switch_band(current: i32, best: i32, cfg: ConnSelectConfig) -> bool {
    if current <= best {
        return true;
    }
    let delta = f64::from(current - best);
    delta / f64::from(best.max(1)) <= cfg.switch_margin || delta <= cfg.switch_abs_margin * 1000.0
}

/// Rebuild the derived `by_ip` / `cidr_buckets` indexes from `rules`.
///
/// Called on snapshot build and after every `observe_ips` batch. An exact `/32`
/// always wins over any LPM entry; among overlapping entries the winner is
/// longer prefix → `closed` → lower index.
fn rebuild_index(snap: &mut LocalRouteSnapshot) {
    let mut by_ip: HashMap<Ipv4Addr, RuleIdx> = HashMap::new();
    // (prefix_len, network, strictness, cidr, rule) — sorted then deduplicated so
    // each (prefix_len, network) keeps exactly one winner.
    let mut entries: Vec<(u8, u32, u8, Ipv4Cidr, RuleIdx)> = Vec::new();

    for (idx, r) in snap.rules.iter().enumerate() {
        let strict = match r.rule.if_all_unavailable {
            AllUnavailable::Closed => 0u8,
            AllUnavailable::Open => 1u8,
        };
        for cidr in &r.compiled_cidrs {
            let len = cidr.network_length();
            let network = u32::from(cidr.first_address());
            if len == 32 {
                let ip = cidr.first_address();
                match by_ip.get(&ip).copied() {
                    None => {
                        by_ip.insert(ip, idx);
                    }
                    Some(old) => {
                        // All `/32` share the same specificity: `closed` wins,
                        // then the lower rule index.
                        let old_strict = match snap.rules[old].rule.if_all_unavailable {
                            AllUnavailable::Closed => 0u8,
                            AllUnavailable::Open => 1u8,
                        };
                        if strict < old_strict || (strict == old_strict && idx < old) {
                            by_ip.insert(ip, idx);
                        }
                    }
                }
            } else {
                entries.push((len, network, strict, *cidr, idx));
            }
        }
    }

    // Prefix length asc → network asc → closed first → lower rule index. The
    // first row of each `(len, network)` group is therefore its winner, and each
    // bucket ends up sorted by network address (ready for binary search).
    entries.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.4.cmp(&b.4))
    });
    let mut buckets: Vec<Vec<CidrEntry>> = (0..=32).map(|_| Vec::new()).collect();
    let mut last: Option<(u8, u32)> = None;
    for (len, network, _strict, cidr, idx) in entries {
        if last == Some((len, network)) {
            continue; // a better entry for this (len, network) already landed
        }
        last = Some((len, network));
        buckets[len as usize].push(CidrEntry { network, cidr, idx });
    }

    snap.by_ip = by_ip;
    snap.cidr_buckets = buckets;
}

/// Normalize a DNS name: lower case, no trailing dot.
pub fn normalize_qname(qname: &str) -> String {
    qname.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// `true` when `name` is exactly `zone` or a subdomain of it.
///
/// Both sides are normalized (lower case, no trailing dot). An empty zone never
/// matches — otherwise every name would be treated as being inside it.
pub fn name_is_in_zone(name: &str, zone: &str) -> bool {
    let name = normalize_qname(name);
    let zone = normalize_qname(zone);
    if zone.is_empty() {
        return false;
    }
    if name == zone {
        return true;
    }
    name.len() > zone.len()
        && name.ends_with(&zone)
        && name.as_bytes()[name.len() - zone.len() - 1] == b'.'
}

/// Find the highest-priority rule owning `qname`.
///
/// Exact match beats any wildcard; among wildcards the longest suffix wins
/// (which is also the RFC 4592 most-specific one). Not on the packet hot path.
fn match_domain_rule(snap: &LocalRouteSnapshot, qname: &str) -> Option<RuleIdx> {
    let qname = normalize_qname(qname);
    if qname.is_empty() {
        return None;
    }
    let mut best: Option<(usize, RuleIdx)> = None;
    for (idx, r) in snap.rules.iter().enumerate() {
        for pattern in &r.rule.domains {
            let specificity = match pattern {
                DomainPattern::Exact(domain) if domain == &qname => usize::MAX,
                DomainPattern::Wildcard(suffix)
                    if qname.len() > suffix.len()
                        && qname.ends_with(suffix.as_str())
                        && qname.as_bytes()[qname.len() - suffix.len() - 1] == b'.' =>
                {
                    suffix.len()
                }
                _ => continue,
            };
            if best.is_none_or(|(best_spec, _)| specificity > best_spec) {
                best = Some((specificity, idx));
            }
        }
    }
    best.map(|(_, idx)| idx)
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyRouteError {
    #[error("{0}")]
    Invalid(String),
}

pub fn parse_select_by(s: &str) -> Result<SelectBy, PolicyRouteError> {
    match s {
        "priority" => Ok(SelectBy::Priority),
        "quality" => Ok(SelectBy::Quality),
        "balance" => Ok(SelectBy::Balance),
        other => Err(PolicyRouteError::Invalid(format!(
            "unknown select_by={other:?}; expected priority|quality|balance"
        ))),
    }
}

pub fn parse_failover_when(s: &str) -> Result<FailoverWhen, PolicyRouteError> {
    match s {
        "unreachable" => Ok(FailoverWhen::Unreachable),
        "degraded" => Ok(FailoverWhen::Degraded),
        "never" => Ok(FailoverWhen::Never),
        other => Err(PolicyRouteError::Invalid(format!(
            "unknown failover_when={other:?}; expected unreachable|degraded|never"
        ))),
    }
}

pub fn parse_all_unavailable(s: &str) -> Result<AllUnavailable, PolicyRouteError> {
    match s {
        "open" => Ok(AllUnavailable::Open),
        "closed" => Ok(AllUnavailable::Closed),
        other => Err(PolicyRouteError::Invalid(format!(
            "unknown if_all_unavailable={other:?}; expected open|closed"
        ))),
    }
}

pub fn parse_domain_pattern(raw: &str) -> Result<DomainPattern, PolicyRouteError> {
    let name = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if name.is_empty() {
        return Err(PolicyRouteError::Invalid(
            "empty domain pattern".to_string(),
        ));
    }
    if let Some(rest) = name.strip_prefix("*.") {
        let labels: Vec<_> = rest.split('.').filter(|l| !l.is_empty()).collect();
        if labels.len() < 2 {
            return Err(PolicyRouteError::Invalid(format!(
                "wildcard domain {raw:?} parent must have ≥2 labels (RFC 4592)"
            )));
        }
        if rest.contains('*') {
            return Err(PolicyRouteError::Invalid(format!(
                "wildcard only allowed as leftmost label: {raw:?}"
            )));
        }
        Ok(DomainPattern::Wildcard(rest.to_string()))
    } else if name.contains('*') {
        Err(PolicyRouteError::Invalid(format!(
            "invalid domain pattern {raw:?}"
        )))
    } else {
        Ok(DomainPattern::Exact(name))
    }
}

/// Validate one config entry. A rule must carry `cidr`, `domains`, or both;
/// domain-only rules learn their IPs from observation (P3).
pub fn validate_route_rule(rule: &RouteRule) -> Result<(), PolicyRouteError> {
    if rule.cidr.is_none() && rule.domains.is_empty() {
        return Err(PolicyRouteError::Invalid(
            "policy route needs at least cidr or domains".into(),
        ));
    }
    if rule.peers.is_empty() && !rule.merge_exit_nodes {
        // A rule that takes the **live** `exit_nodes` may legitimately have an
        // empty candidate list of its own — the reconcile loop fills it from
        // the live list on every tick. Every other rule must name its peers.
        //
        // In practice this exemption only reaches the generated `exit_nodes`
        // sugar rule: an *explicit* rule is validated before it is marked, so
        // it always has to name its peers.
        return Err(PolicyRouteError::Invalid(
            "policy route needs at least one peer".into(),
        ));
    }
    // `never` keeps only the first candidate, so `quality` / `balance` have
    // nothing to choose from and would silently behave like `priority`.
    //
    // Enforced here as well as in `PolicyRouteConfig::validate`: the
    // `ValidatedPolicyRoute` fields are public, so a caller can hand a
    // hand-built value to `build_rules_from_config` and bypass the config
    // layer's check.
    if rule.failover_when == FailoverWhen::Never && rule.select_by != SelectBy::Priority {
        return Err(PolicyRouteError::Invalid(format!(
            "policy route {}: failover_when=never keeps only the first candidate, so \
             select_by={} cannot take effect; use failover_when=unreachable (or \
             select_by=priority)",
            rule.id,
            match rule.select_by {
                SelectBy::Priority => "priority",
                SelectBy::Quality => "quality",
                SelectBy::Balance => "balance",
            }
        )));
    }
    if rule.domains.len() > MAX_DOMAINS_PER_RULE {
        return Err(PolicyRouteError::Invalid(format!(
            "domains exceed limit {MAX_DOMAINS_PER_RULE}"
        )));
    }
    if rule.peers.len() > MAX_PEERS_PER_RULE {
        return Err(PolicyRouteError::Invalid(format!(
            "peers exceed limit {MAX_PEERS_PER_RULE}"
        )));
    }
    // `quality_degraded_cost` and `failover_when=degraded` must travel together:
    // a degraded rule needs a ceiling to compare against, and a ceiling is dead
    // weight without a degraded rule. Hard failure either way (NFR-6).
    match (rule.failover_when, rule.quality_degraded_cost) {
        (FailoverWhen::Degraded, None) => {
            return Err(PolicyRouteError::Invalid(
                "failover_when=degraded requires quality_degraded_cost".into(),
            ));
        }
        (FailoverWhen::Degraded, Some(cost)) if cost <= 0 => {
            return Err(PolicyRouteError::Invalid(
                "quality_degraded_cost must be positive".into(),
            ));
        }
        (FailoverWhen::Unreachable | FailoverWhen::Never, Some(_)) => {
            return Err(PolicyRouteError::Invalid(
                "quality_degraded_cost is only meaningful with failover_when=degraded".into(),
            ));
        }
        _ => {}
    }
    // `closed` with several candidates is a multi-exit black hole, so it needs
    // the explicit confirmation.
    //
    // This only sees the configured peers: the merge can only happen after the
    // `merge_exit_nodes` flag is set, so the overlap-aware count is checked in
    // `build_rules_from_config` (once the flag is known) and re-checked on
    // every hot patch by `update_exit_nodes`.
    if rule.if_all_unavailable == AllUnavailable::Closed
        && !rule.require_confirmed
        && rule.peers.len() > 1
    {
        return Err(PolicyRouteError::Invalid(format!(
            "policy route {}: if_all_unavailable=closed with {} peers requires require_confirmed=true",
            rule.id,
            rule.peers.len()
        )));
    }
    Ok(())
}

pub fn ipv4_default_cidr() -> Ipv4Cidr {
    Ipv4Cidr::new(Ipv4Addr::UNSPECIFIED, 0).expect("0.0.0.0/0")
}

/// Build rules from `[[policy_routes]]` + optional `exit_nodes` sugar.
pub fn build_rules_from_config(
    policy: &[ValidatedPolicyRoute],
    exit_nodes: &[IpAddr],
) -> Result<Vec<RouteRule>, PolicyRouteError> {
    if policy.len() > MAX_POLICY_ROUTES {
        return Err(PolicyRouteError::Invalid(format!(
            "policy_routes exceed limit {MAX_POLICY_ROUTES}"
        )));
    }
    let mut rules = Vec::with_capacity(policy.len() + usize::from(!exit_nodes.is_empty()));
    for (i, p) in policy.iter().enumerate() {
        let rule = p.to_route_rule(i as u32);
        validate_route_rule(&rule)?;
        rules.push(rule);
    }
    // The default route takes the **live** `exit_nodes` — unconditionally.
    //
    // The flag must not depend on whether `exit_nodes` happens to be non-empty
    // *right now*: the strategy table is built once and never rebuilt, so a
    // flag decided at construction time can never pick up an exit node that a
    // later `config_patch` adds. The candidate list is live (the reconcile
    // loop appends the current `exit_nodes` on every tick), the flag has to
    // be unconditional for that to reach the rule.
    let default_cidr = ipv4_default_cidr();
    // An explicit default-route rule and `exit_nodes` describe the same
    // thing — "who carries my default route" — so they have to end up in
    // **one** candidate list, explicit candidates first.
    //
    // The LPM index keeps exactly one entry per (prefix, network) and
    // drops the later rule, so a second `/0` rule never reached the lookup
    // path and `exit_nodes` was silently inert whenever an explicit `/0`
    // rule existed.
    //
    // The merge is **not** baked into `peers`: the rule only records that
    // it wants the exit nodes, and the reconcile loop appends the *live*
    // `exit_nodes` on every tick. Baking them in would freeze a
    // hot-patched exit node for the lifetime of the instance — exactly
    // what `build_local_route_table` warns about.
    let explicit_default = rules
        .iter_mut()
        .find(|rule| rule.cidr == Some(default_cidr));
    if let Some(rule) = explicit_default {
        if !rule.domains.is_empty() {
            // A rule that also carries `domains` would hand the exit nodes
            // to the **domain** candidates as well, silently widening what
            // the rule means. Refuse it instead of guessing.
            return Err(PolicyRouteError::Invalid(
                "a default-route rule with `domains` cannot be combined with exit_nodes: \
                 the exit nodes would become domain candidates too. Use a separate \
                 default-route rule without `domains`."
                    .into(),
            ));
        }
        rule.merge_exit_nodes = true;
        // "never" keeps only the **first** candidate, so the merged exit
        // nodes would never be selected — the same "configured but
        // silently ineffective" shape that B19 rejects for `select_by`.
        // A rule that takes the exit nodes must be able to fail over to
        // them.
        //
        // Checked unconditionally: `exit_nodes` may be added by a later
        // `config_patch`, and then the merge would silently stop working.
        if rule.failover_when == FailoverWhen::Never {
            return Err(PolicyRouteError::Invalid(
                "a default-route rule with failover_when=\"never\" cannot take the \
                 exit nodes: \"never\" keeps only the first candidate, so the exit \
                 nodes would never be used. Use failover_when=\"unreachable\" instead."
                    .into(),
            ));
        }
        // Validate against the candidate list the rule will actually use.
        let merged = merged_candidate_count(rule, exit_nodes);
        if merged > MAX_PEERS_PER_RULE {
            return Err(PolicyRouteError::Invalid(format!(
                "the explicit default-route rule and exit_nodes together exceed the \
                 peer limit {MAX_PEERS_PER_RULE} ({merged} candidates after the merge); \
                 drop some candidates or split the rule"
            )));
        }
        // `closed` with more than one candidate needs the explicit
        // confirmation — a single-candidate closed rule was legal before
        // the merge and would become illegal after it.
        //
        // This is the check L1 was missing: `validate_route_rule` above only
        // sees `rule.peers`, and the merge happens **here**, so a one-peer
        // `closed` default route plus any exit node slipped through without
        // `require_confirmed`. It counts the merged list because that is what
        // the rule walks — for `priority` / `quality` a failover chain, for
        // `balance` the spread pool (where every exit node is its own black
        // hole).
        if merged > 1
            && rule.if_all_unavailable == AllUnavailable::Closed
            && !rule.require_confirmed
        {
            return Err(PolicyRouteError::Invalid(format!(
                "policy route {}: if_all_unavailable=closed with {merged} peers \
                 (explicit candidates + exit_nodes) requires require_confirmed=true",
                rule.id
            )));
        }
    } else {
        // No explicit default-route rule: the exit nodes get their own
        // `/0` rule — but with an **empty** candidate list.
        //
        // Baking the exit nodes into `peers` here would freeze them for
        // the lifetime of the instance: `update_exit_nodes` only replaces
        // the live list, it does not rebuild the table, and the L3 lookup
        // hits this rule (step 3.5) before it ever reaches the live list
        // (step 4). A hot-patched exit peer would then never be used.
        //
        // So the rule only records that it wants the exit nodes, and the
        // reconcile loop fills the candidates from the **live** list on
        // every tick — the same mechanism the explicit `/0` rule uses.
        //
        // `if_all_unavailable` matches the legacy step-4 behaviour: an
        // unreachable exit falls back to the physical gateway instead of
        // black-holing. That keeps the rule at one candidate too — a sugar
        // rule cannot be confirmed, so `closed` + several exit nodes would be
        // rejected by `validate_route_rule` anyway.
        let sugar = RouteRule {
            id: u32::MAX,
            cidr: Some(default_cidr),
            domains: Vec::new(),
            peers: Vec::new(),
            merge_exit_nodes: true,
            select_by: SelectBy::Priority,
            failover_when: FailoverWhen::Unreachable,
            if_all_unavailable: AllUnavailable::Open,
            require_confirmed: false,
            quality_degraded_cost: None,
        };
        if exit_nodes.len() > MAX_PEERS_PER_RULE {
            return Err(PolicyRouteError::Invalid(format!(
                "exit_nodes exceed the peer limit {MAX_PEERS_PER_RULE} ({} candidates)",
                exit_nodes.len()
            )));
        }
        validate_route_rule(&sugar)?;
        rules.push(sugar);
    }
    Ok(rules)
}

/// Candidate count of `rule` once the live `exit_nodes` are merged in.
pub fn merged_candidate_count(rule: &RouteRule, exit_nodes: &[IpAddr]) -> usize {
    merged_candidates(rule, exit_nodes).len()
}

/// The candidate list of `rule` with the live `exit_nodes` merged in —
/// explicit candidates first, exit nodes after them.
///
/// Only a rule that asked for it (`merge_exit_nodes`) gets the exit nodes;
/// every other rule keeps its own candidate list untouched.
pub fn merged_candidates(rule: &RouteRule, exit_nodes: &[IpAddr]) -> Vec<IpAddr> {
    let mut merged = rule.peers.clone();
    if rule.merge_exit_nodes {
        for node in exit_nodes {
            if !merged.contains(node) {
                merged.push(*node);
            }
        }
    }
    merged
}

/// Deserialized + pre-parsed policy route from config / runtime.
#[derive(Debug, Clone)]
pub struct ValidatedPolicyRoute {
    pub cidr: Option<Ipv4Cidr>,
    pub domains: Vec<DomainPattern>,
    pub peers: Vec<IpAddr>,
    pub select_by: SelectBy,
    pub failover_when: FailoverWhen,
    pub if_all_unavailable: AllUnavailable,
    pub require_confirmed: bool,
    pub quality_degraded_cost: Option<i32>,
}

impl ValidatedPolicyRoute {
    fn to_route_rule(&self, id: u32) -> RouteRule {
        RouteRule {
            id,
            cidr: self.cidr,
            domains: self.domains.clone(),
            peers: self.peers.clone(),
            merge_exit_nodes: false,
            select_by: self.select_by,
            failover_when: self.failover_when,
            if_all_unavailable: self.if_all_unavailable,
            require_confirmed: self.require_confirmed,
            quality_degraded_cost: self.quality_degraded_cost,
        }
    }
}

/// TOML / API shape: `[[policy_routes]]` (not legacy `routes = [CIDR…]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRouteConfig {
    #[serde(default)]
    pub cidr: Option<Ipv4Cidr>,
    #[serde(default)]
    pub domains: Vec<String>,
    pub peers: Vec<IpAddr>,
    pub select_by: String,
    pub failover_when: String,
    pub if_all_unavailable: String,
    #[serde(default)]
    pub require_confirmed: bool,
    /// P4: composite-cost ceiling for `failover_when = degraded`.
    #[serde(default)]
    pub quality_degraded_cost: Option<i32>,
}

impl PolicyRouteConfig {
    pub fn validate(&self) -> Result<ValidatedPolicyRoute, PolicyRouteError> {
        let domains = self
            .domains
            .iter()
            .map(|d| parse_domain_pattern(d))
            .collect::<Result<Vec<_>, _>>()?;
        let select_by = parse_select_by(&self.select_by)?;
        let failover_when = parse_failover_when(&self.failover_when)?;

        // `failover_when = "never"` means "never move to another candidate", so
        // the reconcile loop keeps **only the first** entry of `peers`. There is
        // then nothing left for `quality` or `balance` to choose from, and the
        // rule would silently behave exactly like `priority` — a configuration
        // that does not do what it says.
        //
        // The other combinations are load-bearing, so they must not be
        // rejected here: `quality` + `degraded` needs `quality_degraded_cost`
        // (checked below), and `balance` + `never` is caught by this rule.
        if failover_when == FailoverWhen::Never && select_by != SelectBy::Priority {
            return Err(PolicyRouteError::Invalid(format!(
                "failover_when=\"never\" keeps only the first candidate, so \
                 select_by=\"{}\" cannot take effect; use \
                 failover_when=\"unreachable\" (or \"degraded\") instead",
                self.select_by
            )));
        }

        Ok(ValidatedPolicyRoute {
            cidr: self.cidr,
            domains,
            peers: self.peers.clone(),
            select_by,
            failover_when,
            if_all_unavailable: parse_all_unavailable(&self.if_all_unavailable)?,
            require_confirmed: self.require_confirmed,
            quality_degraded_cost: self.quality_degraded_cost,
        })
    }
}

pub fn validate_policy_route_configs(
    configs: &[PolicyRouteConfig],
) -> Result<Vec<ValidatedPolicyRoute>, PolicyRouteError> {
    configs.iter().map(PolicyRouteConfig::validate).collect()
}

/// Validate + compile `[[policy_routes]]` (and optional `exit_nodes` sugar).
///
/// Shared by PeerManager construct and persist-only gates. Callers that keep the
/// empty-table zero-regression path must **not** invoke this when
/// `policy_routes` is empty solely to bake `exit_nodes` — that belongs on the
/// live `update_exit_nodes` / step-4 path.
pub fn validate_and_build_policy_rules(
    configs: &[PolicyRouteConfig],
    exit_nodes: &[IpAddr],
    dns_config: Option<&DnsConfig>,
    tld_dns_zone: &str,
) -> Result<Vec<RouteRule>, PolicyRouteError> {
    let validated = validate_policy_route_configs(configs)?;
    validate_domain_rules_against_dns(&validated, dns_config, tld_dns_zone)?;
    build_rules_from_config(&validated, exit_nodes)
}

/// Persist / run gate: no-op when there are no `[[policy_routes]]`.
///
/// `route_algo` is **not** a TOML field, so a config-layer gate cannot know it.
/// This variant therefore assumes the default (`ospf`, hence a route table),
/// which is right for the paths that *create* an instance from a config
/// (`ValidateConfig` / `RunNetworkInstance` / the wasm config helpers).
/// A gate that runs against an **existing** instance must call
/// [`validate_config_loader_policy_routes_for_algo`] with
/// `CoreInstance::has_route_table()` instead — see `config_patch`.
pub fn validate_config_loader_policy_routes(
    config: &impl crate::config::toml::ConfigLoader,
) -> Result<(), anyhow::Error> {
    validate_config_loader_policy_routes_for_algo(config, true)
}

/// Same gate, but told whether the instance will have a route table.
///
/// `route_algo` is **not** a TOML field — the config layer cannot express it, so
/// a plain [`validate_config_loader_policy_routes`] has to assume the default
/// (`ospf`, and therefore a route table). Two kinds of caller answer differently:
///
/// - **gates that run against an existing instance** (the config hot patch) know
///   the answer and must pass `CoreInstance::has_route_table()`, otherwise an
///   unpriced `quality` / `degraded` rule slips through onto an instance that
///   cannot rank by cost (L5);
/// - **gates that only have a config** (the wasm helpers and the
///   `ValidateConfig` / `RunNetworkInstance` RPCs) have nothing else to go on,
///   because an instance created from a `TomlConfig` always runs
///   `RouteAlgoType::Ospf`.
///
/// An embedder that builds `PortablePeerManagerConfig` with
/// `RouteAlgoType::None` is outside both: it only has the plain config gate, so
/// it must call this variant itself. The construction path is the backstop —
/// `build_local_route_table` refuses the same combination, so such a config
/// fails to start rather than running unpriced.
pub fn validate_config_loader_policy_routes_for_algo(
    config: &impl crate::config::toml::ConfigLoader,
    has_route_table: bool,
) -> Result<(), anyhow::Error> {
    let policy_routes = config.get_policy_routes();
    if policy_routes.is_empty() {
        return Ok(());
    }
    validate_route_algo_can_price_candidates(&policy_routes, has_route_table)
        .map_err(|error| anyhow::anyhow!("invalid policy_routes: {error}"))?;
    let zone = config.get_flags().tld_dns_zone;
    validate_and_build_policy_rules(
        &policy_routes,
        &config.get_exit_nodes(),
        config.get_dns_config().as_ref(),
        &zone,
    )
    .map_err(|error| anyhow::anyhow!("invalid policy_routes: {error}"))?;
    Ok(())
}

/// L5: reject policy rules that need a **cost** when there is no route table.
///
/// `select_by = quality` and `failover_when = degraded` both rank candidates by
/// their composite cost. That cost comes from the route table
/// (`Route::get_path_cost_with_policy` → `path_latency`), with the direct-link
/// measurement as a fallback. With `route_algo = none` there is neither:
///
/// - `DisabledRoute` keeps the trait's default `get_path_cost_with_policy` →
///   `None`, and
/// - the candidate resolution itself goes through the peer map's route list,
///   which is empty.
///
/// Every candidate therefore falls back to `i32::MAX`, so `degraded` drops all
/// of them (rule becomes `Degraded`) and `quality` ranks them all as equals.
/// The configuration would silently do something other than what it says, so it
/// is refused at load time instead — the same policy as the other hard-failure
/// checks.
///
/// Kept free of the `RouteAlgoType` type so this module has no dependency on the
/// peer manager; the caller passes the answer in.
pub fn validate_route_algo_can_price_candidates(
    configs: &[PolicyRouteConfig],
    has_route_table: bool,
) -> Result<(), PolicyRouteError> {
    if has_route_table {
        return Ok(());
    }
    for config in configs {
        // Parse instead of string-comparing so an invalid string is reported by
        // the normal validator (`PolicyRouteConfig::validate`) rather than
        // silently passing here and failing later with a different message.
        let Ok(select_by) = parse_select_by(&config.select_by) else {
            continue;
        };
        let Ok(failover_when) = parse_failover_when(&config.failover_when) else {
            continue;
        };
        if select_by != SelectBy::Quality && failover_when != FailoverWhen::Degraded {
            continue;
        }
        return Err(PolicyRouteError::Invalid(format!(
            "policy route on {:?}: select_by={:?} / failover_when={:?} rank candidates by \
             link cost, which needs a route table, but this instance runs with \
             route_algo = none. Every candidate would be unmeasurable (cost i32::MAX): \
             quality would rank them all equal and degraded would drop all of them. \
             Use route_algo = ospf, or switch to select_by = \"priority\" with \
             failover_when = \"unreachable\".",
            config.cidr, config.select_by, config.failover_when
        )));
    }
    Ok(())
}

/// P3 (§9.8.5 / C3): config-time precheck for `domains` rules.
///
/// MagicDNS guard rules silently drop static-host entries that collide with the
/// route TLD zone or a Split forwarder zone. A policy route whose domain hits
/// one of those guards would never learn an IP, so it is rejected here instead
/// of failing silently at runtime.
///
/// The `route zone same name` guard depends on peer-advertised zones (runtime
/// state) and is therefore **not** checked here.
pub fn validate_domain_rules_against_dns(
    routes: &[ValidatedPolicyRoute],
    // Kept for API stability / future DNS interaction checks. Split forwarder
    // overlap is intentionally allowed: ObservingDnsResolver learns IPs from
    // actual answers (including Split), unlike static-hosts Catalog ownership.
    _dns: Option<&DnsConfig>,
    tld_dns_zone: &str,
) -> Result<(), PolicyRouteError> {
    let tld = normalize_qname(tld_dns_zone);

    for route in routes {
        for pattern in &route.domains {
            let zone = match pattern {
                DomainPattern::Exact(name) => name.as_str(),
                DomainPattern::Wildcard(parent) => parent.as_str(),
            };
            if name_is_in_zone(zone, &tld) {
                return Err(PolicyRouteError::Invalid(format!(
                    "policy route domain {zone:?} falls inside the MagicDNS route TLD zone \
                     {tld:?}; MagicDNS answers it from route hostnames, so the rule would \
                     never learn an IP"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn rule_cidr(id: u32, cidr: &str, peers: &[&str]) -> RouteRule {
        RouteRule {
            id,
            cidr: Some(cidr.parse().unwrap()),
            domains: vec![],
            peers: peers.iter().map(|p| p.parse().unwrap()).collect(),
            merge_exit_nodes: false,
            select_by: SelectBy::Priority,
            failover_when: FailoverWhen::Unreachable,
            if_all_unavailable: AllUnavailable::Open,
            require_confirmed: false,
            quality_degraded_cost: None,
        }
    }

    fn hit_peer(table: &LocalRouteTable, ip: Ipv4Addr) -> Option<PeerId> {
        match table.lookup(&ip, None) {
            RouteLookup::Hit { peer, .. } => Some(peer),
            RouteLookup::Block | RouteLookup::Miss => None,
        }
    }

    #[test]
    fn lookup_lpm_prefers_longer_prefix() {
        let table = LocalRouteTable::empty();
        let mut snap = build_snapshot(vec![
            rule_cidr(0, "10.0.0.0/8", &["10.144.0.1"]),
            rule_cidr(1, "10.1.0.0/16", &["10.144.0.2"]),
        ]);
        // Simulate reconcile: set active peers
        snap.rules[0].active_peer = Some(1);
        snap.rules[0].state = RuleState::Stable;
        snap.rules[1].active_peer = Some(2);
        snap.rules[1].state = RuleState::Stable;
        table.store(snap);

        assert_eq!(hit_peer(&table, Ipv4Addr::new(10, 1, 2, 3)), Some(2));
        assert_eq!(hit_peer(&table, Ipv4Addr::new(10, 2, 0, 1)), Some(1));
    }

    #[test]
    fn lpm_buckets_handle_mixed_prefixes_and_miss() {
        let table = LocalRouteTable::empty();
        let mut snap = build_snapshot(vec![
            rule_cidr(0, "10.0.0.0/8", &["10.144.0.1"]),
            rule_cidr(1, "10.1.0.0/16", &["10.144.0.2"]),
            rule_cidr(2, "10.1.2.0/24", &["10.144.0.3"]),
        ]);
        for (idx, peer) in [(0usize, 10u32), (1, 20), (2, 30)] {
            snap.rules[idx].active_peer = Some(peer);
            snap.rules[idx].state = RuleState::Stable;
        }
        table.store(snap);

        // Longest prefix wins at every depth…
        assert_eq!(hit_peer(&table, Ipv4Addr::new(10, 1, 2, 5)), Some(30));
        assert_eq!(hit_peer(&table, Ipv4Addr::new(10, 1, 9, 9)), Some(20));
        assert_eq!(hit_peer(&table, Ipv4Addr::new(10, 9, 9, 9)), Some(10));
        // …and an address outside every bucket misses (walks all 33 buckets).
        assert_eq!(hit_peer(&table, Ipv4Addr::new(192, 168, 1, 1)), None);
    }

    #[test]
    fn duplicate_cidr_keeps_the_closed_rule() {
        // Two rules declare the same CIDR: `closed` wins, then the lower index.
        let table = LocalRouteTable::empty();
        let mut open = rule_cidr(0, "10.5.0.0/16", &["10.144.0.1"]);
        open.if_all_unavailable = AllUnavailable::Open;
        let mut closed = rule_cidr(1, "10.5.0.0/16", &["10.144.0.2"]);
        closed.if_all_unavailable = AllUnavailable::Closed;
        let mut snap = build_snapshot(vec![open, closed]);
        snap.rules[0].active_peer = Some(10);
        snap.rules[0].state = RuleState::Stable;
        snap.rules[1].active_peer = Some(20);
        snap.rules[1].state = RuleState::Stable;
        table.store(snap);

        assert_eq!(hit_peer(&table, Ipv4Addr::new(10, 5, 1, 1)), Some(20));
    }

    #[test]
    fn lookup_miss_when_degraded() {
        let table = LocalRouteTable::empty();
        let mut snap = build_snapshot(vec![rule_cidr(0, "192.0.2.0/24", &["10.144.0.1"])]);
        snap.rules[0].active_peer = Some(1);
        snap.rules[0].state = RuleState::Degraded;
        table.store(snap);
        // `if_all_unavailable = open` (the `rule_cidr` default) → fall through.
        assert_eq!(
            table.lookup(&Ipv4Addr::new(192, 0, 2, 10), None),
            RouteLookup::Miss
        );
    }

    #[test]
    fn require_confirmed_for_closed_multi_peer() {
        let mut r = rule_cidr(0, "0.0.0.0/0", &["10.144.0.1", "10.144.0.2"]);
        r.if_all_unavailable = AllUnavailable::Closed;
        assert!(validate_route_rule(&r).is_err());
        r.require_confirmed = true;
        assert!(validate_route_rule(&r).is_ok());
    }

    #[test]
    fn exit_nodes_sugar_builds_default_route() {
        let rules = build_rules_from_config(&[], &["10.144.0.9".parse().unwrap()]).unwrap();
        assert_eq!(rules.len(), 1);
        // The sugar rule is the `/0` one, marked by the reserved id.
        assert_eq!(rules[0].id, u32::MAX);
        assert_eq!(rules[0].cidr.unwrap().network_length(), 0);
    }

    #[test]
    fn domain_only_rule_is_accepted_in_p3() {
        let mut r = rule_cidr(0, "0.0.0.0/0", &["10.144.0.1"]);
        r.cidr = None;
        r.domains = vec![DomainPattern::Exact("example.com".into())];
        assert!(validate_route_rule(&r).is_ok());
    }

    #[test]
    fn rule_without_cidr_and_domains_is_rejected() {
        let mut r = rule_cidr(0, "0.0.0.0/0", &["10.144.0.1"]);
        r.cidr = None;
        assert!(validate_route_rule(&r).is_err());
    }

    #[test]
    fn observe_ips_populates_l3_and_l2() {
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "0.0.0.0/0", &["10.144.0.1"]);
        r.cidr = None;
        r.domains = vec![DomainPattern::Exact("api.example.com".into())];
        let mut snap = build_snapshot(vec![r]);
        snap.rules[0].active_peer = Some(1);
        snap.rules[0].state = RuleState::Stable;
        table.store(snap);
        let rev_after_store = table.revision();

        assert!(table.observe_ips("api.example.com", &["203.0.113.7".parse().unwrap()]));
        // L2 monitor skip gate must see this bump even when peer-info is idle.
        assert!(table.revision() > rev_after_store);
        // L3: the observed /32 resolves through the rule.
        assert_eq!(
            table.lookup(&"203.0.113.7".parse().unwrap(), None),
            RouteLookup::Hit { peer: 1 }
        );
        // L2: the very same snapshot contributes the CIDR.
        assert!(
            table
                .l2_contributing_cidrs()
                .contains(&"203.0.113.7/32".parse().unwrap())
        );
    }

    #[test]
    fn observe_ignores_unmatched_domains() {
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "0.0.0.0/0", &["10.144.0.1"]);
        r.cidr = None;
        r.domains = vec![DomainPattern::Exact("api.example.com".into())];
        table.store(build_snapshot(vec![r]));

        assert!(!table.observe_ips("other.example.com", &["203.0.113.7".parse().unwrap()]));
    }

    #[test]
    fn wildcard_domain_matches_subdomains_only() {
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "0.0.0.0/0", &["10.144.0.1"]);
        r.cidr = None;
        r.domains = vec![DomainPattern::Wildcard("atlassian.net".into())];
        table.store(build_snapshot(vec![r]));

        assert!(table.observe_ips("jira.atlassian.net", &["203.0.113.9".parse().unwrap()]));
        // The parent itself is not owned by `*.atlassian.net`.
        assert!(!table.observe_ips("atlassian.net", &["203.0.113.10".parse().unwrap()]));
        // Nor is a lookalike suffix without a label boundary.
        assert!(!table.observe_ips("notatlassian.net", &["203.0.113.11".parse().unwrap()]));
    }

    #[test]
    fn observed_ips_are_kept_and_capped() {
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "0.0.0.0/0", &["10.144.0.1"]);
        r.cidr = None;
        r.domains = vec![DomainPattern::Exact("api.example.com".into())];
        table.store(build_snapshot(vec![r]));

        for i in 0..(MAX_OBSERVED_IPS_PER_RULE + 4) {
            let ip = Ipv4Addr::new(203, 0, 113, i as u8);
            assert!(table.observe_ips("api.example.com", &[ip]));
        }
        let snap = table.load_full();
        assert_eq!(snap.rules[0].observed_ips.len(), MAX_OBSERVED_IPS_PER_RULE);
        // Oldest evicted, newest retained — never "revoked on TTL".
        assert!(
            !snap.rules[0]
                .observed_ips
                .contains(&Ipv4Addr::new(203, 0, 113, 0))
        );
        assert!(snap.rules[0].observed_ips.contains(&Ipv4Addr::new(
            203,
            0,
            113,
            (MAX_OBSERVED_IPS_PER_RULE + 3) as u8
        )));
    }

    #[test]
    fn observe_ips_publishes_when_only_deque_changes() {
        // Regression: learning an IP that is already the rule's explicit /32
        // must still persist it into `observed_ips`. Previously `changed` was
        // tied only to `compiled_cidrs.insert`, so the RCU closure returned the
        // old Arc and the deque update vanished.
        let table = LocalRouteTable::empty();
        let explicit: Ipv4Addr = "203.0.113.1".parse().unwrap();
        let mut r = rule_cidr(0, "203.0.113.1/32", &["10.144.0.1"]);
        r.domains = vec![DomainPattern::Exact("api.example.com".into())];
        table.store(build_snapshot(vec![r]));

        assert!(table.observe_ips("api.example.com", &[explicit]));
        let snap = table.load_full();
        assert!(
            snap.rules[0].observed_ips.contains(&explicit),
            "deque must record the IP even when compiled_cidrs already had it"
        );
    }

    #[test]
    fn fifo_eviction_keeps_explicit_slash32() {
        // A rule that both declares an explicit /32 and learns domain IPs must
        // not lose the configured CIDR when the same address ages out of the
        // observation deque.
        let table = LocalRouteTable::empty();
        let explicit: Ipv4Addr = "203.0.113.1".parse().unwrap();
        let mut r = rule_cidr(0, "203.0.113.1/32", &["10.144.0.1"]);
        r.domains = vec![DomainPattern::Exact("api.example.com".into())];
        table.store(build_snapshot(vec![r]));

        // Fill past the cap, starting with the explicit address so it is the
        // oldest observed entry when eviction begins.
        assert!(table.observe_ips("api.example.com", &[explicit]));
        for i in 2..=(MAX_OBSERVED_IPS_PER_RULE + 2) {
            let ip = Ipv4Addr::new(203, 0, 113, i as u8);
            assert!(table.observe_ips("api.example.com", &[ip]));
        }

        let snap = table.load_full();
        let explicit_cidr = Ipv4Cidr::new(explicit, 32).unwrap();
        assert!(
            snap.rules[0].compiled_cidrs.contains(&explicit_cidr),
            "explicit /32 must survive FIFO eviction of observed IPs"
        );
        assert!(
            snap.by_ip.contains_key(&explicit),
            "explicit /32 must remain in the L3 index after eviction"
        );
    }

    #[test]
    fn peer_updates_preserve_observed_ips() {
        // Regression: the reconcile pass publishes only `(idx, active_peer,
        // state)` through `apply_peer_updates`. It must land on the *latest*
        // snapshot so a concurrent `observe_ips` is not clobbered — otherwise a
        // freshly learned domain `/32` disappears until the next DNS answer.
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "0.0.0.0/0", &["10.144.0.1"]);
        r.cidr = None;
        r.domains = vec![DomainPattern::Exact("api.example.com".into())];
        table.store(build_snapshot(vec![r]));

        let learned: Ipv4Addr = "203.0.113.7".parse().unwrap();
        assert!(table.observe_ips("api.example.com", &[learned]));

        // Simulate the reconcile pass selecting the peer.
        table.apply_peer_updates(
            &HashMap::new(),
            &HashMap::new(),
            &[RuleUpdate {
                idx: 0,
                active_peer: Some(42),
                state: RuleState::Stable,
                challenger: None,
                resolved_peers: Vec::new(),
                pinned_peer: None,
                costed_peers: Vec::new(),
            }],
        );

        let snap = table.load_full();
        assert_eq!(snap.rules[0].observed_ips, vec![learned]);
        assert_eq!(snap.rules[0].active_peer, Some(42));
        assert_eq!(table.lookup(&learned, None), RouteLookup::Hit { peer: 42 });
    }

    #[test]
    fn exact_domain_beats_wildcard() {
        let table = LocalRouteTable::empty();
        let mut exact = rule_cidr(0, "0.0.0.0/0", &["10.144.0.1"]);
        exact.cidr = None;
        exact.domains = vec![DomainPattern::Exact("jira.atlassian.net".into())];
        let mut wild = rule_cidr(1, "0.0.0.0/0", &["10.144.0.2"]);
        wild.cidr = None;
        wild.domains = vec![DomainPattern::Wildcard("atlassian.net".into())];
        let mut snap = build_snapshot(vec![exact, wild]);
        snap.rules[0].active_peer = Some(1);
        snap.rules[0].state = RuleState::Stable;
        snap.rules[1].active_peer = Some(2);
        snap.rules[1].state = RuleState::Stable;
        table.store(snap);

        assert!(table.observe_ips("jira.atlassian.net", &["203.0.113.20".parse().unwrap()]));
        // The exact rule (peer 1) owns it, not the wildcard (peer 2).
        assert_eq!(
            table.lookup(&"203.0.113.20".parse().unwrap(), None),
            RouteLookup::Hit { peer: 1 }
        );
    }

    #[test]
    fn wildcard_syntax_validated() {
        assert!(parse_domain_pattern("*.atlassian.net").is_ok());
        // RFC 4592: the parent of `*.x` must own at least two labels.
        assert!(parse_domain_pattern("*.com").is_err());
        // `*` is only allowed as the leftmost label.
        assert!(parse_domain_pattern("a.*.com").is_err());
        // Names are normalized to lower case and stripped of a trailing dot.
        assert!(matches!(
            parse_domain_pattern("Example.COM.").unwrap(),
            DomainPattern::Exact(ref d) if d == "example.com"
        ));
    }

    #[test]
    fn name_zone_filter_matches_exact_and_subdomains_only() {
        assert!(name_is_in_zone("host.et.net", "et.net."));
        assert!(name_is_in_zone("Host.ET.Net.", "et.net"));
        assert!(name_is_in_zone("et.net", "et.net"));
        // A suffix that only shares characters must not match.
        assert!(!name_is_in_zone("notet.net", "et.net"));
        assert!(!name_is_in_zone("api.example.com", "et.net"));
        // An empty zone never swallows every name.
        assert!(!name_is_in_zone("api.example.com", ""));
        assert!(!name_is_in_zone("", ""));
    }

    fn dns_with_split(domains: &[&str]) -> DnsConfig {
        DnsConfig {
            forwarders: vec![crate::config::DnsForwarder {
                domains: domains.iter().map(|d| (*d).to_string()).collect(),
                servers: vec!["10.0.0.53".to_string()],
            }],
            ..Default::default()
        }
    }

    fn route_with_domains(domains: &[&str]) -> ValidatedPolicyRoute {
        PolicyRouteConfig {
            cidr: None,
            domains: domains.iter().map(|d| (*d).to_string()).collect(),
            peers: vec!["10.144.0.9".parse().unwrap()],
            select_by: "priority".to_string(),
            failover_when: "unreachable".to_string(),
            if_all_unavailable: "open".to_string(),
            require_confirmed: false,
            quality_degraded_cost: None,
        }
        .validate()
        .unwrap()
    }

    #[test]
    fn domain_precheck_rejects_route_tld_zone() {
        let routes = vec![route_with_domains(&["*.et.net"])];
        let error = validate_domain_rules_against_dns(&routes, None, "et.net.").unwrap_err();
        assert!(error.to_string().contains("TLD zone"));

        // An exact name inside the TLD zone is rejected too.
        let routes = vec![route_with_domains(&["node.et.net"])];
        assert!(validate_domain_rules_against_dns(&routes, None, "et.net.").is_err());

        // An unrelated domain passes.
        let routes = vec![route_with_domains(&["*.atlassian.net"])];
        assert!(validate_domain_rules_against_dns(&routes, None, "et.net.").is_ok());
    }

    #[test]
    fn domain_precheck_allows_wildcard_over_split_zone() {
        // Domain policy routes observe real answers (incl. Split). Unlike static
        // hosts, a wildcard rule + same-suffix Split forwarder is a valid combo.
        let dns = dns_with_split(&["atlassian.net"]);
        let routes = vec![route_with_domains(&["*.atlassian.net"])];
        assert!(validate_domain_rules_against_dns(&routes, Some(&dns), "").is_ok());

        let routes = vec![route_with_domains(&["app.atlassian.net"])];
        assert!(validate_domain_rules_against_dns(&routes, Some(&dns), "").is_ok());

        let routes = vec![route_with_domains(&["*.example.com"])];
        assert!(validate_domain_rules_against_dns(&routes, Some(&dns), "").is_ok());
    }

    #[test]
    fn exact_ip_wins_over_cidr_lpm() {
        let table = LocalRouteTable::empty();
        let mut snap = build_snapshot(vec![
            rule_cidr(0, "10.0.0.0/8", &["10.144.0.1"]),
            rule_cidr(1, "10.1.2.3/32", &["10.144.0.2"]),
        ]);
        snap.rules[0].active_peer = Some(1);
        snap.rules[0].state = RuleState::Stable;
        snap.rules[1].active_peer = Some(2);
        snap.rules[1].state = RuleState::Stable;
        table.store(snap);

        assert_eq!(hit_peer(&table, Ipv4Addr::new(10, 1, 2, 3)), Some(2));
    }

    #[test]
    fn empty_table_is_a_no_op() {
        let table = LocalRouteTable::empty();
        assert_eq!(
            table.lookup(&Ipv4Addr::new(8, 8, 8, 8), None),
            RouteLookup::Miss
        );
    }

    #[test]
    fn closed_degraded_rule_blocks_instead_of_falling_through() {
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "198.51.100.0/24", &["10.144.0.1"]);
        r.if_all_unavailable = AllUnavailable::Closed;
        let mut snap = build_snapshot(vec![r]);
        snap.rules[0].state = RuleState::Degraded;
        table.store(snap);

        assert_eq!(
            table.lookup(&Ipv4Addr::new(198, 51, 100, 7), None),
            RouteLookup::Block
        );
    }

    #[test]
    fn open_degraded_rule_misses_and_falls_through() {
        let table = LocalRouteTable::empty();
        let mut snap = build_snapshot(vec![rule_cidr(0, "198.51.100.0/24", &["10.144.0.1"])]);
        snap.rules[0].state = RuleState::Degraded;
        table.store(snap);
        assert_eq!(
            table.lookup(&Ipv4Addr::new(198, 51, 100, 7), None),
            RouteLookup::Miss
        );
    }

    #[test]
    fn l2_contributions_only_from_live_rules() {
        let table = LocalRouteTable::empty();
        let mut snap = build_snapshot(vec![
            rule_cidr(0, "10.0.0.0/8", &["10.144.0.1"]),
            rule_cidr(1, "172.16.0.0/12", &["10.144.0.2"]),
        ]);
        snap.rules[0].state = RuleState::Stable;
        snap.rules[0].active_peer = Some(1);
        snap.rules[1].state = RuleState::Degraded;
        table.store(snap);

        let cidrs = table.l2_contributing_cidrs();
        assert!(cidrs.contains(&"10.0.0.0/8".parse().unwrap()));
        // A Degraded rule must not pull traffic into the TUN.
        assert!(!cidrs.contains(&"172.16.0.0/12".parse().unwrap()));
    }

    #[test]
    fn failover_rules_still_contribute_to_l2() {
        let table = LocalRouteTable::empty();
        let mut snap = build_snapshot(vec![rule_cidr(0, "10.0.0.0/8", &["10.144.0.1"])]);
        snap.rules[0].state = RuleState::Failover;
        snap.rules[0].active_peer = Some(2);
        table.store(snap);

        assert!(
            table
                .l2_contributing_cidrs()
                .contains(&"10.0.0.0/8".parse().unwrap())
        );
    }

    #[test]
    fn select_by_and_failover_parse() {
        assert_eq!(parse_select_by("priority").unwrap(), SelectBy::Priority);
        assert_eq!(parse_select_by("quality").unwrap(), SelectBy::Quality);
        assert!(parse_select_by("fastest").is_err());
        assert_eq!(parse_failover_when("never").unwrap(), FailoverWhen::Never);
        assert_eq!(
            parse_failover_when("degraded").unwrap(),
            FailoverWhen::Degraded
        );
        assert!(parse_failover_when("sometimes").is_err());
        assert_eq!(
            parse_all_unavailable("closed").unwrap(),
            AllUnavailable::Closed
        );
    }

    fn config_with(select_by: &str, failover_when: &str, cost: Option<i32>) -> PolicyRouteConfig {
        PolicyRouteConfig {
            cidr: Some("10.0.0.0/24".parse().unwrap()),
            domains: vec![],
            peers: vec!["10.144.0.9".parse().unwrap()],
            select_by: select_by.to_string(),
            failover_when: failover_when.to_string(),
            if_all_unavailable: "open".to_string(),
            require_confirmed: false,
            quality_degraded_cost: cost,
        }
    }

    fn rule_from(select_by: &str, failover_when: &str, cost: Option<i32>) -> RouteRule {
        config_with(select_by, failover_when, cost)
            .validate()
            .expect("parse-level validation")
            .to_route_rule(0)
    }

    #[test]
    fn quality_and_degraded_are_accepted_in_p4() {
        // P4: quality selection and the degraded ceiling are now legal.
        let rule = rule_from("quality", "unreachable", None);
        assert_eq!(rule.select_by, SelectBy::Quality);
        assert!(validate_route_rule(&rule).is_ok());

        let rule = rule_from("priority", "degraded", Some(500));
        assert_eq!(rule.failover_when, FailoverWhen::Degraded);
        assert_eq!(rule.quality_degraded_cost, Some(500));
        assert!(validate_route_rule(&rule).is_ok());

        // P5: balance is now accepted too.
        let rule = rule_from("balance", "unreachable", None);
        assert_eq!(rule.select_by, SelectBy::Balance);
        assert!(validate_route_rule(&rule).is_ok());
    }

    #[test]
    fn degraded_ceiling_must_pair_with_degraded_failover() {
        // degraded without a ceiling cannot decide anything.
        assert!(validate_route_rule(&rule_from("priority", "degraded", None)).is_err());
        // A non-positive ceiling is meaningless.
        assert!(validate_route_rule(&rule_from("priority", "degraded", Some(0))).is_err());
        // A ceiling without `degraded` is dead weight.
        assert!(validate_route_rule(&rule_from("quality", "unreachable", Some(500))).is_err());
    }

    #[test]
    fn degraded_cost_threshold_semantics() {
        // Only `degraded` consults the ceiling; above it counts as degraded.
        assert!(is_degraded_cost(FailoverWhen::Degraded, Some(500), 501));
        assert!(!is_degraded_cost(FailoverWhen::Degraded, Some(500), 500));
        assert!(!is_degraded_cost(
            FailoverWhen::Unreachable,
            Some(500),
            9_999
        ));
        assert!(!is_degraded_cost(FailoverWhen::Degraded, None, 9_999));
    }

    #[test]
    fn switch_band_keeps_the_incumbent_on_noise() {
        let cfg = ConnSelectConfig::default();
        // Better or equal incumbent always stays.
        assert!(within_switch_band(100, 100, cfg));
        assert!(within_switch_band(90, 100, cfg));
        // Within the relative margin (10%) stays…
        assert!(within_switch_band(108, 100, cfg));
        // …and beyond it switches.
        assert!(!within_switch_band(130, 100, cfg));
        // The absolute margin (5 cost units) covers near-zero scores.
        assert!(within_switch_band(5, 2, cfg));
    }

    #[test]
    fn pick_balance_is_stable_and_even() {
        let peers = vec![10u32, 20, 30];
        // One flow key → one candidate, always.
        assert_eq!(pick_balance(Some(7), &peers), pick_balance(Some(7), &peers));
        assert_eq!(pick_balance(Some(7), &peers), Some(20)); // 7 % 3 == 1
        // Every candidate is used across keys (no starvation).
        let hits: BTreeSet<u32> = (0..9)
            .filter_map(|key| pick_balance(Some(key), &peers))
            .collect();
        assert_eq!(hits, BTreeSet::from([10, 20, 30]));
        // No flow key degrades to the first slot; an empty list yields nothing.
        assert_eq!(pick_balance(None, &peers), Some(10));
        assert_eq!(pick_balance(Some(3), &[]), None);
    }

    #[test]
    fn balance_rule_spreads_flows_on_hot_path() {
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1"]);
        r.select_by = SelectBy::Balance;
        table.store(build_snapshot(vec![r]));
        // The reconcile pass publishes the candidate list for balance rules.
        table.apply_peer_updates(
            &HashMap::new(),
            &HashMap::new(),
            &[RuleUpdate {
                idx: 0,
                active_peer: Some(10),
                state: RuleState::Stable,
                challenger: None,
                resolved_peers: vec![10, 20],
                pinned_peer: None,
                costed_peers: Vec::new(),
            }],
        );

        let ip: Ipv4Addr = "10.0.0.5".parse().unwrap();
        // The same flow keeps the same egress…
        assert_eq!(table.lookup(&ip, Some(4)), RouteLookup::Hit { peer: 10 });
        assert_eq!(table.lookup(&ip, Some(4)), table.lookup(&ip, Some(4)));
        // …while a different flow takes the other candidate.
        assert_eq!(table.lookup(&ip, Some(5)), RouteLookup::Hit { peer: 20 });
    }

    #[test]
    fn degraded_balance_rule_honours_closed() {
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1"]);
        r.select_by = SelectBy::Balance;
        r.if_all_unavailable = AllUnavailable::Closed;
        table.store(build_snapshot(vec![r]));
        // No candidates published → Degraded → `closed` blocks instead of leaking.
        assert_eq!(
            table.lookup(&"10.0.0.5".parse().unwrap(), Some(1)),
            RouteLookup::Block
        );
    }

    /// A `balance` rule with several candidates is picked by the flow key, so
    /// a caller that has no flow key cannot know which peer owns the flow.
    ///
    /// Without the flow key `lookup` answers for slot 0 — a peer that may not
    /// be the one that will actually carry the packet. `is_balance_ambiguous`
    /// is how such a caller finds out that it must not make that assumption.
    #[test]
    fn balance_rule_with_several_candidates_is_ambiguous() {
        let table = LocalRouteTable::empty();
        let mut snap = build_snapshot(vec![rule_cidr(
            0,
            "10.0.0.0/24",
            &["10.144.0.1", "10.144.0.2"],
        )]);
        snap.rules[0].rule.select_by = SelectBy::Balance;
        snap.rules[0].resolved_peers = vec![10, 20];
        snap.rules[0].active_peer = Some(10);
        snap.rules[0].state = RuleState::Stable;
        table.store(snap);

        let ip: Ipv4Addr = "10.0.0.5".parse().unwrap();
        assert!(table.is_balance_ambiguous(&ip));
        // The flow key really does decide the peer …
        assert_eq!(table.lookup(&ip, Some(4)), RouteLookup::Hit { peer: 10 });
        assert_eq!(table.lookup(&ip, Some(5)), RouteLookup::Hit { peer: 20 });
        // … and without one, the answer is slot 0 — the wrong peer half the
        // time, which is exactly why the ambiguity has to be reported.
        assert_eq!(table.lookup(&ip, None), RouteLookup::Hit { peer: 10 });
    }

    /// A `priority` rule is decided by the reconcile loop, not by the flow key,
    /// so it is never ambiguous.
    #[test]
    fn priority_rule_is_never_ambiguous() {
        let table = LocalRouteTable::empty();
        let mut snap = build_snapshot(vec![rule_cidr(
            0,
            "10.0.0.0/24",
            &["10.144.0.1", "10.144.0.2"],
        )]);
        snap.rules[0].active_peer = Some(10);
        snap.rules[0].state = RuleState::Stable;
        table.store(snap);

        assert!(!table.is_balance_ambiguous(&"10.0.0.5".parse().unwrap()));
    }

    /// L3: `failover_when = never` keeps routing to its pinned peer after that
    /// peer became unreachable.
    ///
    /// "Never fail over" constrains **which** peer carries the traffic, not
    /// whether the rule survives: flipping to `Degraded` would hand the flow to
    /// the physical gateway (`open`) or drop it (`closed`) — both of which are
    /// egresses the user ruled out. The pin is what makes the option mean what
    /// it says.
    #[test]
    fn a_pinned_never_rule_keeps_routing_to_its_peer_while_it_is_down() {
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1"]);
        r.failover_when = FailoverWhen::Never;
        table.store(build_snapshot(vec![r]));

        let ip: Ipv4Addr = "10.0.0.5".parse().unwrap();
        // Before the first reconcile there is nothing pinned yet…
        assert_eq!(table.lookup(&ip, None), RouteLookup::Miss);

        // …then the peer was reachable and got pinned.
        table.apply_peer_updates(
            &HashMap::new(),
            &HashMap::new(),
            &[RuleUpdate {
                idx: 0,
                active_peer: Some(10),
                state: RuleState::Stable,
                challenger: None,
                resolved_peers: Vec::new(),
                pinned_peer: Some(PinnedPeer::new(10, None)),
                costed_peers: Vec::new(),
            }],
        );
        assert_eq!(table.lookup(&ip, None), RouteLookup::Hit { peer: 10 });

        // The peer now drops out: the rule is Degraded, but the pin still
        // answers — no fall-through to the gateway, no block.
        table.apply_peer_updates(
            &HashMap::new(),
            &HashMap::new(),
            &[RuleUpdate {
                idx: 0,
                active_peer: None,
                state: RuleState::Degraded,
                challenger: None,
                resolved_peers: Vec::new(),
                pinned_peer: Some(PinnedPeer::new(10, None)),
                costed_peers: Vec::new(),
            }],
        );
        assert_eq!(table.lookup(&ip, None), RouteLookup::Hit { peer: 10 });
        // L2 must keep the CIDR installed, otherwise the packet never reaches
        // L3 in the first place and the pin is moot.
        assert!(
            table
                .l2_contributing_cidrs()
                .contains(&"10.0.0.0/24".parse().unwrap()),
            "a pinned rule must keep its L2 route while its peer is down"
        );
    }

    /// The pin only covers the `never` rule: without it, a `Degraded` rule
    /// keeps the documented behaviour (`open` → fall through, `closed` → drop).
    #[test]
    fn a_degraded_rule_without_a_pin_still_falls_through_or_blocks() {
        let table = LocalRouteTable::empty();
        let r = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1"]);
        table.store(build_snapshot(vec![r]));
        let ip: Ipv4Addr = "10.0.0.5".parse().unwrap();
        assert_eq!(table.lookup(&ip, None), RouteLookup::Miss);
        assert!(table.l2_contributing_cidrs().is_empty());
    }

    /// A `never` rule that never resolved a peer has nothing to pin, so it
    /// keeps the ordinary `Degraded` behaviour instead of inventing a target.
    #[test]
    fn a_never_rule_without_a_resolved_peer_is_still_degraded() {
        let table = LocalRouteTable::empty();
        let mut r = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1"]);
        r.failover_when = FailoverWhen::Never;
        r.if_all_unavailable = AllUnavailable::Closed;
        table.store(build_snapshot(vec![r]));

        assert_eq!(
            table.lookup(&"10.0.0.5".parse().unwrap(), None),
            RouteLookup::Block,
            "no pin + closed must not silently fall through"
        );
    }

    fn policy_config(select_by: &str, failover_when: &str) -> PolicyRouteConfig {
        PolicyRouteConfig {
            cidr: Some("10.0.0.0/24".parse().unwrap()),
            domains: vec![],
            peers: vec![
                "10.144.0.9".parse().unwrap(),
                "10.144.0.10".parse().unwrap(),
            ],
            select_by: select_by.to_owned(),
            failover_when: failover_when.to_owned(),
            if_all_unavailable: "open".to_owned(),
            require_confirmed: false,
            quality_degraded_cost: None,
        }
    }

    /// `failover_when = "never"` keeps only the first candidate, so there is
    /// nothing left for `quality` or `balance` to choose from — the rule would
    /// silently behave like `priority`.
    ///
    /// The configuration must be rejected instead of accepted and quietly
    /// doing something else.
    #[test]
    fn failover_never_with_a_non_priority_select_by_is_rejected() {
        assert!(policy_config("quality", "never").validate().is_err());
        assert!(policy_config("balance", "never").validate().is_err());
        // `priority` is the only mode that still means something there.
        assert!(policy_config("priority", "never").validate().is_ok());
        // The other combinations stay legal.
        assert!(policy_config("quality", "unreachable").validate().is_ok());
        assert!(policy_config("balance", "unreachable").validate().is_ok());
    }

    fn validated_policy(cidr: Option<&str>, peers: &[&str]) -> ValidatedPolicyRoute {
        ValidatedPolicyRoute {
            cidr: cidr.map(|c| c.parse().unwrap()),
            domains: vec![],
            peers: peers.iter().map(|p| p.parse().unwrap()).collect(),
            select_by: SelectBy::Priority,
            failover_when: FailoverWhen::Unreachable,
            if_all_unavailable: AllUnavailable::Open,
            require_confirmed: false,
            quality_degraded_cost: None,
        }
    }

    /// The exit nodes must join the **candidate list** of an explicit default
    /// route rule, not become a second `/0` rule.
    ///
    /// The LPM index keeps one entry per (prefix, network) and drops the later
    /// rule, so a second `/0` rule never reached the lookup path and the exit
    /// nodes were silently inert whenever an explicit `/0` rule existed.
    #[test]
    fn exit_nodes_merge_into_an_explicit_default_route_rule() {
        let policy = vec![validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"])];
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];

        let rules = build_rules_from_config(&policy, &exit_nodes).expect("rules");
        assert_eq!(
            rules.len(),
            1,
            "the exit nodes must not become a second /0 rule"
        );
        assert!(
            rules[0].merge_exit_nodes,
            "the explicit default-route rule must take the exit nodes"
        );
        // The merge happens at reconcile time, not here — the rule keeps its
        // own candidates so a hot-patched exit list is not frozen.
        assert_eq!(
            rules[0].peers,
            vec!["10.144.0.9".parse::<IpAddr>().unwrap()],
            "the rule must not bake the exit nodes into its own candidate list"
        );
    }

    /// The merge must follow the **live** exit nodes, so a hot-patched list
    /// takes effect without rebuilding the rule.
    #[test]
    fn the_merged_candidates_follow_the_live_exit_nodes() {
        let policy = vec![validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"])];
        let rules = build_rules_from_config(&policy, &["10.144.0.10".parse().unwrap()]).unwrap();
        let rule = &rules[0];

        let first = merged_candidates(rule, &["10.144.0.10".parse().unwrap()]);
        assert_eq!(
            first,
            vec![
                "10.144.0.9".parse::<IpAddr>().unwrap(),
                "10.144.0.10".parse::<IpAddr>().unwrap()
            ],
            "explicit candidates first, exit nodes after them"
        );

        // The hot-patched list replaces the old one — no rebuild involved.
        let second = merged_candidates(rule, &["10.144.0.11".parse().unwrap()]);
        assert_eq!(
            second,
            vec![
                "10.144.0.9".parse::<IpAddr>().unwrap(),
                "10.144.0.11".parse::<IpAddr>().unwrap()
            ],
            "the merge must follow the live exit nodes"
        );
    }

    /// A rule that does not ask for the merge must never see the exit nodes.
    ///
    /// Only the **default route** takes them: the `exit_nodes` are the
    /// fallback for "everything else", so a narrower rule (here `/8`) must not
    /// silently inherit them.
    #[test]
    fn a_rule_without_the_merge_flag_keeps_its_own_candidates() {
        let policy = vec![validated_policy(Some("10.0.0.0/8"), &["10.144.0.9"])];
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];
        let rules = build_rules_from_config(&policy, &exit_nodes).unwrap();
        for rule in &rules {
            if rule.cidr == Some(ipv4_default_cidr()) {
                // The default route is the one that takes them.
                continue;
            }
            assert!(!rule.merge_exit_nodes);
            assert_eq!(merged_candidates(rule, &exit_nodes), rule.peers);
        }
    }

    /// The `exit_nodes` sugar rule must take its candidates from the **live**
    /// list, not from the one that existed when the table was built.
    ///
    /// `update_exit_nodes` only replaces the live list — it does not rebuild the
    /// table — and the L3 lookup reaches this rule (step 3.5) before the live
    /// list (step 4). Baking the exit nodes into `peers` would therefore freeze
    /// a hot-patched exit peer for the lifetime of the instance.
    #[test]
    fn the_exit_nodes_sugar_rule_follows_the_live_exit_nodes() {
        let policy = vec![validated_policy(Some("10.0.0.0/8"), &["10.144.0.9"])];
        let rules = build_rules_from_config(&policy, &["10.144.0.10".parse().unwrap()]).unwrap();

        let sugar = rules
            .iter()
            .find(|r| r.cidr == Some(ipv4_default_cidr()))
            .expect("the exit nodes get their own default-route rule");
        assert!(
            sugar.merge_exit_nodes,
            "the sugar rule must take the live exit nodes"
        );
        assert!(
            sugar.peers.is_empty(),
            "the sugar rule must not bake the exit nodes in: {:?}",
            sugar.peers
        );

        // The candidates come from the live list, whatever it holds now.
        assert_eq!(
            merged_candidates(sugar, &["10.144.0.10".parse().unwrap()]),
            vec!["10.144.0.10".parse::<IpAddr>().unwrap()]
        );
        // … and a hot-patched list replaces the old one, with no rebuild.
        assert_eq!(
            merged_candidates(sugar, &["10.144.0.11".parse().unwrap()]),
            vec!["10.144.0.11".parse::<IpAddr>().unwrap()]
        );
    }

    /// L6: the management-plane view must show the **merged** candidate list
    /// (config peers + live exit_nodes), including the sugar `/0` rule.
    #[test]
    fn effective_policy_routes_show_merged_candidates_and_sugar() {
        let policy = validated_policy(Some("10.0.0.0/24"), &["10.144.0.9"]);
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];
        // No explicit `/0` ⇒ sugar rule is synthesized for the exit nodes.
        let rules = build_rules_from_config(&[policy], &exit_nodes).unwrap();
        let mut snap = build_snapshot(rules);
        snap.rules[0].active_peer = Some(9);
        snap.rules[0].state = RuleState::Stable;
        snap.rules[1].active_peer = Some(10);
        snap.rules[1].state = RuleState::Stable;

        let views = collect_effective_policy_routes(&snap, &exit_nodes);
        assert_eq!(views.len(), 2);

        assert!(!views[0].sugar);
        assert_eq!(views[0].config_peers, vec!["10.144.0.9".to_string()]);
        assert_eq!(
            views[0].effective_candidates,
            vec!["10.144.0.9".to_string()]
        );
        assert!(!views[0].merge_exit_nodes);

        assert!(views[1].sugar);
        assert!(views[1].config_peers.is_empty());
        assert!(views[1].merge_exit_nodes);
        assert_eq!(
            views[1].effective_candidates,
            vec!["10.144.0.10".to_string()]
        );
        assert_eq!(views[1].cidr.as_deref(), Some("0.0.0.0/0"));
    }

    /// A default-route rule that also carries `domains` would hand the exit
    /// nodes to the **domain** candidates as well. That silently widens what
    /// the rule means, so it is refused instead of guessed.
    #[test]
    fn a_default_route_rule_with_domains_cannot_be_combined_with_exit_nodes() {
        let mut policy = validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"]);
        policy.domains = vec![parse_domain_pattern("*.example.com").unwrap()];
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];

        let err = build_rules_from_config(&[policy], &exit_nodes)
            .expect_err("a domain-carrying default route must not take the exit nodes");
        assert!(
            err.to_string().contains("domains"),
            "unexpected error: {err}"
        );
    }

    /// `"never"` keeps only the **first** candidate, so a rule that takes the
    /// exit nodes could never fail over to them — the exit nodes would be
    /// merged in and then never selected. That is the very "configured but
    /// silently ineffective" shape B19 rejects for `select_by`.
    #[test]
    fn merging_exit_nodes_into_a_never_rule_is_rejected() {
        let mut policy = validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"]);
        policy.failover_when = FailoverWhen::Never;
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];

        let err = build_rules_from_config(&[policy], &exit_nodes).expect_err(
            "\"never\" keeps only the first candidate, so the exit nodes would never be used",
        );
        assert!(err.to_string().contains("never"), "unexpected error: {err}");
    }

    /// The strategy table is built **once** and never rebuilt, so the
    /// "take the exit nodes" flag must not depend on whether `exit_nodes`
    /// happens to be non-empty at construction time.
    ///
    /// Otherwise an instance that starts without an exit node and gets one
    /// through `config_patch` would never route through it: the rule would
    /// not be marked, and the live list (step 4) is never reached because the
    /// default-route rule (step 3.5) answers first.
    #[test]
    fn the_default_route_takes_exit_nodes_even_when_there_are_none_yet() {
        let policy = vec![validated_policy(Some("10.0.0.0/8"), &["10.144.0.9"])];
        let rules = build_rules_from_config(&policy, &[]).expect("rules");

        let sugar = rules
            .iter()
            .find(|r| r.cidr == Some(ipv4_default_cidr()))
            .expect("the default route must exist even without exit nodes");
        assert!(
            sugar.merge_exit_nodes,
            "the flag must be set unconditionally, or a later exit node is invisible"
        );
        // A rule that already has an explicit default route is marked too.
        let policy = vec![validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"])];
        let rules = build_rules_from_config(&policy, &[]).expect("rules");
        assert_eq!(rules.len(), 1);
        assert!(
            rules[0].merge_exit_nodes,
            "an explicit default route must take exit nodes added later"
        );
    }

    /// `never` keeps only the first candidate, so a rule that takes the exit
    /// nodes could never fail over to them. Checked **unconditionally** — the
    /// exit nodes may be added by a later `config_patch`, and then the merge
    /// would silently stop working.
    #[test]
    fn a_never_default_route_is_rejected_even_without_exit_nodes() {
        let mut policy = validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"]);
        policy.failover_when = FailoverWhen::Never;
        let err = build_rules_from_config(&[policy], &[])
            .expect_err("never + a default route that takes exit nodes must be rejected");
        assert!(err.to_string().contains("never"), "unexpected error: {err}");
    }

    /// The hot-patch path validates a `TomlConfig` directly (it implements
    /// `ConfigLoader`), so the combination check has to work on that type too
    /// — otherwise the GUI / RPC could save a configuration the run path would
    /// have rejected.
    #[test]
    fn a_toml_config_with_an_illegal_policy_route_is_rejected() {
        use crate::config::toml::{ConfigLoader as _, TomlConfig};

        let cfg = TomlConfig::default();
        cfg.set_policy_routes(vec![PolicyRouteConfig {
            cidr: Some("10.0.0.0/8".parse().unwrap()),
            domains: vec![],
            peers: vec!["10.144.0.9".parse().unwrap()],
            select_by: "quality".to_owned(),
            failover_when: "never".to_owned(),
            if_all_unavailable: "open".to_owned(),
            require_confirmed: false,
            quality_degraded_cost: None,
        }]);

        let err = validate_config_loader_policy_routes(&cfg)
            .expect_err("never + quality must be rejected on the hot-patch path too");
        assert!(
            err.to_string().contains("select_by"),
            "unexpected error: {err}"
        );
    }

    /// A single-candidate `closed` rule is legal; merging an exit node into it
    /// makes it multi-peer, which then needs the explicit confirmation.
    ///
    /// Companion to `merging_exit_nodes_into_a_closed_rule_requires_confirmation`
    /// (which covers the rejection); this one covers that the confirmed
    /// configuration still loads.
    #[test]
    fn a_confirmed_closed_rule_still_loads_with_the_exit_nodes_merged_in() {
        let mut policy = validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"]);
        policy.if_all_unavailable = AllUnavailable::Closed;
        policy.require_confirmed = true;
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];

        let rules = build_rules_from_config(&[policy], &exit_nodes)
            .expect("a confirmed closed rule must load");
        assert!(rules[0].merge_exit_nodes);
        assert_eq!(
            merged_candidate_count(&rules[0], &exit_nodes),
            2,
            "the exit node is part of the candidate list"
        );
    }

    /// The merged rule must be reachable from the lookup, and the L2
    /// contributing set must agree with it.
    #[test]
    fn the_merged_default_route_rule_is_reachable_and_agrees_with_l2() {
        let policy = vec![validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"])];
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];

        let rules = build_rules_from_config(&policy, &exit_nodes).expect("rules");
        let mut snap = build_snapshot(rules);
        snap.rules[0].active_peer = Some(10);
        snap.rules[0].state = RuleState::Stable;
        let table = LocalRouteTable::empty();
        table.store(snap);

        assert_eq!(
            table.lookup(&"8.8.8.8".parse().unwrap(), None),
            RouteLookup::Hit { peer: 10 },
            "the merged rule must own the default route"
        );
        assert_eq!(
            table.l2_contributing_cidrs(),
            BTreeSet::from([ipv4_default_cidr()]),
            "L2 must agree with L3: one rule, one answer"
        );
    }

    /// Without an explicit default route the exit nodes still get their own
    /// rule — the merge must not change that path.
    ///
    /// The rule carries **no** candidates of its own; it takes them from the
    /// live `exit_nodes` at reconcile time (see
    /// `the_exit_nodes_sugar_rule_follows_the_live_exit_nodes`).
    #[test]
    fn exit_nodes_without_an_explicit_default_route_keep_their_own_rule() {
        let policy = vec![validated_policy(Some("10.0.0.0/8"), &["10.144.0.9"])];
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];

        let rules = build_rules_from_config(&policy, &exit_nodes).expect("rules");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1].cidr, Some(ipv4_default_cidr()));
        assert!(
            rules[1].peers.is_empty(),
            "the sugar rule must not bake the exit nodes in"
        );
        assert!(rules[1].merge_exit_nodes);
        assert_eq!(
            merged_candidates(&rules[1], &exit_nodes),
            exit_nodes,
            "the sugar rule's candidates are the live exit nodes"
        );
    }

    /// More candidates than the limit is a hard error, not a silent
    /// truncation — a dropped candidate is exactly the kind of surprise this
    /// whole change is about.
    #[test]
    fn merging_more_candidates_than_the_limit_is_rejected() {
        let peers: Vec<String> = (0..MAX_PEERS_PER_RULE)
            .map(|i| format!("10.144.0.{i}"))
            .collect();
        let peer_refs: Vec<&str> = peers.iter().map(|s| s.as_str()).collect();
        let policy = vec![validated_policy(Some("0.0.0.0/0"), &peer_refs)];
        let exit_nodes = vec!["10.200.0.1".parse::<IpAddr>().unwrap()];

        let err = build_rules_from_config(&policy, &exit_nodes)
            .expect_err("more candidates than the limit must be rejected");
        assert!(
            err.to_string().contains("exceed"),
            "unexpected error: {err}"
        );
    }

    /// A single-peer `closed` rule is legal; merging an exit node into it makes
    /// it multi-peer and must then require `require_confirmed`.
    #[test]
    fn merging_exit_nodes_into_a_closed_rule_requires_confirmation() {
        let mut policy = validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"]);
        policy.if_all_unavailable = AllUnavailable::Closed;
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];

        let err = build_rules_from_config(&[policy], &exit_nodes)
            .expect_err("closed + multi-peer after merge must be rejected");
        assert!(
            err.to_string().contains("require_confirmed"),
            "unexpected error: {err}"
        );
    }

    /// L1: the merge must be validated against the list the rule will
    /// **actually walk**, not just the configured `peers`.
    ///
    /// A `balance` rule spreads flows over every merged candidate, so each exit
    /// node is an independent black hole once `closed` is set. Before this
    /// check the constructor accepted the rule (one configured peer) and the
    /// data plane silently started hashing flows onto a second egress that the
    /// user had never confirmed.
    #[test]
    fn a_balance_closed_rule_cannot_gain_exit_nodes_without_confirmation() {
        let mut policy = validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"]);
        policy.select_by = SelectBy::Balance;
        policy.if_all_unavailable = AllUnavailable::Closed;
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];

        let err = build_rules_from_config(&[policy.clone()], &exit_nodes)
            .expect_err("balance + closed + exit node must be rejected");
        assert!(
            err.to_string().contains("require_confirmed"),
            "unexpected error: {err}"
        );

        // Confirming it is enough — the rule itself was never illegal.
        policy.require_confirmed = true;
        let rules = build_rules_from_config(&[policy], &exit_nodes).expect("confirmed rule");
        // One rule: the explicit `/0` absorbs the exit nodes.
        assert_eq!(rules.len(), 1);
        assert!(rules[0].merge_exit_nodes);
        assert_eq!(
            merged_candidates(&rules[0], &exit_nodes).len(),
            2,
            "the balance pool is the merged list"
        );
    }

    /// **Decision A (2026-10-20)**: the `closed` confirmation counts the
    /// **effective** candidate list for every `select_by`, not just `balance`.
    ///
    /// `priority` / `quality` treat the exit nodes as a failover *behind* the
    /// configured peers rather than as a spread pool, but the consequence of
    /// not asking is the same: a configuration that says "one gateway, drop if
    /// it is down" silently becomes "one gateway plus a fallback" — and the
    /// user never agreed to a second egress, nor to what happens when it dies.
    ///
    /// This is deliberately the **stricter** of the two options: a previously
    /// loadable config (one peer + exit nodes, no `require_confirmed`) now
    /// fails to load until it is confirmed. That is the accepted trade — see
    /// the L1 row in `routing-bug-audit-2026-10.md`.
    #[test]
    fn a_priority_closed_rule_also_needs_confirmation_once_it_takes_exit_nodes() {
        let mut policy = validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"]);
        policy.if_all_unavailable = AllUnavailable::Closed;
        assert_eq!(policy.select_by, SelectBy::Priority);
        let exit_nodes = vec!["10.144.0.10".parse::<IpAddr>().unwrap()];

        let err = build_rules_from_config(&[policy.clone()], &exit_nodes)
            .expect_err("a failover candidate is still a second egress");
        assert!(
            err.to_string().contains("require_confirmed"),
            "unexpected error: {err}"
        );

        policy.require_confirmed = true;
        let rules = build_rules_from_config(&[policy], &exit_nodes).expect("confirmed rule");
        assert_eq!(merged_candidate_count(&rules[0], &exit_nodes), 2);
    }

    /// A `priority` default route whose exit nodes resolve to the peer it
    /// already names stays a **one-candidate** rule: the merge is overlap
    /// aware, so the `closed` confirmation guard must not fire on it.
    ///
    /// Guards the L1 check against being over-applied — the same exit VIP
    /// listed in `peers` is not a second egress.
    #[test]
    fn a_closed_rule_whose_exit_node_is_already_a_configured_peer_stays_single() {
        let mut policy = validated_policy(Some("0.0.0.0/0"), &["10.144.0.9"]);
        policy.if_all_unavailable = AllUnavailable::Closed;
        let exit_nodes = vec!["10.144.0.9".parse::<IpAddr>().unwrap()];

        let rules = build_rules_from_config(&[policy], &exit_nodes)
            .expect("an overlapping exit node is not a second candidate");
        assert_eq!(rules.len(), 1);
        assert_eq!(
            merged_candidate_count(&rules[0], &exit_nodes),
            1,
            "the overlap must be deduplicated, not counted twice"
        );
    }

    /// L5: cost-based selection needs a route table. Without one every
    /// candidate is unmeasurable (`i32::MAX`), so `quality` would rank them all
    /// equal and `degraded` would drop all of them.
    #[test]
    fn cost_based_selection_is_rejected_without_a_route_table() {
        let mut quality = policy_config("quality", "unreachable");
        quality.cidr = None;
        quality.domains = vec!["api.example.com".to_owned()];

        let mut degraded = policy_config("priority", "degraded");
        degraded.cidr = None;
        degraded.domains = vec!["api.example.com".to_owned()];
        degraded.quality_degraded_cost = Some(500);

        for config in [quality, degraded] {
            let err = validate_route_algo_can_price_candidates(&[config], false)
                .expect_err("a cost-based rule cannot work without a route table");
            assert!(
                err.to_string().contains("route_algo = none"),
                "the error must name the cause: {err}"
            );
        }
    }

    /// The same rules load fine when the route table exists, and rules that
    /// never consult a cost are unaffected.
    #[test]
    fn cost_based_selection_is_accepted_with_a_route_table() {
        let config = policy_config("quality", "unreachable");
        assert!(validate_route_algo_can_price_candidates(&[config], true).is_ok());

        // `priority` / `unreachable` never read a cost, so they stay legal even
        // without a route table.
        let priority = policy_config("priority", "unreachable");
        assert!(validate_route_algo_can_price_candidates(&[priority], false).is_ok());
    }

    /// An unparsable value is left to `PolicyRouteConfig::validate` so the
    /// user gets the single, specific error instead of this coarser one.
    #[test]
    fn an_invalid_select_by_is_not_masked_by_the_route_algo_check() {
        let config = policy_config("bestest", "unreachable");
        assert!(validate_route_algo_can_price_candidates(&[config], false).is_ok());
    }

    /// L2: the two cost families are stored with their source, so the
    /// management plane can say whether a number is an accumulated path weight
    /// or a local first-hop measurement.
    #[test]
    fn peer_costs_carry_their_source_to_the_management_plane() {
        let mut r = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1", "10.144.0.2"]);
        r.select_by = SelectBy::Quality;
        let table = LocalRouteTable::empty();
        table.store(build_snapshot(vec![r]));

        let quality = HashMap::from([(10, 120), (20, 340)]);
        let sources = HashMap::from([(10, CostSource::Path), (20, CostSource::Edge)]);
        table.apply_peer_updates(
            &quality,
            &sources,
            &[RuleUpdate {
                idx: 0,
                active_peer: Some(10),
                state: RuleState::Stable,
                challenger: None,
                resolved_peers: Vec::new(),
                pinned_peer: None,
                costed_peers: vec![
                    CostedPeer {
                        peer_id: 10,
                        cost: 120,
                        source: CostSource::Path,
                    },
                    CostedPeer {
                        peer_id: 20,
                        cost: 340,
                        source: CostSource::Edge,
                    },
                ],
            }],
        );

        let views = collect_effective_policy_routes(&table.load_full(), &[]);
        assert_eq!(
            views[0].peer_costs,
            vec![
                PeerCostInfo {
                    peer_id: 10,
                    cost: 120,
                    source: CostSource::Path
                },
                PeerCostInfo {
                    peer_id: 20,
                    cost: 340,
                    source: CostSource::Edge
                },
            ],
            "both the number and its family must reach the status page"
        );
    }

    /// A rule that never priced a peer must not report it — even when the
    /// shared memo holds a `0` written for a different rule.
    #[test]
    fn rules_that_ignore_cost_report_no_peer_costs() {
        let r = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1"]);
        assert_eq!(r.select_by, SelectBy::Priority);
        let table = LocalRouteTable::empty();
        table.store(build_snapshot(vec![r]));
        let quality = HashMap::from([(10, 0)]);
        table.apply_peer_updates(
            &quality,
            &HashMap::new(),
            &[RuleUpdate {
                idx: 0,
                active_peer: Some(10),
                state: RuleState::Stable,
                challenger: None,
                resolved_peers: Vec::new(),
                pinned_peer: None,
                // A `priority` rule prices nothing.
                costed_peers: Vec::new(),
            }],
        );

        let views = collect_effective_policy_routes(&table.load_full(), &[]);
        assert!(
            views[0].peer_costs.is_empty(),
            "a priority rule must not surface a placeholder as a cost"
        );
    }

    /// L2 (review finding): a rule reports only the peers **it** priced.
    ///
    /// Two `quality` rules with disjoint candidate sets share one memo. Before
    /// the per-rule list, each rule showed the other's candidates on the status
    /// page — and a `priority` rule's placeholder `0` for a shared peer came
    /// out as a real `0 · path` measurement.
    #[test]
    fn a_rule_reports_only_the_costs_it_priced_itself() {
        let mut rule_a = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1"]);
        rule_a.select_by = SelectBy::Quality;
        let mut rule_b = rule_cidr(1, "10.1.0.0/24", &["10.144.0.2"]);
        rule_b.select_by = SelectBy::Quality;
        let table = LocalRouteTable::empty();
        table.store(build_snapshot(vec![rule_a, rule_b]));

        // Shared memo holds both rules' candidates, plus a placeholder `0` a
        // `priority` rule wrote for unrelated peer 30 (no source recorded).
        let quality = HashMap::from([(10, 120), (20, 340), (30, 0)]);
        let sources = HashMap::from([(10, CostSource::Path), (20, CostSource::Path)]);
        table.apply_peer_updates(
            &quality,
            &sources,
            &[
                RuleUpdate {
                    idx: 0,
                    active_peer: Some(10),
                    state: RuleState::Stable,
                    challenger: None,
                    resolved_peers: Vec::new(),
                    pinned_peer: None,
                    costed_peers: vec![CostedPeer {
                        peer_id: 10,
                        cost: 120,
                        source: CostSource::Path,
                    }],
                },
                RuleUpdate {
                    idx: 1,
                    active_peer: Some(20),
                    state: RuleState::Stable,
                    challenger: None,
                    resolved_peers: Vec::new(),
                    pinned_peer: None,
                    costed_peers: vec![CostedPeer {
                        peer_id: 20,
                        cost: 340,
                        source: CostSource::Path,
                    }],
                },
            ],
        );

        let views = collect_effective_policy_routes(&table.load_full(), &[]);
        assert_eq!(
            views[0]
                .peer_costs
                .iter()
                .map(|c| c.peer_id)
                .collect::<Vec<_>>(),
            vec![10],
            "rule A must not see rule B's candidate"
        );
        assert_eq!(
            views[1]
                .peer_costs
                .iter()
                .map(|c| c.peer_id)
                .collect::<Vec<_>>(),
            vec![20],
            "rule B must not see rule A's candidate"
        );
        assert!(
            views
                .iter()
                .all(|v| v.peer_costs.iter().all(|c| c.peer_id != 30)),
            "an unmeasured placeholder must never appear as a cost"
        );
    }

    /// A peer a rule did not price is not reported by that rule — even when the
    /// shared memo holds a real number for it (R4).
    ///
    /// This is the ordering-dependent leak: `apply_peer_updates` rewrites the
    /// whole memo on every pass, so any peer another rule priced this tick is
    /// present in `peer_quality`. Resolving `costed_peers` against the map at
    /// view time cannot tell "this rule priced it" from "somebody priced it".
    /// The reconcile loop therefore records the numbers per rule, and a rule
    /// that reads no cost leaves its list empty.
    #[test]
    fn a_rule_does_not_inherit_a_cost_another_rule_measured() {
        let r = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1"]);
        assert_eq!(r.select_by, SelectBy::Priority);
        let table = LocalRouteTable::empty();
        table.store(build_snapshot(vec![r]));
        table.apply_peer_updates(
            // Peer 10 **was** measured this tick by a `quality` rule elsewhere,
            // so the shared memo really holds 120 from `path`.
            &HashMap::from([(10, 120)]),
            &HashMap::from([(10, CostSource::Path)]),
            &[RuleUpdate {
                idx: 0,
                active_peer: Some(10),
                state: RuleState::Stable,
                challenger: None,
                resolved_peers: Vec::new(),
                pinned_peer: None,
                // …but this `priority` rule consulted no cost.
                costed_peers: Vec::new(),
            }],
        );

        let views = collect_effective_policy_routes(&table.load_full(), &[]);
        assert!(
            views[0].peer_costs.is_empty(),
            "a rule that reads no cost must not display one, however the memo was filled"
        );
    }

    /// The reported number is the one the rule saw, not whatever the shared memo
    /// holds by the time the status page reads it (R4).
    #[test]
    fn peer_costs_report_the_number_the_rule_priced_with() {
        let mut r = rule_cidr(0, "10.0.0.0/24", &["10.144.0.1"]);
        r.select_by = SelectBy::Quality;
        let table = LocalRouteTable::empty();
        table.store(build_snapshot(vec![r]));
        table.apply_peer_updates(
            &HashMap::from([(10, 120)]),
            &HashMap::from([(10, CostSource::Path)]),
            &[RuleUpdate {
                idx: 0,
                active_peer: Some(10),
                state: RuleState::Stable,
                challenger: None,
                resolved_peers: Vec::new(),
                pinned_peer: None,
                costed_peers: vec![CostedPeer {
                    peer_id: 10,
                    cost: 120,
                    source: CostSource::Path,
                }],
            }],
        );
        // A later tick re-baselines the memo (source switch + new number).
        table.apply_peer_updates(
            &HashMap::from([(10, 900)]),
            &HashMap::from([(10, CostSource::Edge)]),
            &[],
        );

        let views = collect_effective_policy_routes(&table.load_full(), &[]);
        assert_eq!(
            views[0].peer_costs,
            vec![PeerCostInfo {
                peer_id: 10,
                cost: 120,
                source: CostSource::Path
            }],
            "the per-rule record must survive a memo-only update"
        );
    }

    /// The cost source survives a reconcile pass, so the next tick can tell
    /// whether the previous number is a valid hysteresis baseline.
    #[test]
    fn cost_source_is_persisted_for_the_next_reconcile_tick() {
        let table = LocalRouteTable::empty();
        table.store(build_snapshot(vec![rule_cidr(
            0,
            "10.0.0.0/24",
            &["10.144.0.1"],
        )]));
        table.apply_peer_updates(
            &HashMap::from([(10, 500)]),
            &HashMap::from([(10, CostSource::Edge)]),
            &[],
        );

        let snap = table.load_full();
        assert_eq!(snap.peer_quality.get(&10), Some(&500));
        assert_eq!(
            snap.peer_cost_source.get(&10),
            Some(&CostSource::Edge),
            "without the source the next tick cannot tell the families apart"
        );
    }
}
