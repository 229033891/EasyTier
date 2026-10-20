//! PeerConn bonding Phase 2a: diversity-first member set + per-flow hash send.
//!
//! See `docs/roadmap/multi-link-bonding.md`. Factory default `bond_count=2`
//! enables a small multi-path set; `1` keeps the single `default_conn` path.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
#[cfg(any(feature = "proxy-smoltcp-stack", test))]
use std::net::Ipv4Addr;

use easytier_proto::common::FlagsInConfig;

use super::conn_select::{ConnSelectConfig, ScoredConn};
use super::peer_conn::{PeerConn, PeerConnId};

/// Runtime hard cap for `peer_link_bond_count` (roadmap suggested max 5).
pub const BOND_COUNT_HARD_CAP: u32 = 5;

/// Extra live tunnels (out-of-band hot standby) tolerated beyond `bond_count`
/// while filling the bond set.
///
/// Out-of-band paths never join the send set, so counting them as "fill
/// progress" would let a permanently worse path occupy a live slot and stop
/// fill forever. This slack lets fill keep dialing past `bond_count`; it is
/// bounded so a thin quality band cannot spin dialing indefinitely.
pub const BOND_STANDBY_SLACK: u32 = 2;

/// Live PeerConn ceiling while filling (`bond_count` + standby slack).
///
/// Distinct from [`BOND_COUNT_HARD_CAP`]: the send set stays ≤ 5, but fill may
/// hold a few out-of-band standbys on top so `N=5` still has room to keep
/// dialing for another in-band member.
pub const BOND_LIVE_HARD_CAP: u32 = BOND_COUNT_HARD_CAP + BOND_STANDBY_SLACK;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BondConfig {
    /// Target bond set size. 1 = disabled (single default_conn).
    pub bond_count: u32,
    /// Max members sharing the same diversity class when filling.
    pub replica_fill_max: u32,
}

impl Default for BondConfig {
    fn default() -> Self {
        Self {
            bond_count: 2,
            replica_fill_max: 2,
        }
    }
}

impl BondConfig {
    /// Resolve from flags. `0` on either field means built-in default.
    pub fn from_flags(flags: &FlagsInConfig) -> Self {
        let mut bond_count = if flags.peer_link_bond_count == 0 {
            2
        } else {
            flags.peer_link_bond_count
        };
        bond_count = bond_count.clamp(1, BOND_COUNT_HARD_CAP);

        let replica_fill_max = if flags.peer_link_replica_fill_max == 0 {
            2
        } else {
            flags.peer_link_replica_fill_max.max(1)
        };

        Self {
            bond_count,
            replica_fill_max,
        }
    }

    pub fn enabled(&self) -> bool {
        self.bond_count > 1
    }
}

/// Diversity class for Phase 2a: scheme + remote (underlay exit is Phase 3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiversityClass {
    pub scheme: String,
    pub remote: String,
}

#[derive(Debug, Clone)]
pub struct BondCandidate {
    pub scored: ScoredConn,
    pub class: DiversityClass,
}

fn normalize_scheme(scheme: &str) -> String {
    let s = scheme.trim().to_ascii_lowercase();
    // Collapse "udp" / "UDP" / composite prefixes kept as-is for diversity.
    if s.is_empty() {
        "unknown".to_owned()
    } else {
        s
    }
}

fn normalize_remote(remote: &str) -> String {
    let r = remote.trim().to_ascii_lowercase();
    if r.is_empty() {
        "unknown".to_owned()
    } else {
        r
    }
}

pub fn diversity_class(scheme: &str, remote: &str) -> DiversityClass {
    DiversityClass {
        scheme: normalize_scheme(scheme),
        remote: normalize_remote(remote),
    }
}

/// Diversity class of one live connection, shared by member selection and
/// status reporting so both agree on what "same class" means.
pub(crate) fn conn_diversity_class(conn: &PeerConn) -> DiversityClass {
    let info = conn.get_conn_info();
    let scheme = info
        .tunnel
        .as_ref()
        .map(|t| t.display_tunnel_type())
        .unwrap_or_else(|| conn.tunnel_type().unwrap_or("unknown").to_owned());
    let remote = info
        .tunnel
        .as_ref()
        .and_then(|t| t.display_remote_addr())
        .unwrap_or_default();
    diversity_class(&scheme, &remote)
}

/// Whether this peer still needs more tunnels for bonding
/// (`bond_count` target not yet reached by live conn count).
///
/// Counts every live PeerConn, including out-of-band hot standby. Prefer
/// [`bond_fill_needed_with_standby`] when the bond send-set size is known —
/// a standby path must not be mistaken for fill progress.
pub fn bond_fill_needed(bond: &BondConfig, live_conns: usize) -> bool {
    bond.enabled() && (live_conns as u32) < bond.bond_count
}

/// Live-conn ceiling while filling the bond set: `bond_count` plus standby
/// slack, capped at [`BOND_LIVE_HARD_CAP`] (not the send-set hard cap).
pub fn bond_fill_live_ceiling(bond: &BondConfig) -> usize {
    bond.bond_count
        .saturating_add(BOND_STANDBY_SLACK)
        .min(BOND_LIVE_HARD_CAP) as usize
}

/// Whether another tunnel should be dialed to fill the bond set.
///
/// `live_conns` counts all live PeerConns (in-band members + out-of-band
/// standby); `bond_set_len` is the current send-set size (see `pick_bond_set`).
///
/// - Fewer live conns than `bond_count` → the send set cannot be full yet: fill.
/// - Live conns reach `bond_count` but the send set is smaller (some live paths
///   are out-of-band standby) → keep filling while under
///   [`bond_fill_live_ceiling`], so a worse path cannot block fill forever.
/// - At the ceiling, stop: everything dialed so far is out-of-band, and further
///   attempts would only add standby tunnels.
pub fn bond_fill_needed_with_standby(
    bond: &BondConfig,
    live_conns: usize,
    bond_set_len: usize,
) -> bool {
    if bond_fill_needed(bond, live_conns) {
        return true;
    }
    if !bond.enabled() {
        return false;
    }
    live_conns < bond_fill_live_ceiling(bond) && bond_set_len < bond.bond_count as usize
}

/// Whether `score` is in the same quality band as `best` (lower is better).
pub fn in_quality_band(score: f64, best: f64, cfg: ConnSelectConfig) -> bool {
    if score <= best {
        return true;
    }
    let delta = score - best;
    if best <= f64::EPSILON {
        return delta <= cfg.switch_abs_margin;
    }
    let rel = delta / best;
    rel <= cfg.switch_margin || delta <= cfg.switch_abs_margin
}

fn eligible_for_bond(scored: &[BondCandidate]) -> Vec<&BondCandidate> {
    if scored.is_empty() {
        return Vec::new();
    }
    let has_verified = scored.iter().any(|c| !c.scored.unverified_hole_punch);
    let has_unfused = scored
        .iter()
        .any(|c| !c.scored.unverified_hole_punch && !c.scored.fused);

    scored
        .iter()
        .filter(|c| {
            if has_verified && c.scored.unverified_hole_punch {
                return false;
            }
            if has_unfused && c.scored.fused {
                return false;
            }
            true
        })
        .collect()
}

/// Total order for candidates: score first, conn id as deterministic tie-break.
///
/// Input order comes from DashMap iteration (randomized); without the id
/// tie-break, equal-score candidates would resolve differently on every
/// rebuild and flap the flow→member mapping.
fn cmp_candidate_score_then_id(a: &BondCandidate, b: &BondCandidate) -> std::cmp::Ordering {
    a.scored
        .score
        .total_cmp(&b.scored.score)
        .then_with(|| a.scored.conn_id.cmp(&b.scored.conn_id))
}

/// Pick up to `bond.bond_count` member ids: quality band → diversity → replica fill.
///
/// **Quality-first:** only peers in the same quality band as the best eligible
/// candidate may join the bond send set. Paths outside the band stay alive as
/// hot standby (failover / probe) but must not share payload with better paths.
/// Replica-fill never pulls out-of-band candidates just to reach `N`.
///
/// Returns ordered member ids (best-first among early diversity picks). Empty if no candidates.
pub fn pick_bond_set(
    candidates: &[BondCandidate],
    bond: BondConfig,
    cfg: ConnSelectConfig,
) -> Vec<PeerConnId> {
    let n = bond.bond_count.max(1) as usize;
    let replica_max = bond.replica_fill_max.max(1) as usize;

    let eligible = eligible_for_bond(candidates);
    if eligible.is_empty() {
        return Vec::new();
    }

    let best_score = eligible
        .iter()
        .map(|c| c.scored.score)
        .min_by(|a, b| a.total_cmp(b))
        .unwrap_or(0.0);

    let mut band: Vec<&BondCandidate> = eligible
        .iter()
        .copied()
        .filter(|c| in_quality_band(c.scored.score, best_score, cfg))
        .collect();
    // `band` holds `&BondCandidate`, so sort_by sees `&&BondCandidate`.
    band.sort_by(|a, b| cmp_candidate_score_then_id(a, b));

    let mut selected: Vec<PeerConnId> = Vec::with_capacity(n.min(band.len().max(1)));
    let mut class_count: HashMap<DiversityClass, usize> = HashMap::new();
    let mut covered_schemes: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut covered_remotes: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut used: std::collections::HashSet<PeerConnId> = std::collections::HashSet::new();

    // Pass 1: diversity-first within quality band (prefer new scheme/remote).
    loop {
        if selected.len() >= n {
            break;
        }
        let mut best_pick: Option<&BondCandidate> = None;
        let mut best_gain: u8 = 0;
        for c in &band {
            if used.contains(&c.scored.conn_id) {
                continue;
            }
            let count = *class_count.get(&c.class).unwrap_or(&0);
            if count >= replica_max {
                continue;
            }
            let gain = (!covered_schemes.contains(&c.class.scheme) as u8)
                + (!covered_remotes.contains(&c.class.remote) as u8);
            if gain == 0 {
                continue;
            }
            // Higher gain wins; tie → better score (lower is better),
            // then smaller conn id so DashMap order never decides.
            let take = match best_pick {
                None => true,
                Some(prev) => {
                    gain > best_gain
                        || (gain == best_gain
                            && cmp_candidate_score_then_id(c, prev) == std::cmp::Ordering::Less)
                }
            };
            if take {
                best_gain = gain;
                best_pick = Some(c);
            }
        }
        let Some(pick) = best_pick else {
            break;
        };
        used.insert(pick.scored.conn_id);
        selected.push(pick.scored.conn_id);
        *class_count.entry(pick.class.clone()).or_insert(0) += 1;
        covered_schemes.insert(pick.class.scheme.clone());
        covered_remotes.insert(pick.class.remote.clone());
    }

    // Pass 2: replica-fill within the quality band only (never out-of-band).
    // Out-of-band live PeerConns remain as hot standby outside the bond set.
    for c in &band {
        if selected.len() >= n {
            break;
        }
        if used.contains(&c.scored.conn_id) {
            continue;
        }
        let count = *class_count.get(&c.class).unwrap_or(&0);
        if count >= replica_max {
            continue;
        }
        used.insert(c.scored.conn_id);
        selected.push(c.scored.conn_id);
        *class_count.entry(c.class.clone()).or_insert(0) += 1;
    }

    selected
}

/// Per-(flow, conn) hash used to pick a bond member by consistent hashing.
pub(crate) fn bond_flow_to_conn_hash(flow_key: u64, conn_id: PeerConnId) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    flow_key.hash(&mut hasher);
    conn_id.hash(&mut hasher);
    hasher.finish()
}

/// Pick the bond member for `flow_key` by **consistent hashing over conn ids**.
///
/// **Why not `flow_key % member_count`**: that hashes the member's *position*, so
/// any change to the member set — a member dying, replica fill, `bond_count`
/// change, or `pick_bond_set` re-ordering after a quality shift — can re-point an
/// *unrelated* flow to a different conn. Two independent paths have different
/// latency, and that reordering is seen as packet loss by the inner TCP / RDP
/// stream (S4 stutter).
///
/// Hashing `(flow_key, conn_id)` and taking the minimum keeps a flow on its conn
/// until **its own** member leaves the set — the standard consistent-hashing
/// property — while keeping the load spreading bonding exists for: a single
/// elephant flow still benefits from several parallel paths.
///
/// **Deliberately does NOT force the best-scoring member.** Bonding exists to
/// aggregate throughput; collapsing it to "always the best path" would delete
/// that capability, which is the same mistake as lowering `bond_count` to 1
/// (see [`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md)
/// §2.2: quality is the user-facing goal, aggregation is an orthogonal throughput
/// win). Quality still governs *membership* via `pick_bond_set`.
///
/// **`#[cfg(test)]`-only on purpose.** The hot path in `peer.rs` inlines the same
/// hash-and-take-minimum over an already-materialised member list, precisely so it
/// need not build a `Vec<PeerConnId>` per packet. This slice-taking form exists
/// as the readable reference that the tests assert against — the same arrangement
/// as [`bond_member_index`] below. If it is ever needed in production again, drop
/// the gate rather than re-introducing the per-packet allocation.
#[cfg(test)]
pub fn bond_member_for_flow(flow_key: u64, member_ids: &[PeerConnId]) -> Option<usize> {
    let mut best_idx: Option<usize> = None;
    let mut best_hash = u64::MAX;
    for (idx, conn_id) in member_ids.iter().enumerate() {
        let h = bond_flow_to_conn_hash(flow_key, *conn_id);
        if h < best_hash {
            best_hash = h;
            best_idx = Some(idx);
        }
    }
    best_idx
}

/// Stable index into `member_count` for a flow key.
///
/// Position-based; kept only as a reference for the S4 remapping test.
#[cfg(test)]
pub fn bond_member_index(flow_key: u64, member_count: usize) -> usize {
    if member_count == 0 {
        return 0;
    }
    (flow_key % member_count as u64) as usize
}

/// Hash inner IP 5-tuple when payload looks like IPv4/IPv6; else hash a payload prefix.
pub fn flow_key_from_payload(payload: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Some(key) = parse_ip_flow_key(payload) {
        key.hash(&mut hasher);
    } else {
        // Fallback: sticky-ish hash of payload head (non-IP / RPC / tests).
        payload
            .get(..32.min(payload.len()))
            .unwrap_or(payload)
            .hash(&mut hasher);
        payload.len().hash(&mut hasher);
    }
    hasher.finish()
}

/// Same hash as [`flow_key_from_payload`] for a known IPv4 5-tuple.
///
/// Used when the caller has socket endpoints but not a serialized IP packet
/// yet (e.g. data-plane TCP connect before the first segment is built). Must
/// stay bit-identical to the key derived from an IPv4 packet with the same
/// fields — otherwise a `balance` rule would judge the bypass against one peer
/// and then send the flow to another.
#[cfg(any(feature = "proxy-smoltcp-stack", test))]
pub fn flow_key_from_ipv4_tuple(
    proto: u8,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut src_b = [0u8; 16];
    let mut dst_b = [0u8; 16];
    src_b[12..16].copy_from_slice(&src.octets());
    dst_b[12..16].copy_from_slice(&dst.octets());
    let key: IpFlowKey = (proto, src_b, dst_b, src_port, dst_port);
    key.hash(&mut hasher);
    hasher.finish()
}

type IpFlowKey = (u8, [u8; 16], [u8; 16], u16, u16);

fn parse_ip_flow_key(payload: &[u8]) -> Option<IpFlowKey> {
    if payload.is_empty() {
        return None;
    }
    let version = payload[0] >> 4;
    match version {
        4 => {
            if payload.len() < 20 {
                return None;
            }
            let ihl = (payload[0] & 0x0f) as usize * 4;
            if ihl < 20 || payload.len() < ihl {
                return None;
            }
            let proto = payload[9];
            let mut src = [0u8; 16];
            let mut dst = [0u8; 16];
            src[12..16].copy_from_slice(&payload[12..16]);
            dst[12..16].copy_from_slice(&payload[16..20]);
            let (sp, dp) = transport_ports(proto, &payload[ihl..]);
            Some((proto, src, dst, sp, dp))
        }
        6 => {
            if payload.len() < 40 {
                return None;
            }
            let proto = payload[6];
            let mut src = [0u8; 16];
            let mut dst = [0u8; 16];
            src.copy_from_slice(&payload[8..24]);
            dst.copy_from_slice(&payload[24..40]);
            let (sp, dp) = transport_ports(proto, &payload[40..]);
            Some((proto, src, dst, sp, dp))
        }
        _ => None,
    }
}

fn transport_ports(proto: u8, transport: &[u8]) -> (u16, u16) {
    // TCP=6 UDP=17; others → 0,0 (still sticky on IP+proto).
    if (proto == 6 || proto == 17) && transport.len() >= 4 {
        let sp = u16::from_be_bytes([transport[0], transport[1]]);
        let dp = u16::from_be_bytes([transport[2], transport[3]]);
        (sp, dp)
    } else {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::conn::conn_select::{ConnMetrics, conn_quality_score, score_conn};
    use uuid::Uuid;

    fn id(n: u8) -> PeerConnId {
        Uuid::from_bytes([n; 16])
    }

    fn cand(
        n: u8,
        latency_us: u64,
        loss: f32,
        scheme: &str,
        remote: &str,
        hole: bool,
    ) -> BondCandidate {
        let cfg = ConnSelectConfig::default();
        let m = ConnMetrics {
            conn_id: id(n),
            latency_us,
            loss_rate: loss,
            jitter_us: 0,
            is_hole_punched: hole,
        };
        BondCandidate {
            scored: score_conn(&m, cfg),
            class: diversity_class(scheme, remote),
        }
    }

    #[test]
    fn bond_config_defaults_and_cap() {
        let mut flags = FlagsInConfig::default();
        let d = BondConfig::from_flags(&flags);
        assert_eq!(d.bond_count, 2);
        assert_eq!(d.replica_fill_max, 2);
        assert!(d.enabled());
        assert_eq!(BondConfig::default(), d);

        flags.peer_link_bond_count = 9;
        flags.peer_link_replica_fill_max = 3;
        let c = BondConfig::from_flags(&flags);
        assert_eq!(c.bond_count, BOND_COUNT_HARD_CAP);
        assert_eq!(c.replica_fill_max, 3);
        assert!(c.enabled());

        flags.peer_link_bond_count = 1;
        flags.peer_link_replica_fill_max = 1;
        let single = BondConfig::from_flags(&flags);
        assert_eq!(single.bond_count, 1);
        assert!(!single.enabled());
    }

    #[test]
    fn diversity_prefers_different_scheme_when_scores_close() {
        let cfg = ConnSelectConfig::default();
        let bond = BondConfig {
            bond_count: 2,
            replica_fill_max: 5,
        };
        // Same quality band (~13ms vs ~14ms), different schemes.
        let set = pick_bond_set(
            &[
                cand(1, 13_000, 0.0, "udp", "1.2.3.4:11010", false),
                cand(2, 13_000, 0.0, "udp", "1.2.3.4:11010", false),
                cand(3, 14_000, 0.0, "tcp", "1.2.3.4:11010", false),
            ],
            bond,
            cfg,
        );
        assert_eq!(set.len(), 2);
        let schemes: std::collections::HashSet<_> = set
            .iter()
            .map(|cid| if *cid == id(3) { "tcp" } else { "udp" })
            .collect();
        assert!(
            schemes.contains("udp") && schemes.contains("tcp"),
            "expected udp+tcp diversity, got {set:?}"
        );
    }

    #[test]
    fn replica_fill_when_single_class() {
        let cfg = ConnSelectConfig::default();
        let bond = BondConfig {
            bond_count: 3,
            replica_fill_max: 5,
        };
        let set = pick_bond_set(
            &[
                cand(1, 10_000, 0.0, "udp", "a:1", false),
                cand(2, 11_000, 0.0, "udp", "a:1", false),
                cand(3, 12_000, 0.0, "udp", "a:1", false),
            ],
            bond,
            cfg,
        );
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn replica_fill_max_respected() {
        let cfg = ConnSelectConfig::default();
        let bond = BondConfig {
            bond_count: 5,
            replica_fill_max: 2,
        };
        let set = pick_bond_set(
            &[
                cand(1, 10_000, 0.0, "udp", "a:1", false),
                cand(2, 11_000, 0.0, "udp", "a:1", false),
                cand(3, 12_000, 0.0, "udp", "a:1", false),
            ],
            bond,
            cfg,
        );
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn fused_excluded_when_alternative_exists() {
        let cfg = ConnSelectConfig::default();
        let bond = BondConfig {
            bond_count: 2,
            replica_fill_max: 5,
        };
        let set = pick_bond_set(
            &[
                cand(1, 10_000, 0.5, "udp", "a:1", false), // fused at 20%
                cand(2, 20_000, 0.0, "tcp", "b:1", false),
            ],
            bond,
            cfg,
        );
        assert_eq!(set, vec![id(2)]);
    }

    #[test]
    fn same_five_tuple_sticky_index() {
        // Minimal IPv4 TCP: version/ihl, proto=6, src/dst, ports
        let mut pkt = vec![0u8; 40];
        pkt[0] = 0x45;
        pkt[9] = 6;
        pkt[12..16].copy_from_slice(&[10, 0, 0, 1]);
        pkt[16..20].copy_from_slice(&[10, 0, 0, 2]);
        pkt[20..24].copy_from_slice(&[0x01, 0xbb, 0x15, 0xb3]); // 443, 5555
        let k1 = flow_key_from_payload(&pkt);
        let k2 = flow_key_from_payload(&pkt);
        assert_eq!(k1, k2);
        assert_eq!(bond_member_index(k1, 3), bond_member_index(k2, 3));
        // The tuple helper must produce the same key as the packet parser.
        assert_eq!(
            k1,
            flow_key_from_ipv4_tuple(
                6,
                Ipv4Addr::new(10, 0, 0, 1),
                Ipv4Addr::new(10, 0, 0, 2),
                443,
                0x15b3,
            )
        );

        pkt[20] = 0x02; // change src port
        let k3 = flow_key_from_payload(&pkt);
        assert_ne!(k1, k3);
    }

    #[test]
    fn bond_fill_needed_until_target_reached() {
        let bond = BondConfig {
            bond_count: 3,
            replica_fill_max: 5,
        };
        assert!(bond_fill_needed(&bond, 0));
        assert!(bond_fill_needed(&bond, 2));
        assert!(!bond_fill_needed(&bond, 3));
        assert!(!bond_fill_needed(&bond, 9));
        // Disabled (bond_count=1) never needs fill, even with zero conns.
        assert!(!bond_fill_needed(
            &BondConfig {
                bond_count: 1,
                replica_fill_max: 2,
            },
            0
        ));
        // Factory default bond_count=2 still requests fill until target is met.
        assert!(bond_fill_needed(&BondConfig::default(), 0));
        assert!(bond_fill_needed(&BondConfig::default(), 1));
        assert!(!bond_fill_needed(&BondConfig::default(), 2));
    }

    #[test]
    fn standby_ceiling_is_bond_count_plus_slack_capped() {
        // Factory default: 2 + 2 = 4.
        assert_eq!(bond_fill_live_ceiling(&BondConfig::default()), 4);
        // Send-set hard cap is 5; live fill ceiling keeps slack on top (5+2=7)
        // so N=5 is not starved of further in-band dial attempts.
        let wide = BondConfig {
            bond_count: 5,
            replica_fill_max: 5,
        };
        assert_eq!(bond_fill_live_ceiling(&wide), BOND_LIVE_HARD_CAP as usize);
        assert_eq!(BOND_LIVE_HARD_CAP, BOND_COUNT_HARD_CAP + BOND_STANDBY_SLACK);
    }

    #[test]
    fn out_of_band_standby_does_not_block_fill() {
        let bond = BondConfig {
            bond_count: 2,
            replica_fill_max: 2,
        };
        // Below target: fill regardless of the (stale/empty) send set.
        assert!(bond_fill_needed_with_standby(&bond, 1, 0));
        // Live reaches 2, but one is out-of-band standby (set len 1) → keep
        // filling while under the ceiling.
        assert!(bond_fill_needed_with_standby(&bond, 2, 1));
        assert!(bond_fill_needed_with_standby(&bond, 3, 1));
        // Send set full → stop even though live is under the ceiling.
        assert!(!bond_fill_needed_with_standby(&bond, 2, 2));
        assert!(!bond_fill_needed_with_standby(&bond, 3, 2));
        // Ceiling reached with a still-thin set → stop (no unbounded dial loop).
        assert!(!bond_fill_needed_with_standby(&bond, 4, 1));
        // Bonding off (bond_count=1) never fills.
        let off = BondConfig {
            bond_count: 1,
            replica_fill_max: 2,
        };
        assert!(!bond_fill_needed_with_standby(&off, 0, 0));
    }

    #[test]
    fn max_bond_count_keeps_standby_slack_for_fill() {
        // Regression: live ceiling must not collapse onto the send-set cap at
        // N=5, or out-of-band standbys freeze fill with set_len < N.
        let bond = BondConfig {
            bond_count: 5,
            replica_fill_max: 5,
        };
        assert_eq!(bond_fill_live_ceiling(&bond), 7);
        // Five live, one in-band → still fill (two standby slots left).
        assert!(bond_fill_needed_with_standby(&bond, 5, 1));
        assert!(bond_fill_needed_with_standby(&bond, 6, 1));
        // Live ceiling → stop even if the send set is still thin.
        assert!(!bond_fill_needed_with_standby(&bond, 7, 1));
        // Send set full → stop under the ceiling.
        assert!(!bond_fill_needed_with_standby(&bond, 5, 5));
    }

    #[test]
    fn quality_band_excludes_much_worse() {
        let cfg = ConnSelectConfig::default();
        let best = conn_quality_score(10_000, 0.0, 0, cfg);
        let close = conn_quality_score(10_500, 0.0, 0, cfg);
        let far = conn_quality_score(50_000, 0.0, 0, cfg);
        assert!(in_quality_band(close, best, cfg));
        assert!(!in_quality_band(far, best, cfg));
    }

    #[test]
    fn out_of_band_not_pulled_to_fill_bond_count() {
        // Quality-first: a much worse TCP must not join the bond send set just
        // because bond_count=2 and diversity would like a second scheme.
        // It stays as hot standby outside the set (failover when UDP dies).
        let cfg = ConnSelectConfig::default();
        let bond = BondConfig {
            bond_count: 2,
            replica_fill_max: 5,
        };
        let set = pick_bond_set(
            &[
                cand(1, 64_000, 0.0, "udp", "et.example:11010", false), // ~64ms
                cand(2, 782_000, 0.1, "tcp", "et.example:11010", false), // ~782ms + loss
            ],
            bond,
            cfg,
        );
        assert_eq!(
            set,
            vec![id(1)],
            "only in-band UDP should bond; TCP stays standby, got {set:?}"
        );
    }

    #[test]
    fn only_worse_paths_still_bond_among_themselves() {
        // When no better path exists, the remaining (worse) paths form the
        // bond set among their own quality band — standby becomes usable.
        let cfg = ConnSelectConfig::default();
        let bond = BondConfig {
            bond_count: 2,
            replica_fill_max: 5,
        };
        let set = pick_bond_set(
            &[
                cand(1, 400_000, 0.05, "udp", "et.example:11010", false),
                cand(2, 420_000, 0.05, "tcp", "et.example:11010", false),
            ],
            bond,
            cfg,
        );
        assert_eq!(
            set.len(),
            2,
            "same-band worse paths still bond together: {set:?}"
        );
        assert!(set.contains(&id(1)) && set.contains(&id(2)));
    }

    #[test]
    fn sticky_key_survives_ciphertext_payload_mutation() {
        // Simulate: flow key taken from plaintext IP, then payload overwritten
        // by AEAD (classic encrypt-before-send). Bonding must keep using the
        // precomputed key — hashing ciphertext would spray per packet.
        let mut pkt = vec![0u8; 40];
        pkt[0] = 0x45;
        pkt[9] = 6;
        pkt[12..16].copy_from_slice(&[10, 0, 0, 1]);
        pkt[16..20].copy_from_slice(&[10, 0, 0, 2]);
        pkt[20..24].copy_from_slice(&[0x01, 0xbb, 0x15, 0xb3]);
        let sticky = flow_key_from_payload(&pkt);

        // "Encrypt" two times with different nonces → different ciphertext heads.
        let mut c1 = pkt.clone();
        c1[0] = 0xa5;
        c1[1] = 0x11;
        let mut c2 = pkt.clone();
        c2[0] = 0x5a;
        c2[1] = 0x22;
        let h1 = flow_key_from_payload(&c1);
        let h2 = flow_key_from_payload(&c2);
        assert_ne!(
            h1, h2,
            "ciphertext hashes must differ (documents the spray bug)"
        );
        // Same sticky key → same member forever; ciphertext-derived keys diverge.
        assert_eq!(bond_member_index(sticky, 4), bond_member_index(sticky, 4));
        assert_ne!(sticky, h1);
        assert_ne!(sticky, h2);
    }

    /// S4: position-based hashing (`key % member_count`) re-points flows whenever
    /// the member set changes. Consistent hashing over conn ids has a much
    /// stronger property:
    ///
    /// **A flow can only move because its own member left the set, or because a
    /// newly added member now wins the min-hash. It is never re-pointed to a
    /// different member that was already present.**
    ///
    /// That is exactly what reordering looks like to the inner TCP/RDP stream:
    /// moving to an already-present sibling conn is pure loss with no benefit.
    #[test]
    fn consistent_hashing_never_repoints_a_flow_to_an_existing_sibling() {
        let a = id(1);
        let b = id(2);
        let c = id(3);
        let d = id(4);
        let flows: Vec<u64> = (0u64..1024).collect();

        // `b` dies, `d` joins (member death + replica fill).
        let before = [a, b, c];
        let after = [a, c, d];

        let pick = |ids: &[PeerConnId]| -> Vec<PeerConnId> {
            flows
                .iter()
                .map(|f| ids[bond_member_for_flow(*f, ids).unwrap()])
                .collect()
        };
        let pb = pick(&before);
        let pa = pick(&after);

        let mut illegal = 0;
        let mut moved = 0;
        for i in 0..flows.len() {
            if pb[i] == pa[i] {
                continue;
            }
            moved += 1;
            // Legal only if it left a dead member, or landed on the new one.
            if pb[i] != b && pa[i] != d {
                illegal += 1;
            }
        }
        assert_eq!(
            illegal, 0,
            "a flow was re-pointed to an already-present sibling conn (S4 reorder)"
        );
        assert!(moved > 0, "sanity: the set change should move *some* flows");

        // Every conn keeps a meaningful share of the load (bonding still spreads).
        for id in [a, c, d] {
            let share = pa.iter().filter(|p| **p == id).count();
            assert!(
                share > flows.len() / 8,
                "conn {id:?} got only {share} flows"
            );
        }

        // Stronger case: a **pure re-order** (no member added or removed) must
        // move *nothing*. `pick_bond_set` re-orders via
        // `cmp_candidate_score_then_id` whenever the quality band shifts, so
        // this is the common case, not the exotic one.
        let reordered: Vec<PeerConnId> = pick(&[c, a, b]);
        let reordered_moved = pb
            .iter()
            .zip(reordered.iter())
            .filter(|(x, y)| x != y)
            .count();
        assert_eq!(
            reordered_moved, 0,
            "re-ordering the same members must not re-point a single flow"
        );
    }

    /// Counter-example documenting why the old index-based hash had to go.
    ///
    /// `flow_key % member_count` reads the member at a **position**. Both of its
    /// failure modes come from that, and neither happens to consistent hashing:
    ///
    /// 1. **Pure re-order** — `pick_bond_set` re-orders members when the quality
    ///    band shifts (`cmp_candidate_score_then_id`), with nothing added or
    ///    removed. Every flow on a moved index is re-pointed to a sibling that
    ///    was *already there*: pure reordering, which is the S4 stutter.
    /// 2. **Count change** — replica fill or a `bond_count` change re-points a
    ///    large share of flows at once.
    #[test]
    fn positional_hashing_repoints_flows_on_reorder_or_count_change() {
        let a = id(1);
        let b = id(2);
        let c = id(3);
        let flows: Vec<u64> = (0u64..1024).collect();

        let pick_positional = |ids: &[PeerConnId]| -> Vec<PeerConnId> {
            flows
                .iter()
                .map(|f| ids[bond_member_index(*f, ids.len())])
                .collect()
        };

        // (1) Pure reorder, same members.
        let before = pick_positional(&[a, b, c]);
        let after = pick_positional(&[c, b, a]);
        let moved = before
            .iter()
            .zip(after.iter())
            .filter(|(x, y)| x != y)
            .count();
        // [a,b,c] → [c,b,a] leaves index 1 (`b`) alone and moves the other two.
        assert!(
            moved > flows.len() / 2,
            "index-based hashing must move a majority of flows on a pure re-order, \
             got {moved}/{}",
            flows.len()
        );

        // (2) Count change.
        let two = flows
            .iter()
            .map(|f| bond_member_index(*f, 2))
            .collect::<Vec<_>>();
        let three = flows
            .iter()
            .map(|f| bond_member_index(*f, 3))
            .collect::<Vec<_>>();
        let moved = flows
            .iter()
            .zip(two.iter().zip(three.iter()))
            .filter(|(_, (x, y))| x != y)
            .count();
        assert!(
            moved > flows.len() / 3,
            "index-based hashing should move >1/3 of flows on a count change, got {moved}"
        );
    }

    #[test]
    fn bond_member_for_flow_is_deterministic_and_total() {
        let ids = [id(1), id(2)];
        for f in 0u64..64 {
            let a = bond_member_for_flow(f, &ids);
            let b = bond_member_for_flow(f, &ids);
            assert_eq!(a, b, "must be deterministic");
            assert!(a.is_some_and(|i| i < 2));
        }
        assert_eq!(bond_member_for_flow(7, &[]), None);
        // Single member always wins, regardless of the key.
        assert_eq!(bond_member_for_flow(7, &[id(9)]), Some(0));
    }

    #[test]
    fn pick_bond_set_is_deterministic_for_same_inputs() {
        let cfg = ConnSelectConfig::default();
        let bond = BondConfig {
            bond_count: 3,
            replica_fill_max: 5,
        };
        let cands = [
            cand(1, 10_000, 0.0, "udp", "a:1", false),
            cand(2, 11_000, 0.0, "tcp", "b:1", false),
            cand(3, 12_000, 0.0, "udp", "c:1", false),
        ];
        let a = pick_bond_set(&cands, bond, cfg);
        let b = pick_bond_set(&cands, bond, cfg);
        assert_eq!(a, b);
        assert_eq!(a.len(), 3);
        // Same set in different order (DashMap iteration is randomized) must
        // give the same member set — otherwise flow mapping flaps on rebuild.
        let rev = [cands[2].clone(), cands[1].clone(), cands[0].clone()];
        assert_eq!(pick_bond_set(&rev, bond, cfg), a);
    }

    #[test]
    fn equal_scores_do_not_depend_on_input_order() {
        let cfg = ConnSelectConfig::default();
        let bond = BondConfig {
            bond_count: 2,
            replica_fill_max: 5,
        };
        // Identical scores across classes: tie-break must be conn id, not input order.
        let fwd = [
            cand(1, 10_000, 0.0, "udp", "a:1", false),
            cand(2, 10_000, 0.0, "tcp", "b:1", false),
            cand(3, 10_000, 0.0, "wg", "c:1", false),
        ];
        let rev = [fwd[2].clone(), fwd[1].clone(), fwd[0].clone()];
        assert_eq!(
            pick_bond_set(&fwd, bond, cfg),
            pick_bond_set(&rev, bond, cfg)
        );
    }
}
