//! Same-peer PeerConn quality selection (connection-stability P1.7).
//!
//! Lower score is better. Defaults: loss weight dominates slight RTT wins;
//! loss fuse excludes high-loss paths when an alternative exists; hysteresis
//! avoids flapping across the periodic default_conn reselect cycle.

use easytier_proto::common::FlagsInConfig;

use super::peer_conn::PeerConnId;

/// Tunables for PeerConn quality scoring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConnSelectConfig {
    pub w_lat: f64,
    pub w_loss: f64,
    pub w_jitter: f64,
    /// Loss rate above this (0..1) is fused out when another path exists.
    pub loss_fuse: f64,
    /// Relative score margin required to challenge the incumbent (0..1).
    pub switch_margin: f64,
    /// Absolute score delta required in addition to relative margin.
    /// Prevents sticky near-zero scores where relative % is meaningless.
    pub switch_abs_margin: f64,
    /// Consecutive reselect cycles the challenger must beat margin before switch.
    pub switch_windows: u32,
}

impl Default for ConnSelectConfig {
    fn default() -> Self {
        Self {
            w_lat: 1.0,
            w_loss: 4.0,
            w_jitter: 1.0,
            loss_fuse: 0.20,
            switch_margin: 0.10,
            // ~5ms RTT-equivalent on the default w_lat scale.
            switch_abs_margin: 0.005,
            switch_windows: 2,
        }
    }
}

impl ConnSelectConfig {
    /// Resolve from `FlagsInConfig`.
    ///
    /// - All select fields `0` (prost / unset Default) → built-in defaults.
    /// - Otherwise values are taken literally so TOML can set a weight to `0`
    ///   (runtime flags are merged from `gen_default_flags`, so absent keys
    ///   become 100/400/… rather than 0).
    ///
    /// Weights are hundredths (`100` = 1.0). Fuse / relative margin are percents.
    /// Absolute margin is milli-units (`5` = 0.005).
    pub fn from_flags(flags: &FlagsInConfig) -> Self {
        let unset = flags.conn_select_w_lat == 0
            && flags.conn_select_w_loss == 0
            && flags.conn_select_w_jitter == 0
            && flags.conn_select_loss_fuse_pct == 0
            && flags.conn_select_switch_margin_pct == 0
            && flags.conn_select_switch_abs_margin_milli == 0
            && flags.conn_select_switch_windows == 0;
        if unset {
            return Self::default();
        }
        Self {
            w_lat: f64::from(flags.conn_select_w_lat) / 100.0,
            w_loss: f64::from(flags.conn_select_w_loss) / 100.0,
            w_jitter: f64::from(flags.conn_select_w_jitter) / 100.0,
            loss_fuse: (f64::from(flags.conn_select_loss_fuse_pct) / 100.0).clamp(0.0, 1.0),
            switch_margin: (f64::from(flags.conn_select_switch_margin_pct) / 100.0).clamp(0.0, 1.0),
            switch_abs_margin: f64::from(flags.conn_select_switch_abs_margin_milli) / 1000.0,
            switch_windows: flags.conn_select_switch_windows.max(1),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConnMetrics {
    pub conn_id: PeerConnId,
    pub latency_us: u64,
    pub loss_rate: f32,
    pub jitter_us: u64,
    pub is_hole_punched: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ScoredConn {
    pub conn_id: PeerConnId,
    pub score: f64,
    /// Unverified hole-punch (latency 0) — always deprioritized.
    pub unverified_hole_punch: bool,
    /// Above loss fuse; ineligible unless every alternative is also fused/unverified.
    pub fused: bool,
}

/// Composite quality score (lower = better).
pub fn conn_quality_score(
    latency_us: u64,
    loss_rate: f32,
    jitter_us: u64,
    cfg: ConnSelectConfig,
) -> f64 {
    let norm_rtt = latency_us as f64 / 1_000_000.0;
    let norm_loss = f64::from(loss_rate).clamp(0.0, 1.0);
    let norm_jitter = jitter_us as f64 / 1_000_000.0;
    cfg.w_lat * norm_rtt + cfg.w_loss * norm_loss + cfg.w_jitter * norm_jitter
}

/// Cap for values published into `DirectConnectedPeerInfo.latency_ms` / OSPF edge weight.
/// Must stay well below `AVOID_RELAY_COST` (i32::MAX) so fuse/avoid math remains distinct.
pub const OSPF_EDGE_COST_MAX: i32 = 1_000_000;
/// Extra cost when the path is loss-fused (discourages multi-hop via bad links).
pub const OSPF_FUSE_COST_BONUS: i32 = 10_000;
/// Publisher hysteresis: ignore cost wobble smaller than this (ms-equivalent units).
pub const OSPF_EDGE_COST_MIN_DELTA: i32 = 20;

/// Encode a quality score into the integer edge cost peer-center / OSPF consume.
///
/// With default `w_lat=1`, zero loss/jitter maps ≈ RTT milliseconds — same order
/// of magnitude as the historical latency-only publisher.
pub fn ospf_edge_cost_from_score(score: f64, fused: bool) -> i32 {
    let mut cost = (score * 1000.0).round() as i32;
    if fused {
        cost = cost.saturating_add(OSPF_FUSE_COST_BONUS);
    }
    cost.clamp(1, OSPF_EDGE_COST_MAX)
}

/// Build OSPF edge cost from raw metrics (same formula as `select_conn`).
pub fn ospf_edge_cost_ms(
    latency_us: u64,
    loss_rate: f32,
    jitter_us: u64,
    fused: bool,
    cfg: ConnSelectConfig,
) -> i32 {
    ospf_edge_cost_from_score(
        conn_quality_score(latency_us, loss_rate, jitter_us, cfg),
        fused,
    )
}

/// Keep the last published cost when the raw delta is below `min_delta`.
pub fn apply_ospf_cost_hysteresis(raw: i32, last: Option<i32>, min_delta: i32) -> i32 {
    match last {
        Some(prev) if (raw - prev).abs() < min_delta => prev,
        _ => raw,
    }
}

pub fn score_conn(m: &ConnMetrics, cfg: ConnSelectConfig) -> ScoredConn {
    let unverified_hole_punch = m.is_hole_punched && m.latency_us == 0;
    let fused = !unverified_hole_punch && f64::from(m.loss_rate) > cfg.loss_fuse;
    ScoredConn {
        conn_id: m.conn_id,
        score: conn_quality_score(m.latency_us, m.loss_rate, m.jitter_us, cfg),
        unverified_hole_punch,
        fused,
    }
}

/// Pick the next default PeerConn id given scored candidates and hysteresis state.
///
/// Returns `(selected_id, new_better_streak)`.
pub fn pick_default_conn(
    scored: &[ScoredConn],
    last_default: Option<PeerConnId>,
    better_streak: u32,
    cfg: ConnSelectConfig,
) -> Option<(PeerConnId, u32)> {
    if scored.is_empty() {
        return None;
    }

    let has_verified = scored.iter().any(|c| !c.unverified_hole_punch);
    let has_unfused = scored.iter().any(|c| !c.unverified_hole_punch && !c.fused);

    let best = scored
        .iter()
        .filter(|c| {
            if has_verified && c.unverified_hole_punch {
                return false;
            }
            if has_unfused && c.fused {
                return false;
            }
            true
        })
        .min_by(|a, b| a.score.total_cmp(&b.score))?;

    let Some(last_id) = last_default else {
        return Some((best.conn_id, 0));
    };

    let Some(incumbent) = scored.iter().find(|c| c.conn_id == last_id) else {
        return Some((best.conn_id, 0));
    };

    // Incumbent no longer usable → take best immediately.
    let incumbent_unusable =
        (has_verified && incumbent.unverified_hole_punch) || (has_unfused && incumbent.fused);
    if incumbent_unusable {
        return Some((best.conn_id, 0));
    }

    if best.conn_id == last_id {
        return Some((last_id, 0));
    }

    let required_delta = (incumbent.score * cfg.switch_margin).max(cfg.switch_abs_margin);
    if incumbent.score - best.score >= required_delta {
        let next_streak = better_streak.saturating_add(1);
        if next_streak >= cfg.switch_windows.max(1) {
            return Some((best.conn_id, 0));
        }
        return Some((last_id, next_streak));
    }

    Some((last_id, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn id(n: u8) -> PeerConnId {
        Uuid::from_bytes([n; 16])
    }

    fn metrics(n: u8, latency_us: u64, loss_rate: f32, jitter_us: u64, hole: bool) -> ConnMetrics {
        ConnMetrics {
            conn_id: id(n),
            latency_us,
            loss_rate,
            jitter_us,
            is_hole_punched: hole,
        }
    }

    #[test]
    fn low_rtt_high_loss_loses_to_higher_rtt_low_loss() {
        let cfg = ConnSelectConfig::default();
        // ~5ms + 30% loss vs ~25ms + 1% loss (P1.8 acceptance sketch)
        let a = conn_quality_score(5_000, 0.30, 0, cfg);
        let b = conn_quality_score(25_000, 0.01, 0, cfg);
        assert!(b < a, "expected low-loss path better: a={a} b={b}");
    }

    #[test]
    fn ospf_edge_cost_pure_rtt_matches_latency_ms() {
        let cfg = ConnSelectConfig::default();
        // 25ms RTT, no loss/jitter → ~25 cost units (historical latency_ms).
        let cost = ospf_edge_cost_ms(25_000, 0.0, 0, false, cfg);
        assert_eq!(cost, 25);
    }

    #[test]
    fn ospf_edge_cost_prefers_low_loss_over_low_rtt() {
        let cfg = ConnSelectConfig::default();
        let high_loss = ospf_edge_cost_ms(5_000, 0.30, 0, false, cfg);
        let low_loss = ospf_edge_cost_ms(25_000, 0.01, 0, false, cfg);
        assert!(
            low_loss < high_loss,
            "low-loss edge should be cheaper for OSPF: high_loss={high_loss} low_loss={low_loss}"
        );
    }

    #[test]
    fn ospf_fuse_bonus_raises_cost() {
        let cfg = ConnSelectConfig::default();
        let normal = ospf_edge_cost_ms(20_000, 0.25, 0, false, cfg);
        let fused = ospf_edge_cost_ms(20_000, 0.25, 0, true, cfg);
        assert_eq!(fused - normal, OSPF_FUSE_COST_BONUS);
    }

    #[test]
    fn ospf_hysteresis_holds_small_wobble() {
        let held = apply_ospf_cost_hysteresis(55, Some(50), OSPF_EDGE_COST_MIN_DELTA);
        assert_eq!(held, 50);
        let updated = apply_ospf_cost_hysteresis(80, Some(50), OSPF_EDGE_COST_MIN_DELTA);
        assert_eq!(updated, 80);
    }

    #[test]
    fn loss_fuse_excludes_high_loss_when_alternative_exists() {
        let cfg = ConnSelectConfig::default();
        let scored = [
            score_conn(&metrics(1, 5_000, 0.35, 0, false), cfg),
            score_conn(&metrics(2, 25_000, 0.01, 0, false), cfg),
        ];
        assert!(scored[0].fused);
        assert!(!scored[1].fused);
        let (picked, _) = pick_default_conn(&scored, None, 0, cfg).unwrap();
        assert_eq!(picked, id(2));
    }

    #[test]
    fn unverified_hole_punch_deferred() {
        let cfg = ConnSelectConfig::default();
        let scored = [
            score_conn(&metrics(1, 0, 0.0, 0, true), cfg),
            score_conn(&metrics(2, 20_000, 0.0, 0, false), cfg),
        ];
        let (picked, _) = pick_default_conn(&scored, None, 0, cfg).unwrap();
        assert_eq!(picked, id(2));
    }

    #[test]
    fn hysteresis_requires_consecutive_windows() {
        let cfg = ConnSelectConfig {
            switch_windows: 2,
            switch_margin: 0.10,
            ..ConnSelectConfig::default()
        };
        // Incumbent score high; challenger much better.
        let incumbent = ScoredConn {
            conn_id: id(1),
            score: 1.0,
            unverified_hole_punch: false,
            fused: false,
        };
        let challenger = ScoredConn {
            conn_id: id(2),
            score: 0.5, // 50% better > 10% margin
            unverified_hole_punch: false,
            fused: false,
        };
        let scored = [incumbent, challenger];

        let (pick1, streak1) = pick_default_conn(&scored, Some(id(1)), 0, cfg).unwrap();
        assert_eq!(pick1, id(1));
        assert_eq!(streak1, 1);

        let (pick2, streak2) = pick_default_conn(&scored, Some(id(1)), streak1, cfg).unwrap();
        assert_eq!(pick2, id(2));
        assert_eq!(streak2, 0);
    }

    #[test]
    fn small_improvement_does_not_switch() {
        let cfg = ConnSelectConfig {
            switch_margin: 0.10,
            switch_abs_margin: 0.005,
            switch_windows: 1,
            ..ConnSelectConfig::default()
        };
        let incumbent = ScoredConn {
            conn_id: id(1),
            score: 1.0,
            unverified_hole_punch: false,
            fused: false,
        };
        let challenger = ScoredConn {
            conn_id: id(2),
            score: 0.95, // only 5% better
            unverified_hole_punch: false,
            fused: false,
        };
        let (picked, streak) =
            pick_default_conn(&[incumbent, challenger], Some(id(1)), 0, cfg).unwrap();
        assert_eq!(picked, id(1));
        assert_eq!(streak, 0);
    }

    #[test]
    fn near_zero_score_uses_absolute_margin() {
        let cfg = ConnSelectConfig {
            switch_margin: 0.10,
            switch_abs_margin: 0.005,
            switch_windows: 1,
            ..ConnSelectConfig::default()
        };
        let incumbent = ScoredConn {
            conn_id: id(1),
            score: 0.01,
            unverified_hole_punch: false,
            fused: false,
        };
        let almost = ScoredConn {
            conn_id: id(2),
            score: 0.008, // Δ=0.002 < 0.005
            unverified_hole_punch: false,
            fused: false,
        };
        let (picked, _) = pick_default_conn(&[incumbent, almost], Some(id(1)), 0, cfg).unwrap();
        assert_eq!(picked, id(1));

        let clearly_better = ScoredConn {
            conn_id: id(3),
            score: 0.004, // Δ=0.006 >= 0.005
            unverified_hole_punch: false,
            fused: false,
        };
        let (picked, _) =
            pick_default_conn(&[incumbent, clearly_better], Some(id(1)), 0, cfg).unwrap();
        assert_eq!(picked, id(3));
    }

    #[test]
    fn from_flags_all_zero_keeps_builtin_default() {
        let flags = FlagsInConfig::default();
        assert_eq!(
            ConnSelectConfig::from_flags(&flags),
            ConnSelectConfig::default()
        );
    }

    #[test]
    fn from_flags_materialized_zero_weight_disables_metric() {
        // Simulate gen_default_flags merge, then explicit w_lat = 0.
        let flags = FlagsInConfig {
            conn_select_w_lat: 0,
            conn_select_w_loss: 400,
            conn_select_w_jitter: 100,
            conn_select_loss_fuse_pct: 20,
            conn_select_switch_margin_pct: 10,
            conn_select_switch_abs_margin_milli: 5,
            conn_select_switch_windows: 2,
            ..Default::default()
        };
        let cfg = ConnSelectConfig::from_flags(&flags);
        assert_eq!(cfg.w_lat, 0.0);
        assert_eq!(cfg.w_loss, 4.0);
        assert_eq!(cfg.w_jitter, 1.0);
        assert_eq!(cfg.loss_fuse, 0.20);
        assert_eq!(cfg.switch_windows, 2);
    }

    #[test]
    fn from_flags_overrides_weights() {
        let flags = FlagsInConfig {
            conn_select_w_lat: 100,
            conn_select_w_loss: 800, // 8.0
            conn_select_w_jitter: 100,
            conn_select_loss_fuse_pct: 30,
            conn_select_switch_margin_pct: 10,
            conn_select_switch_abs_margin_milli: 5,
            conn_select_switch_windows: 3,
            ..Default::default()
        };
        let cfg = ConnSelectConfig::from_flags(&flags);
        assert_eq!(cfg.w_lat, 1.0);
        assert_eq!(cfg.w_loss, 8.0);
        assert_eq!(cfg.loss_fuse, 0.30);
        assert_eq!(cfg.switch_windows, 3);
    }

    #[test]
    fn p18_acceptance_pick_prefers_stable_over_lossy_fast() {
        let cfg = ConnSelectConfig::default();
        let scored = [
            score_conn(&metrics(1, 5_000, 0.30, 2_000, false), cfg),
            score_conn(&metrics(2, 25_000, 0.01, 500, false), cfg),
        ];
        let (picked, _) = pick_default_conn(&scored, None, 0, cfg).unwrap();
        assert_eq!(picked, id(2));
    }

    #[test]
    fn p18_acceptance_hysteresis_avoids_single_window_flap() {
        let cfg = ConnSelectConfig {
            switch_windows: 2,
            ..ConnSelectConfig::default()
        };
        let stable = score_conn(&metrics(1, 20_000, 0.01, 0, false), cfg);
        let burst = score_conn(&metrics(2, 8_000, 0.01, 0, false), cfg);
        // Incumbent is stable (higher score); burst is better by enough margin.
        assert!(burst.score + 0.005 < stable.score);
        let scored = [stable, burst];
        let (p1, s1) = pick_default_conn(&scored, Some(id(1)), 0, cfg).unwrap();
        assert_eq!(p1, id(1));
        assert_eq!(s1, 1);
        let (p2, s2) = pick_default_conn(&scored, Some(id(1)), s1, cfg).unwrap();
        assert_eq!(p2, id(2));
        assert_eq!(s2, 0);
    }
}
