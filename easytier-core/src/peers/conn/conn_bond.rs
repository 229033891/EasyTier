//! PeerConn bonding Phase 2a: diversity-first member set + per-flow hash send.
//!
//! See `docs/roadmap/multi-link-bonding.md`. Default `bond_count=1` keeps today's
//! single `default_conn` path.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use easytier_proto::common::FlagsInConfig;

use super::conn_select::{ConnSelectConfig, ScoredConn};
use super::peer_conn::{PeerConn, PeerConnId};

/// Runtime hard cap for `peer_link_bond_count` (roadmap suggested max 5).
pub const BOND_COUNT_HARD_CAP: u32 = 5;

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
            bond_count: 1,
            replica_fill_max: 5,
        }
    }
}

impl BondConfig {
    /// Resolve from flags. `0` on either field means built-in default.
    pub fn from_flags(flags: &FlagsInConfig) -> Self {
        let mut bond_count = if flags.peer_link_bond_count == 0 {
            1
        } else {
            flags.peer_link_bond_count
        };
        bond_count = bond_count.clamp(1, BOND_COUNT_HARD_CAP);

        let replica_fill_max = if flags.peer_link_replica_fill_max == 0 {
            5
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
/// (`bond_count` target not yet reached).
pub fn bond_fill_needed(bond: &BondConfig, live_conns: usize) -> bool {
    bond.enabled() && (live_conns as u32) < bond.bond_count
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

    // Pass 2: replica-fill (same class allowed) from band, then remaining eligible.
    let mut fill_pool: Vec<&BondCandidate> = band.clone();
    for c in &eligible {
        if !fill_pool
            .iter()
            .any(|x| x.scored.conn_id == c.scored.conn_id)
        {
            fill_pool.push(*c);
        }
    }
    fill_pool.sort_by(|a, b| cmp_candidate_score_then_id(a, b));

    for c in fill_pool {
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

/// Stable index into `member_count` for a flow key.
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
        assert_eq!(d.bond_count, 1);
        assert_eq!(d.replica_fill_max, 5);
        assert!(!d.enabled());

        flags.peer_link_bond_count = 9;
        flags.peer_link_replica_fill_max = 3;
        let c = BondConfig::from_flags(&flags);
        assert_eq!(c.bond_count, BOND_COUNT_HARD_CAP);
        assert_eq!(c.replica_fill_max, 3);
        assert!(c.enabled());
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
        assert!(!bond_fill_needed(&BondConfig::default(), 0));
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
