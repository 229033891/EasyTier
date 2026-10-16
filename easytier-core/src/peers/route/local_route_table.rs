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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailoverWhen {
    Unreachable,
    Degraded,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AllUnavailable {
    Open,
    Closed,
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
    pub active_peer: Option<PeerId>,
    /// P4: consecutive reconcile ticks the challenger has beaten the incumbent
    /// by more than the switch band. Switch only once it reaches
    /// `ConnSelectConfig::switch_windows`; reset whenever the incumbent holds.
    pub challenger: Option<(PeerId, u32)>,
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
            RuleState::Degraded => match r.rule.if_all_unavailable {
                AllUnavailable::Closed => RouteLookup::Block,
                AllUnavailable::Open => RouteLookup::Miss,
            },
            // Defensive: a consistent reconcile always pairs `None` with
            // `Degraded`. If a writer race ever left Stable/Failover without a
            // peer, honour `closed` instead of silently falling through.
            RuleState::Stable | RuleState::Failover => {
                // P5: balance spreads flows over the candidate list; everything
                // else uses the single reconciled `active_peer`.
                let peer = if r.rule.select_by == SelectBy::Balance {
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

    /// CIDRs this table contributes to the L2 desired set.
    ///
    /// Only `Stable` / `Failover` rules count: a `Degraded` rule must **not**
    /// pull traffic into the TUN (that is exactly the black-hole case §7.3
    /// warns about).
    pub fn l2_contributing_cidrs(&self) -> BTreeSet<Ipv4Cidr> {
        let snap = self.snapshot.load();
        let mut out = BTreeSet::new();
        for r in &snap.rules {
            if matches!(r.state, RuleState::Stable | RuleState::Failover) {
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
    pub fn apply_peer_updates(&self, peer_quality: &HashMap<PeerId, i32>, updates: &[RuleUpdate]) {
        if updates.is_empty() && peer_quality.is_empty() {
            return;
        }
        let mut l2_state_changed = false;
        self.snapshot.rcu(|old| {
            l2_state_changed = false;
            let mut any = old.peer_quality != *peer_quality;
            for update in updates {
                let Some(r) = old.rules.get(update.idx) else {
                    continue;
                };
                if r.active_peer != update.active_peer
                    || r.state != update.state
                    || r.challenger != update.challenger
                    || r.resolved_peers != update.resolved_peers
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
            for update in updates {
                let Some(r) = new.rules.get_mut(update.idx) else {
                    continue;
                };
                r.active_peer = update.active_peer;
                r.state = update.state;
                r.challenger = update.challenger;
                r.resolved_peers.clone_from(&update.resolved_peers);
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
            }
        })
        .collect();

    let mut snap = LocalRouteSnapshot {
        rules: resolved,
        ..Default::default()
    };
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
    if rule.peers.is_empty() {
        return Err(PolicyRouteError::Invalid(
            "policy route needs at least one peer".into(),
        ));
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
    if rule.if_all_unavailable == AllUnavailable::Closed
        && rule.peers.len() > 1
        && !rule.require_confirmed
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
    if !exit_nodes.is_empty() {
        let sugar = RouteRule {
            id: u32::MAX,
            cidr: Some(ipv4_default_cidr()),
            domains: Vec::new(),
            peers: exit_nodes.to_vec(),
            select_by: SelectBy::Priority,
            failover_when: FailoverWhen::Unreachable,
            if_all_unavailable: AllUnavailable::Open,
            require_confirmed: false,
            quality_degraded_cost: None,
        };
        validate_route_rule(&sugar)?;
        rules.push(sugar);
    }
    Ok(rules)
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
        Ok(ValidatedPolicyRoute {
            cidr: self.cidr,
            domains,
            peers: self.peers.clone(),
            select_by: parse_select_by(&self.select_by)?,
            failover_when: parse_failover_when(&self.failover_when)?,
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
pub fn validate_config_loader_policy_routes(
    config: &impl crate::config::toml::ConfigLoader,
) -> Result<(), anyhow::Error> {
    let policy_routes = config.get_policy_routes();
    if policy_routes.is_empty() {
        return Ok(());
    }
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
            &[RuleUpdate {
                idx: 0,
                active_peer: Some(42),
                state: RuleState::Stable,
                challenger: None,
                resolved_peers: Vec::new(),
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
            &[RuleUpdate {
                idx: 0,
                active_peer: Some(10),
                state: RuleState::Stable,
                challenger: None,
                resolved_peers: vec![10, 20],
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
}
