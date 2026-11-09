use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use rand::{RngExt, rng};
use tokio::{
    sync::{broadcast, mpsc::error::TrySendError},
    task::JoinSet,
};

use crate::{
    config::PeerId,
    foundation::time::{Interval, interval, sleep, timeout},
    packet::{PacketType, ZCPacket},
    peers::{
        conn::{
            conn_select::{ConnMetrics, ConnSelectConfig, is_unfused_verified},
            peer_conn::PeerConnCloseNotify,
            peer_conn_liveness::PeerConnLiveness,
        },
        context::ArcPeerContext,
        error::Error,
    },
    tunnel::{
        TunnelError,
        mpsc::MpscTunnelSender,
        stats::{Throughput, WindowLatency},
    },
};

/// Callback used by the ordinary keepalive when consecutive failures reach
/// `ping_fail_close_count`. Returns `true` if this PeerConn may be torn down.
///
/// Wired from [`super::peer::Peer`]: under a peer-wide lock it checks for a
/// verified unfused alternative and, when allowing close, marks the conn closed
/// before releasing the lock so concurrent ping tasks cannot avalanche.
pub(crate) type PingFailCloseGate = Arc<dyn Fn() -> bool + Send + Sync>;

/// P1.5 pure decision: may consecutive ping failures tear this conn down?
///
/// `true` only when at least one *other* live path is a real alternative
/// (`is_unfused_verified`). Unverified hole-punches do not count — same predicate
/// as `pick_default_conn` / cached-path invalidation.
pub(crate) fn may_tear_down_for_ping_failure<'a>(
    alternatives: impl IntoIterator<Item = &'a ConnMetrics>,
    cfg: ConnSelectConfig,
) -> bool {
    alternatives
        .into_iter()
        .any(|metrics| is_unfused_verified(metrics, cfg))
}

/// Wait budget for one ordinary keepalive ping.
const PING_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Wall-clock budget for holding the last path **after** `fail_close_count` is
/// reached with no verified alternative (P1.5). Measured from the first miss at
/// or above the threshold, then force-close (zombie escape).
///
/// Why wall clock and not "N more misses": the keepalive retries as fast as one
/// outstanding ping per [`PING_WAIT_TIMEOUT`], but the *interval* between
/// attempts is owned by `PingIntervalController`, which may slow down (backoff,
/// `max_ping_interval`). A miss-count budget therefore silently stretched from
/// ~10s to well over a minute on slow links — long enough to outlive
/// `credential_non_reusable_*` (60s) and to keep `has_live_conns()` true while
/// the connector refused to re-dial (`direct peers=[]` in the credential tests).
///
/// The hold is also **bounded on purpose**: it buys a brief grace window, it is
/// not a promise to keep a dead conn forever. `fail_close_count` consecutive
/// misses (~20s at the default 10 × 2s) is already the jitter tolerance; this is
/// only the escape hatch on top of it.
const LAST_PATH_HOLD_MAX: Duration = Duration::from_secs(5);

/// How long the egress ring may stay full before the keepalive gives up.
///
/// `TunnelError::BufferFull` on a ping is **local congestion**, not a path miss,
/// so it must not advance `loss_counter` (a full ring says nothing about the
/// peer). But a full ring that never clears is a stuck sink: the ping itself
/// cannot leave the box, so without this clock the conn would look healthy
/// forever — `has_live_conns()` stays true, the direct connector refuses to
/// re-dial, and the only escape left is the 30s per-write budget inside
/// `MpscTunnel`. This is that escape, expressed where liveness is decided.
///
/// Note what "full" means here after 2026-11-09: the ping first retries a
/// `BufferFull` for up to 50 ms, so a ring that is merely saturated (slots
/// free every few microseconds at line rate) never reaches this clock. Only a
/// ring that cannot yield a slot within 50 ms — a genuinely stuck sink —
/// accumulates towards this deadline.
const LOCAL_CONGESTION_CLOSE_AFTER: Duration = Duration::from_secs(30);

/// Fast liveness probe (roadmap `live-next-hop.md` Phase F — "BFD-style").
///
/// This is an *extra* short round trip whose only job is to publish the loss
/// rate early. It never closes the conn: teardown stays with the ordinary
/// keepalive and its `ping_fail_close_count`, so the deliberate flap tolerance
/// around a reconnect (which costs ~30s to rebuild) is unchanged.
const FAST_PROBE_INTERVAL: Duration = Duration::from_millis(333);
/// Misses without an intervening proven round trip before the conn is published
/// as degraded (100% loss).
///
/// "Intervening" includes the keepalive: a pong (or liveness echo) that lands in
/// the same round ends the miss run — see [`fast_probe_apply_round_evidence`] —
/// because it proves the path is carrying packets right now even though this
/// probe got no answer within its own budget. Inbound bytes end it too, but only
/// while the probe budget is still short enough for a queue to explain the
/// timeout; see [`DeliveryEvidence::excuses_a_timeout`].
const FAST_PROBE_MISSES_TO_DEGRADE: u32 = 3;
/// Consecutive **rounds of proof** needed to clear a published degradation.
///
/// Detection stays at [`FAST_PROBE_MISSES_TO_DEGRADE`]; only the way back is
/// slower. `misses` restarts at 0 on any single piece of proof, so under bursty
/// loss (miss, miss, ok, miss, miss, ok …) the old rule cleared `degraded` on
/// that lone ok and re-set it three probes later — forever, several times a
/// second. Each flip re-prices our edge (`loss_rate()` → `score_conn` →
/// `ospf_edge_cost_from_score`) and re-runs SPF, so a flapping link produced continuous
/// route churn on top of the packet loss. Three clean rounds ≈ 1s of proof
/// before traffic is moved back onto the link.
///
/// A round of proof is one probe round in which a **round trip** completed: the
/// probe's own pong, a keepalive pong, or a liveness echo that landed during it.
/// When several happen they still count once — this constant is about rounds,
/// not samples.
///
/// Inbound bytes are deliberately **not** a round of proof. They show the peer
/// can still reach us, which is enough to stop accusing the path of being gone,
/// but not enough to call it healthy: a one-way outage keeps bytes arriving
/// while carrying nothing we send. Letting that clear a degradation is how a
/// half-dead link would keep its place in the SPF forever.
const FAST_PROBE_RECOVER_ROUNDS: u32 = 3;
/// Consecutive missed probe rounds before the degradation is allowed to drive the
/// **local** routing view (see `PeerConn::is_fast_probe_sustainedly_degraded`).
///
/// Publishing 100% loss is cheap to undo — worst case the network avoids a link
/// that is merely bad — so it fires on the third miss. Letting it re-price our
/// own outgoing edge in the local SPF is not: a probe burst that happens to be
/// lost while data still flows would move real traffic onto a worse path, or make
/// the peer look unreachable when it is the only path. So that decision waits for
/// a *sustained* run of misses, long enough that independent loss alone cannot
/// produce it: at 30% per-probe loss, 9 consecutive misses happen with
/// probability ~2e-5.
///
/// Note the real wall-clock cost is larger than `9 × 333ms`, because every missed
/// round also burns its timeout budget (`FAST_PROBE_TIMEOUT_MIN`, 300ms at best).
/// A dead low-RTT link therefore reaches this gate in ~5.7s, not 3s.
const FAST_PROBE_SUSTAINED_ROUNDS: u32 = 9;

/// Whether `consecutive_misses` is enough degradation to override local routing.
fn fast_probe_is_sustained(consecutive_misses: u32) -> bool {
    consecutive_misses >= FAST_PROBE_SUSTAINED_ROUNDS
}

/// What the fast probe publishes this round.
///
/// Going *in* takes [`FAST_PROBE_MISSES_TO_DEGRADE`] consecutive misses; coming
/// *out* takes [`FAST_PROBE_RECOVER_ROUNDS`] consecutive rounds of proof (see
/// that constant for why the asymmetry exists).
fn fast_probe_publish_degraded(
    consecutive_misses: u32,
    consecutive_hits: u32,
    was_degraded: bool,
) -> bool {
    consecutive_misses >= FAST_PROBE_MISSES_TO_DEGRADE
        || (was_degraded && consecutive_hits < FAST_PROBE_RECOVER_ROUNDS)
}

/// What one fast-probe round established, once every piece of evidence the conn
/// produced while the probe was in flight has been folded in.
///
/// The verdict is deliberately not a boolean: "the path is gone", "this probe's
/// budget was too small for the queue in front of it", and "the ping never left
/// the box" are three different facts, and collapsing them is what published
/// 100% loss on a conn that had just served 103 of 103 requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoundOutcome {
    /// Nothing answered and nothing arrived.
    Miss,
    /// The probe never left the box (local congestion). It says nothing about the
    /// path, so it moves neither counter — not even the miss run.
    Inconclusive,
    /// The probe timed out, but the conn was still delivering bytes against a
    /// budget short enough for a queue to explain the timeout. The path is alive
    /// but unproven: the miss run ends, and so does the run of proof.
    DeliveryOnly,
    /// A full round trip completed this round — probe pong, keepalive pong, or
    /// liveness echo. The only evidence that covers **both** directions.
    Proven,
}

/// What the conn's own byte counters say about the round that just finished.
#[derive(Debug, Clone, Copy)]
struct DeliveryEvidence {
    /// Bytes that arrived on this conn while the probe was in flight.
    rx_delta: u64,
    /// Bytes this conn handed to the tunnel during the same window.
    ///
    /// Diagnostic only — deliberately **not** an excuse for a timeout. A dead
    /// peer absorbs sends into the local socket buffer just as readily as a live
    /// one, so `tx > 0` with `rx == 0` is the signature of a black hole, not of
    /// a busy link. Judging liveness by it would keep a dead path in the route
    /// table forever, which is strictly worse than re-pricing a busy one.
    tx_delta: u64,
    /// Egress packets this conn refused over the same window.
    ///
    /// Separates the two ways a round can produce no evidence: a local egress
    /// that is saturated (`egress_drops > 0`, our fault) versus a path that
    /// swallowed the probe (`egress_drops == 0`, the remote's).
    egress_drops: u64,
    /// The wait budget this round actually used.
    budget: Duration,
}

impl DeliveryEvidence {
    /// Did anything arrive from the peer this round?
    fn carried_data(&self) -> bool {
        self.rx_delta > 0
    }

    /// Whether inbound delivery excuses a timeout this round.
    ///
    /// Inbound bytes prove the peer can still reach us — enough to stop accusing
    /// the path of being gone, not enough to call it healthy, because a one-way
    /// outage keeps bytes arriving while carrying nothing we send.
    ///
    /// The excuse is rationed by [`FAST_PROBE_EXCUSE_MAX_BUDGET`] *and* by
    /// [`DeliveryExcuse`]: the widening is worth one episode, and a round that
    /// has already spent it cannot buy another by getting cheaper. The budget
    /// test alone stopped being sufficient once the floor started falling again
    /// on a miss — a budget that drops back under the ceiling re-arms the
    /// excuse, and a one-way outage then alternates `DeliveryOnly` / `Miss`
    /// forever (see [`DeliveryExcuse`]).
    fn excuses_a_timeout(&self, excuse: DeliveryExcuse) -> bool {
        // All three clauses, and the third one is the whole point of
        // [`DeliveryExcuse`]: without it the ratchet is inert and a one-way
        // outage alternates `DeliveryOnly` / `Miss` forever, because the miss
        // halves the floor back under the budget test.
        self.carried_data() && !excuse.spent && self.budget < FAST_PROBE_EXCUSE_MAX_BUDGET
    }
}

/// Per-episode ratchet on the delivery excuse.
///
/// The budget floor is not monotonic: it doubles on an excused timeout and
/// halves on a miss or a proven round. So "is the budget still under
/// [`FAST_PROBE_EXCUSE_MAX_BUDGET`]" cannot decide on its own whether the excuse
/// is still available, and reading it that way makes the exemption
/// self-renewing in exactly the case it exists to catch. A one-way outage pushes
/// bytes at us every round, so its round is excused and the floor doubles to the
/// ceiling; the next round is a miss and halves the floor back under the
/// ceiling; the round after that is excused again. `DeliveryOnly` clears the
/// miss run, so the run never reaches [`FAST_PROBE_MISSES_TO_DEGRADE`] and the
/// half-dead link keeps its edge forever, carrying every packet we originate
/// into the hole.
///
/// `spent` is therefore latched when a round's budget reaches the ceiling and
/// cleared only by a proven round trip — the one piece of evidence that the path
/// carries traffic in **both** directions. It lives in the round fold's state
/// rather than being derived per round, so `fast_probe_apply_round_evidence`
/// stays the single place the decision moves. An `Inconclusive` round — the ping
/// never left the box — neither spends it nor gives it back, because local
/// congestion says nothing about the path.
#[derive(Debug, Clone, Copy, Default)]
struct DeliveryExcuse {
    /// A round in this episode already used the whole widening.
    spent: bool,
}

/// Folds one fast-probe round — the probe's own outcome, any proven round trip
/// that landed during it, and what the byte counters saw — into the miss/hit
/// window, and reports what the round established.
///
/// This is the only place either counter moves, and the only way keepalive
/// evidence may affect the published flag: writing `false` into
/// `fast_probe_degraded` directly is what used to bypass
/// [`FAST_PROBE_RECOVER_ROUNDS`] and make the edge cost flip on every keepalive.
///
/// * [`RoundOutcome::Proven`] — a pong or echo. Clears the miss run **and** earns
///   one hit, because a round trip is the only evidence covering both directions.
/// * [`RoundOutcome::DeliveryOnly`] — the probe timed out while bytes were still
///   arriving. Clears the miss run so a busy-but-alive conn is not accused of
///   being gone, and restarts the run of proof: recovery must be driven by
///   consecutive *proven* rounds, otherwise a link that only ever receives
///   clears its own degradation forever. It also widens the next round's budget
///   — see [`fast_probe_next_budget_floor`] — and spends the episode's excuse
///   once that widening runs out of room — see [`DeliveryExcuse`].
/// * [`RoundOutcome::Inconclusive`] — the ping never left the box. Moves neither
///   counter: local congestion accuses nothing and credits nothing.
/// * [`RoundOutcome::Miss`] — nothing answered and nothing arrived.
///
/// One round is one unit of evidence. A probe that pongs *and* a keepalive that
/// lands in the same round still count once — [`FAST_PROBE_RECOVER_ROUNDS`] is
/// about rounds, not samples. A keepalive hit is also **not** wiped by the same
/// round's probe miss: on a link whose jitter keeps the real RTT above `3 × EWMA`
/// the probe can miss every single round while the keepalive answers every
/// second, and if each miss zeroed `consecutive_hits` that link would stay
/// published as 100% loss forever.
fn fast_probe_apply_round_evidence(
    probe: &Result<PingResponse, Error>,
    roundtrip_epoch: u32,
    seen_epoch: &mut u32,
    delivery: DeliveryEvidence,
    excuse: &mut DeliveryExcuse,
    consecutive_misses: &mut u32,
    consecutive_hits: &mut u32,
) -> RoundOutcome {
    // The epoch is bumped on the keepalive path for every proven round trip. An
    // unchanged value means nothing was proven since the previous round (or the
    // epoch wrapped all the way back, in which case treating it as "nothing new"
    // only delays recovery by one round).
    let epoch_advanced = *seen_epoch != roundtrip_epoch;
    if epoch_advanced {
        *seen_epoch = roundtrip_epoch;
    }

    let outcome = match probe {
        // The ping never left the box: it says nothing about the path, but a
        // keepalive pong in the same round still proves the round trip.
        Ok(PingResponse::LocalCongestion) => {
            if epoch_advanced {
                RoundOutcome::Proven
            } else {
                RoundOutcome::Inconclusive
            }
        }
        // Pong and liveness echo alike: a full round trip completed.
        Ok(_) => RoundOutcome::Proven,
        Err(_) => {
            if epoch_advanced {
                RoundOutcome::Proven
            } else if delivery.excuses_a_timeout(*excuse) {
                RoundOutcome::DeliveryOnly
            } else {
                RoundOutcome::Miss
            }
        }
    };

    // The widening has run out of room: this episode's excuse is spent, whatever
    // the floor does next. Latched here rather than re-derived from the budget,
    // because the floor falls again after a miss and a budget test would hand
    // the excuse straight back (see [`DeliveryExcuse`]). Only a proven round trip
    // ends the episode.
    //
    // `Inconclusive` is excluded on purpose. The ping never left the box, so the
    // round says nothing about the path — the same reason it moves neither
    // counter and neither widens nor retracts the floor. Letting it spend the
    // excuse would make a local egress stall revoke a widening that the *path*
    // earned, and the next three timeouts would accuse the peer for our own
    // queue: exactly the class of false positive this whole change removes.
    match outcome {
        RoundOutcome::Proven => excuse.spent = false,
        RoundOutcome::DeliveryOnly | RoundOutcome::Miss => {
            excuse.spent |= delivery.budget >= FAST_PROBE_EXCUSE_MAX_BUDGET;
        }
        RoundOutcome::Inconclusive => {}
    }

    match outcome {
        RoundOutcome::Proven => {
            *consecutive_misses = 0;
            *consecutive_hits = consecutive_hits.saturating_add(1);
        }
        RoundOutcome::DeliveryOnly => {
            // Break the run of failures — the conn was demonstrably carrying
            // bytes — but do not let that count as proof on the way back.
            *consecutive_misses = 0;
            *consecutive_hits = 0;
        }
        // A probe that never left the box is evidence of nothing, in either
        // direction: it must not accuse the path *or* credit it.
        RoundOutcome::Inconclusive => {}
        RoundOutcome::Miss => {
            *consecutive_misses = consecutive_misses.saturating_add(1);
            *consecutive_hits = 0;
        }
    }

    outcome
}

/// Per-probe wait budget derived from the measured RTT, clamped to this range.
const FAST_PROBE_TIMEOUT_MIN: Duration = Duration::from_millis(300);
const FAST_PROBE_TIMEOUT_MAX: Duration = Duration::from_secs(2);
const FAST_PROBE_TIMEOUT_RTT_MULT: u32 = 3;

/// Probe budget above which inbound delivery no longer excuses a timeout.
///
/// [`FAST_PROBE_TIMEOUT_MIN`] is 300ms, so a congested conn doubles its way to
/// 600ms and 1.2s within two rounds — still inside the three-miss window, which
/// is why a queue that merely outruns the floor never publishes a loss. Past this
/// ceiling the excuse is spent: a conn that keeps timing out against ≥1.2s of
/// budget while only *receiving* is one-way dead, and holding its edge open just
/// strands every packet we send into the same hole.
const FAST_PROBE_EXCUSE_MAX_BUDGET: Duration = Duration::from_millis(1200);

/// Wait budget for a conn that has **never** produced an RTT sample.
///
/// `get_latency_us()` returns 0 both for "no sample yet" and for "measured, but
/// every sample in the window was lost", and the old code treated both as
/// `0µs` -> the 300ms floor. The second case is fine (the link *was* fast, so a
/// short budget detects the outage quickly), but the first is not: 300ms is below
/// the RTT of a relayed or long-haul path, so a brand-new slow-but-healthy conn
/// missed its first three probes and was published as 100% loss — the exact
/// opposite of what the probe is for.
const FAST_PROBE_TIMEOUT_UNMEASURED: Duration = Duration::from_secs(1);

/// Top bit of the ping sequence, reserved for the fast probe.
///
/// `ctrl_resp_sender` is a **broadcast**: every subscriber sees every `Pong`,
/// and a `Pong` is matched by sequence equality alone. Two probe loops on the
/// same conn would therefore satisfy each other's wait as soon as their counters
/// line up — a false *success* from the other loop, which would mask exactly the
/// failure this probe exists to report. Splitting the u32 so the keepalive owns
/// the low 31 bits and the fast probe owns the top bit makes that impossible.
const FAST_PROBE_SEQ_BIT: u32 = 1 << 31;
const SEQ_MASK: u32 = FAST_PROBE_SEQ_BIT - 1;

/// Next sequence for the ordinary keepalive (low 31 bits).
fn keepalive_seq(next: u32) -> u32 {
    next & SEQ_MASK
}

/// Next sequence for the fast probe (top bit set).
fn fast_probe_seq(next: u32) -> u32 {
    FAST_PROBE_SEQ_BIT | (next & SEQ_MASK)
}

/// Wait budget for one fast probe: `max(MIN, MULT × measured RTT)`, capped.
///
/// `rtt_us` is `None` until the conn has produced its first sample; that case
/// gets [`FAST_PROBE_TIMEOUT_UNMEASURED`] instead of the floor, because an
/// unmeasured link must not be judged by a budget shorter than its own RTT.
/// The cap exists because a probe cannot conclude faster than the RTT allows;
/// on a very high-RTT link detection is slower than the typical case by design.
fn fast_probe_timeout(rtt_us: Option<u32>) -> Duration {
    let Some(rtt_us) = rtt_us else {
        return FAST_PROBE_TIMEOUT_UNMEASURED;
    };
    let scaled = Duration::from_micros(u64::from(rtt_us) * u64::from(FAST_PROBE_TIMEOUT_RTT_MULT));
    scaled.clamp(FAST_PROBE_TIMEOUT_MIN, FAST_PROBE_TIMEOUT_MAX)
}

/// Budget floor for the next round, given what this round established.
///
/// This is the fix for the root cause the three-state verdict only papers over: a
/// timeout yields **no** RTT sample, so the `3 × EWMA RTT` the budget is derived
/// from never learns about a queue that formed after the last sample, and the
/// budget stays pinned at its 300ms floor for the whole incident.
///
/// The floor doubles every round in which a timeout is excused by inbound
/// delivery, so the probe is retried against a budget that can actually fit the
/// queue instead of one that never can: measured on this tree, a 4-way download
/// over a 6ms LAN conn queued ICMP to a 663ms p99 — two doublings (600ms, then
/// 1.2s) reach it before the third miss, which is why the degradation is never
/// published at all rather than published and excused.
///
/// Every proven round halves it back, so a conn that recovers does not keep a 2s
/// detection floor for the rest of its life. Both directions are bounded by
/// [`FAST_PROBE_TIMEOUT_MAX`] / [`FAST_PROBE_TIMEOUT_MIN`], the same range
/// `fast_probe_timeout` clamps to, so the two can never disagree on the maximum
/// wait.
///
/// A [`RoundOutcome::Miss`] halves it too, and that half matters as much as the
/// first: the widening is bought by *delivery*, so the first round that delivers
/// nothing — the first round of a real outage — has to start paying it back.
/// Without the decay a conn that was merely congested a moment ago would carry
/// its widened budget into the black hole that follows: 3 misses would take
/// `3 × (333ms + 1.2s) ≈ 4.6s` instead of 1.9s, and the sustained gate
/// `9 × 1.53s ≈ 13.8s` instead of 5.7s, in exactly the situation the probe
/// exists for. With it the widened budget costs at most one extra round
/// (`≈3.1s` to publish) and the floor is back at the minimum by the third miss.
///
/// An [`RoundOutcome::Inconclusive`] round moves nothing: local congestion says
/// nothing about the queue on the path, so it may neither justify the widening
/// nor revoke it.
fn fast_probe_next_budget_floor(current: Duration, outcome: RoundOutcome) -> Duration {
    match outcome {
        // Excused timeout: the queue outran the budget, so widen it instead of
        // retrying against a budget that cannot fit.
        RoundOutcome::DeliveryOnly => (current * 2).min(FAST_PROBE_TIMEOUT_MAX),
        // A proven round says the current budget was enough; give the room back.
        // A miss says the opposite of what the widening was bought for — nothing
        // arrived at all — so give it back as well rather than keep paying a
        // wide budget for a path that may be gone.
        RoundOutcome::Proven | RoundOutcome::Miss => (current / 2).max(FAST_PROBE_TIMEOUT_MIN),
        RoundOutcome::Inconclusive => current,
    }
}

#[derive(Debug)]
enum PingResponse {
    Pong(u128),
    LivenessEcho,
    /// Egress ring was full — local congestion, not a path miss.
    LocalCongestion,
}

struct PingIntervalController {
    throughput: Arc<Throughput>,
    loss_counter: Arc<AtomicU32>,

    interval: Interval,
    max_interval: Duration,

    logic_time: u64,
    last_send_logic_time: u64,

    backoff_idx: i32,
    max_backoff_idx: i32,

    last_throughput: Throughput,
}

impl std::fmt::Debug for PingIntervalController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PingIntervalController")
            .field("throughput", &self.throughput)
            .field("loss_counter", &self.loss_counter)
            .field("logic_time", &self.logic_time)
            .field("last_send_logic_time", &self.last_send_logic_time)
            .field("backoff_idx", &self.backoff_idx)
            .field("max_backoff_idx", &self.max_backoff_idx)
            .field("max_interval", &self.max_interval)
            .field("last_throughput", &self.last_throughput)
            .finish()
    }
}

impl PingIntervalController {
    fn new(
        throughput: Arc<Throughput>,
        loss_counter: Arc<AtomicU32>,
        max_interval: Duration,
    ) -> Self {
        let last_throughput = (*throughput).clone();

        Self {
            throughput,
            loss_counter,
            interval: interval(Duration::from_secs(1)),
            max_interval,
            logic_time: 0,
            last_send_logic_time: 0,

            backoff_idx: 0,
            max_backoff_idx: 5,

            last_throughput,
        }
    }

    async fn tick(&mut self) {
        self.interval.tick().await;
        self.logic_time += 1;
    }

    fn tx_increase(&self) -> bool {
        self.throughput.tx_packets() > self.last_throughput.tx_packets()
    }

    fn rx_increase(&self) -> bool {
        self.throughput.rx_packets() > self.last_throughput.rx_packets()
    }

    fn should_send_ping(&mut self) -> bool {
        if self.loss_counter.load(Ordering::Relaxed) > 0 {
            self.backoff_idx = 0;
        } else if self.tx_increase() && !self.rx_increase() {
            // if tx increase but rx not increase, we should do pingpong more frequently
            self.backoff_idx = 0;
        }

        self.last_throughput = (*self.throughput).clone();

        let send_interval = Duration::from_secs(1 << self.backoff_idx).min(self.max_interval);
        if Duration::from_secs(self.logic_time - self.last_send_logic_time) < send_interval {
            return false;
        }

        self.backoff_idx = std::cmp::min(self.backoff_idx + 1, self.max_backoff_idx);

        // use this makes two peers not pingpong at the same time
        if self.backoff_idx > self.max_backoff_idx - 2 && rng().random_bool(0.2) {
            self.backoff_idx -= 1;
        }

        self.last_send_logic_time = self.logic_time;
        true
    }
}

pub struct PeerConnPinger {
    my_peer_id: PeerId,
    peer_id: PeerId,
    sink: MpscTunnelSender,
    ctrl_sender: broadcast::Sender<ZCPacket>,
    latency_stats: Arc<WindowLatency>,
    loss_rate_stats: Arc<AtomicU32>,
    throughput_stats: Arc<Throughput>,
    context: ArcPeerContext,
    network_name: String,
    liveness: PeerConnLiveness,
    /// Set while the fast probe sees consecutive misses; published as 100% loss
    /// through `PeerConn::loss_rate`, which is what raises the OSPF edge cost and
    /// lets the sender re-route (the conn itself is left alone).
    fast_probe_degraded: Arc<AtomicBool>,
    /// Set once the degradation has lasted `FAST_PROBE_SUSTAINED_ROUNDS`. Only
    /// this one may re-price our own outgoing edge in the local SPF; see that
    /// constant for why that needs a stricter gate than the metric does.
    fast_probe_sustained: Arc<AtomicBool>,
    /// Bumped by every **proven** round trip on the keepalive path (Pong or
    /// liveness echo). Evidence only — never a verdict.
    ///
    /// `fast_probe_degraded` must have exactly one writer: the fast probe, which
    /// owns the miss/hit window and the hysteresis gate. The keepalive used to
    /// `store(false)` into it directly on every successful pong, which bypassed
    /// that gate completely: on a lossy link the probe collected three misses and
    /// published `degraded`, the next keepalive pong cleared it, and three probes
    /// later it was published again. That is two flips per keepalive interval,
    /// each one a new `loss_rate()` → a new edge cost → a new `report_peers`
    /// payload → another SPF run, no matter how much hysteresis the probe had.
    ///
    /// So the keepalive now just bumps this counter, and the fast probe folds it
    /// into the same gate as one of its own hits.
    roundtrip_epoch: Arc<AtomicU32>,
    close_notify: Arc<PeerConnCloseNotify>,
    max_interval: Duration,
    /// Consecutive ping failures before closing; default 10 (see `DEFAULT_PING_FAIL_CLOSE_COUNT`).
    fail_close_count: u32,
    /// Optional P1.5 gate: when set, reaching `fail_close_count` only closes if
    /// the gate returns true (peer still has a verified alternative), for at most
    /// [`LAST_PATH_HOLD_MAX`] past the threshold, and only for a conn that has
    /// already round-tripped at least once. `None` keeps the legacy always-close
    /// behaviour (tests / fast_probe).
    close_gate: Option<PingFailCloseGate>,
}

impl std::fmt::Debug for PeerConnPinger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerConnPinger")
            .field("my_peer_id", &self.my_peer_id)
            .field("peer_id", &self.peer_id)
            .finish()
    }
}

impl PeerConnPinger {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        my_peer_id: PeerId,
        peer_id: PeerId,
        sink: MpscTunnelSender,
        ctrl_sender: broadcast::Sender<ZCPacket>,
        latency_stats: Arc<WindowLatency>,
        loss_rate_stats: Arc<AtomicU32>,
        throughput_stats: Arc<Throughput>,
        context: ArcPeerContext,
        network_name: String,
        liveness: PeerConnLiveness,
        fast_probe_degraded: Arc<AtomicBool>,
        fast_probe_sustained: Arc<AtomicBool>,
        roundtrip_epoch: Arc<AtomicU32>,
        close_notify: Arc<PeerConnCloseNotify>,
        max_interval: Duration,
        fail_close_count: u32,
        close_gate: Option<PingFailCloseGate>,
    ) -> Self {
        Self {
            my_peer_id,
            peer_id,
            sink,
            latency_stats,
            ctrl_sender,
            loss_rate_stats,
            throughput_stats,
            context,
            network_name,
            liveness,
            fast_probe_degraded,
            fast_probe_sustained,
            roundtrip_epoch,
            close_notify,
            max_interval,
            fail_close_count: fail_close_count.max(1),
            close_gate,
        }
    }

    fn new_ping_packet(my_node_id: PeerId, peer_id: PeerId, seq: u32) -> ZCPacket {
        let mut packet = ZCPacket::new_with_payload(&seq.to_le_bytes());
        packet.fill_peer_manager_hdr(my_node_id, peer_id, PacketType::Ping as u8);
        packet
    }

    async fn do_pingpong_once(
        &self,
        receiver: &mut broadcast::Receiver<ZCPacket>,
        seq: u32,
        liveness_token: Option<u8>,
        wait_timeout: Duration,
    ) -> Result<PingResponse, Error> {
        // should add seq here. so latency can be calculated more accurately
        let req = Self::new_ping_packet(self.my_peer_id, self.peer_id, seq);
        let req_len = req.buf_len() as u64;
        // A full data ring must not be scored as a *path* miss, but it must not
        // be reported as local congestion on the first `BufferFull` either.
        //
        // Measured 2026-11-09 (`.16`, line rate 113 MB/s): the 32-slot egress
        // queue is full *continuously* for as long as the link stays saturated
        // — data keeps getting in through `send().await` (max wait 5 ms) while
        // the keepalive's `try_send` failed on essentially every round
        // (`enq_full` +3/s, `enq_blocked_max_ms=5`). Declaring congestion
        // immediately therefore turns a healthy saturated link into 30-40 s of
        // `egress full for too long` and then a teardown, on both ends.
        //
        // Slots free every few microseconds at line rate, so a short bounded
        // wait gets the keepalive through without letting the data plane hold
        // it hostage. Only a wait that actually times out is congestion. The
        // packet has to be rebuilt because `try_send` consumes it on failure.
        match self.sink.try_send(req) {
            Ok(()) => {}
            Err(TunnelError::BufferFull) => {
                const CONTROL_EGRESS_WAIT: Duration = Duration::from_millis(50);
                let req = Self::new_ping_packet(self.my_peer_id, self.peer_id, seq);
                match tokio::time::timeout(CONTROL_EGRESS_WAIT, self.sink.send(req)).await {
                    // Delivered. The time spent queued is part of the RTT sample
                    // by design, same as the `try_send` fast path.
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => return Ok(PingResponse::LocalCongestion),
                }
            }
            Err(e) => return Err(e.into()),
        }
        // RTT starts after enqueue (queue drain until wire write remains in the
        // sample by design — that is the observed delivery delay).
        let now = Instant::now();
        self.context.record_control_tx(&self.network_name, req_len);

        let wait_for_pong = async {
            loop {
                match receiver.recv().await {
                    Ok(p) => {
                        let payload = p.payload();
                        // CRITICAL (CR-10): `payload[0..4]` panics when the peer
                        // sends a Pong shorter than 4 bytes, and `start_recv_loop`
                        // forwards *any* `packet_type == Pong` packet here with no
                        // length check. The `try_into()` was meant to be the guard
                        // but it runs on the slice, which is already indexed.
                        //
                        // The blast radius is worse than a dead task: the panic
                        // happens before `notify_close()`, so the conn is never
                        // closed, `has_live_conns()` keeps counting it, the direct
                        // connector will not re-dial ("already has a conn"), and
                        // `latency_us` stays 0 — which is the *best possible*
                        // score, so `score_conn` keeps selecting this dead path.
                        let Some(seq_bytes) = payload.get(..4) else {
                            tracing::debug!("pingpong recv short packet, continue");
                            continue;
                        };
                        let Ok(seq_buf) = <[u8; 4]>::try_from(seq_bytes) else {
                            tracing::debug!("pingpong recv invalid packet, continue");
                            continue;
                        };
                        let resp_seq = u32::from_le_bytes(seq_buf);
                        if resp_seq == seq {
                            break;
                        }
                    }
                    Err(e) => {
                        return Err(Error::WaitRespError(format!(
                            "wait ping response error: {:?}",
                            e
                        )));
                    }
                }
            }
            Ok(())
        };
        let resp = timeout(wait_timeout, async {
            if let Some(token) = liveness_token {
                tokio::select! {
                    ret = wait_for_pong => ret.map(|()| PingResponse::Pong(now.elapsed().as_micros())),
                    () = self.liveness.wait_for_echo(token) => Ok(PingResponse::LivenessEcho),
                }
            } else {
                wait_for_pong
                    .await
                    .map(|()| PingResponse::Pong(now.elapsed().as_micros()))
            }
        })
        .await;

        tracing::trace!(?resp, "wait ping response done");

        if resp.is_err() {
            return Err(Error::WaitRespError(
                "wait ping response timeout".to_owned(),
            ));
        }

        resp.unwrap()
    }

    pub async fn pingpong(&mut self) {
        let my_node_id = self.my_peer_id;

        // one with 1% precision
        let loss_rate_stats_1 = WindowLatency::new(100);
        // disconnect after N consecutive pingpong failures (configurable; default 5)
        let loss_counter = Arc::new(AtomicU32::new(0));
        let fail_close_count = self.fail_close_count;

        let (trigger_sender, mut trigger_receiver) = tokio::sync::mpsc::channel(1);
        let mut controller_tasks = JoinSet::new();
        let throughput = self.throughput_stats.clone();
        let controller_loss_counter = loss_counter.clone();
        let max_interval = self.max_interval;
        controller_tasks.spawn(async move {
            let mut controller =
                PingIntervalController::new(throughput, controller_loss_counter, max_interval);
            loop {
                controller.tick().await;
                if !controller.should_send_ping() {
                    continue;
                }
                match trigger_sender.try_send(()) {
                    Ok(()) | Err(TrySendError::Full(())) => {}
                    Err(TrySendError::Closed(())) => break,
                }
            }
        });

        let mut req_seq = 0u32;
        // P1.5 bookkeeping: has this conn ever completed a round trip, and when
        // did the current `fail_close_count`-crossing hold start?
        let mut had_roundtrip = false;
        let mut last_path_hold: Option<Instant> = None;
        // Since when every ping attempt has been rejected by a full egress ring.
        let mut congestion_since: Option<Instant> = None;
        while trigger_receiver.recv().await.is_some() {
            tracing::debug!(
                "pingpong controller send pingpong task, seq: {}, node_id: {}",
                req_seq,
                my_node_id,
            );

            let liveness_token = (loss_counter.load(Ordering::Relaxed) > 0)
                .then(|| self.liveness.start_probe())
                .flatten();
            let mut receiver = self.ctrl_sender.subscribe();
            let ret = self
                .do_pingpong_once(&mut receiver, req_seq, liveness_token, PING_WAIT_TIMEOUT)
                .await;
            req_seq = keepalive_seq(req_seq.wrapping_add(1));

            // A closed transport is not a path: the sink can no longer carry a
            // ping, so no amount of keepalive will revive it. Close immediately
            // instead of counting 10 misses' worth of wait — during which
            // `has_live_conns()` stays true and the direct connector refuses to
            // re-dial (`direct peers=[]`), which is exactly the failover delay
            // the ping-fail threshold is not supposed to add.
            let transport_gone = matches!(&ret, Err(Error::Tunnel(TunnelError::Shutdown)))
                || self.close_notify.is_closed();
            if transport_gone {
                tracing::warn!(
                    ?ret,
                    ?self,
                    req_seq,
                    "pingpong transport closed, closing the connection"
                );
                break;
            }

            if let Ok(response) = &ret {
                match response {
                    // Local egress full: not a path miss and not a round-trip proof.
                    // Bounded on its own clock so a stuck ring cannot masquerade as
                    // a healthy link forever (see `LOCAL_CONGESTION_CLOSE_AFTER`).
                    PingResponse::LocalCongestion => {
                        let since = *congestion_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= LOCAL_CONGESTION_CLOSE_AFTER {
                            tracing::warn!(
                                ?self,
                                congested_ms = since.elapsed().as_millis() as u64,
                                "pingpong egress full for too long, closing the connection"
                            );
                            break;
                        }
                    }
                    // A liveness echo proves reachability, not prompt delivery of a
                    // queued packet. Only a matching Pong supplies an RTT sample.
                    PingResponse::Pong(lat) => {
                        congestion_since = None;
                        self.latency_stats.record_latency(*lat as u32);
                        if let Some(token) = liveness_token {
                            self.liveness.finish_probe(token);
                        }
                        loss_rate_stats_1.record_latency(0);
                        loss_counter.store(0, Ordering::Relaxed);
                        had_roundtrip = true;
                        last_path_hold = None;
                        // Evidence, not a verdict: the fast probe owns
                        // `fast_probe_degraded` (see `roundtrip_epoch`).
                        self.roundtrip_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                    PingResponse::LivenessEcho => {
                        congestion_since = None;
                        if let Some(token) = liveness_token {
                            self.liveness.finish_probe(token);
                        }
                        loss_rate_stats_1.record_latency(0);
                        loss_counter.store(0, Ordering::Relaxed);
                        had_roundtrip = true;
                        last_path_hold = None;
                        // Evidence, not a verdict: the fast probe owns
                        // `fast_probe_degraded` (see `roundtrip_epoch`).
                        self.roundtrip_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            } else {
                // A real miss (timeout / closed sink) ends the congestion window:
                // the two explanations for "no Pong" must not be mixed, or a link
                // that is genuinely dying could be excused as "just busy".
                congestion_since = None;
                loss_rate_stats_1.record_latency(1);
                loss_counter.fetch_add(1, Ordering::Relaxed);
            }

            let loss_rate_1: f64 = loss_rate_stats_1.get_latency_us();

            tracing::trace!(
                ?ret,
                ?self,
                ?loss_rate_1,
                "pingpong task recv pingpong_once result"
            );

            tracing::debug!(
                "loss_counter: {:?}, loss_rate_1: {}, node_id: {}",
                loss_counter,
                loss_rate_1,
                my_node_id
            );

            let losses = loss_counter.load(Ordering::Relaxed);
            if losses >= fail_close_count {
                // P1.5: prefer keeping the last path through brief jitter when no
                // verified alternative exists. Bounded twice over, because an
                // open-ended hold is a zombie: `has_live_conns()` keeps counting
                // it, the direct connector will not re-dial, and credential
                // failover never converges.
                //
                //   1. only a conn that has answered at least one ping is a path
                //      worth holding. A conn that never round-tripped is a dial
                //      attempt that never worked; holding it open only keeps
                //      `has_directly_connected_conn()` true, which stops the
                //      connector from re-dialing (the credential tests' 30s
                //      `wait_direct_peer` then reports `direct peers=[]`).
                //   2. the hold itself is a wall-clock budget, not a miss count
                //      (`LAST_PATH_HOLD_MAX`).
                let hold_applies = had_roundtrip;
                let hold_started = if hold_applies {
                    *last_path_hold.get_or_insert_with(Instant::now)
                } else {
                    // Not applicable: skip straight to "close", as before P1.5.
                    Instant::now()
                };
                let hold_expired = !hold_applies || hold_started.elapsed() >= LAST_PATH_HOLD_MAX;
                let allowed = if hold_expired {
                    true
                } else {
                    self.close_gate.as_ref().map(|gate| gate()).unwrap_or(true)
                };
                if !allowed {
                    // Debug, not warn: this repeats every keepalive round for the
                    // whole hold and would flood exactly during the incident it
                    // explains (see stability plan §3.4).
                    tracing::debug!(
                        ?ret,
                        ?self,
                        ?loss_rate_1,
                        ?loss_counter,
                        fail_close_count,
                        hold_elapsed_ms = hold_started.elapsed().as_millis() as u64,
                        "ping fail close skipped: no verified alternative PeerConn (keeping last path)",
                    );
                    // Stay at/above threshold so every further miss re-evaluates the
                    // gate (an alternative may appear) or expires the hold.
                    continue;
                }
                // One line per close, not two: the hold expiry used to emit its
                // own "closing the connection" warn right before this one, which
                // read like two separate events. `hold_*` are fields here so the
                // decision (consecutive misses + gate + hold) stays visible.
                tracing::warn!(
                    ?ret,
                    ?self,
                    ?loss_counter,
                    fail_close_count,
                    held_last_path = hold_applies,
                    hold_expired = hold_applies && hold_expired,
                    hold_ms = hold_applies.then(|| LAST_PATH_HOLD_MAX.as_millis() as u64),
                    // Windowed loss over the last 100 keepalive rounds. The close
                    // is decided by `fail_close_count` *consecutive* misses, so
                    // this percentage can legitimately be low when the path is
                    // flapping — do not read it as the close trigger.
                    windowed_loss_rate_1 = loss_rate_1,
                    "too many consecutive pingpong failures, closing the connection",
                );
                break;
            }

            // Percent, rounded — `as u32` truncated, so 29.9% was published as
            // 29% and the same for 57.9% / 58.9%. The loss term is weighted
            // heavily, so a systematic 1-point under-report is not noise.
            self.loss_rate_stats.store(
                (loss_rate_1 * 100.0).clamp(0.0, 100.0).round() as u32,
                Ordering::Relaxed,
            );
        }
    }

    /// Fast liveness probe — roadmap `live-next-hop.md` Phase F.
    ///
    /// Publishes "degraded" (100% loss) as soon as the link stops round-tripping,
    /// which flows `loss_rate` → `score_conn` (fused) →
    /// `ospf_edge_cost_from_score` (+`OSPF_FUSE_COST_BONUS`) → a re-published edge
    /// cost, so OSPF re-routes on the next SPF. That is what turns §10's
    /// ~2 minute black hole into a sub-2-second re-route.
    ///
    /// Deliberately limited to the *metric*: it never closes the conn, never
    /// touches `loss_counter`, and therefore does not change the
    /// `ping_fail_close_count` timing.
    ///
    /// Gated on the negotiated `liveness-echo-v1` feature. Failing closed the
    /// other way round matters: without an echo the far side may legitimately be
    /// slower than our shortened timeout, so an un-negotiated peer is never
    /// judged by it.
    ///
    /// **Sole writer of `fast_probe_degraded` / `fast_probe_sustained`.** The
    /// published value therefore only ever changes on a real decision flip, which
    /// is what keeps the whole downstream chain transition-driven: `loss_rate()`
    /// → `score_conn` → `ospf_edge_cost_from_score` → `direct_peer_info` (with its
    /// publisher hysteresis) → `report_can_be_skipped` → the center's global map
    /// digest → `RouteCostCalculatorImpl::need_update` → SPF. Those consumers all
    /// re-derive and diff, so they already do nothing when the cost is unchanged;
    /// what used to defeat them was the *value* flipping underneath a single
    /// keepalive pong, not the polling. Keepalive evidence now arrives through
    /// `roundtrip_epoch` and is folded in below as one hit.
    pub async fn fast_probe(&mut self) {
        let mut seq = 0u32;
        let mut misses = 0u32;
        let mut hits = 0u32;
        let mut was_degraded = false;
        let mut was_sustained = false;
        let mut seen_roundtrip_epoch = self.roundtrip_epoch.load(Ordering::Relaxed);
        // Adaptive probe budget floor. The `3 × EWMA RTT` budget is derived from
        // samples, and a timeout produces none, so on its own it stays pinned at
        // [`FAST_PROBE_TIMEOUT_MIN`] for the whole incident. This is what widens
        // it — see [`fast_probe_next_budget_floor`].
        let mut budget_floor = FAST_PROBE_TIMEOUT_MIN;
        // Per-episode ratchet on the delivery excuse. It cannot be derived from
        // the budget because the floor falls again on a miss; see
        // [`DeliveryExcuse`].
        let mut excuse = DeliveryExcuse::default();
        loop {
            sleep(FAST_PROBE_INTERVAL).await;

            if self.close_notify.is_closed() {
                break;
            }
            if !self.liveness.is_enabled() {
                // Not negotiated (or not yet): stay out of the way and make sure
                // we never leave a stale degraded reading behind.
                self.fast_probe_degraded.store(false, Ordering::Relaxed);
                self.fast_probe_sustained.store(false, Ordering::Relaxed);
                misses = 0;
                hits = 0;
                // Don't bank proof the keepalive earned while we weren't
                // probing: only round trips that land during a probe round count.
                seen_roundtrip_epoch = self.roundtrip_epoch.load(Ordering::Relaxed);
                was_degraded = false;
                // Nothing was measured while the probe was parked, so it must
                // come back at the floor rather than resuming a widened budget —
                // and with a fresh excuse, because an episode that ended while we
                // were not looking must not carry its spent ratchet into the next.
                budget_floor = FAST_PROBE_TIMEOUT_MIN;
                excuse = DeliveryExcuse::default();
                continue;
            }

            let mut receiver = self.ctrl_sender.subscribe();
            // `None` until the conn has produced its first sample: an unmeasured
            // link must not be timed with a budget shorter than its own RTT.
            let rtt_us = (self.latency_stats.sample_count() > 0)
                .then(|| self.latency_stats.get_latency_us::<u32>());
            // No liveness token: the echo slot is single-slot and shared with the
            // keepalive, and a plain Ping/Pong answers the same question.
            //
            // Sampled per round, before the probe: the byte counters only mean
            // something for the window the probe occupied.
            let rx_bytes_before = self.throughput_stats.rx_bytes();
            let tx_bytes_before = self.throughput_stats.tx_bytes();
            let egress_drops_before = self.throughput_stats.egress_drops();
            // `3 × EWMA RTT` cannot learn about a queue it never sampled, so the
            // adaptive floor is what actually widens this when a timeout keeps
            // being excused by delivery.
            let budget = fast_probe_timeout(rtt_us).max(budget_floor);
            let probe = self
                .do_pingpong_once(&mut receiver, fast_probe_seq(seq), None, budget)
                .await;
            let delivery = DeliveryEvidence {
                rx_delta: self
                    .throughput_stats
                    .rx_bytes()
                    .saturating_sub(rx_bytes_before),
                tx_delta: self
                    .throughput_stats
                    .tx_bytes()
                    .saturating_sub(tx_bytes_before),
                egress_drops: self
                    .throughput_stats
                    .egress_drops()
                    .saturating_sub(egress_drops_before),
                budget,
            };
            seq = seq.wrapping_add(1);

            // One round of evidence, folded through a single gate rather than
            // written straight into the flag — without this, one pong cleared a
            // published degradation outright and the probe re-published it three
            // rounds later, the flip-flop that kept re-pricing the edge. See
            // `fast_probe_apply_round_evidence` for why a proven round outranks
            // this round's probe miss.
            let outcome = fast_probe_apply_round_evidence(
                &probe,
                self.roundtrip_epoch.load(Ordering::Relaxed),
                &mut seen_roundtrip_epoch,
                delivery,
                &mut excuse,
                &mut misses,
                &mut hits,
            );
            budget_floor = fast_probe_next_budget_floor(budget_floor, outcome);

            let degraded = fast_probe_publish_degraded(misses, hits, was_degraded);
            self.fast_probe_degraded.store(degraded, Ordering::Relaxed);
            // `misses` restarts at 0 on every `Proven` or `DeliveryOnly` round, so
            // the sustained flag can only be reached by one unbroken run of
            // misses, and is dropped by the same evidence that drops `degraded`.
            // An `Inconclusive` round (local congestion) neither adds to the run
            // nor breaks it.
            let sustained = fast_probe_is_sustained(misses);
            self.fast_probe_sustained
                .store(sustained, Ordering::Relaxed);

            // Log the transitions only: at ~3 probes/s a per-probe log would
            // flood exactly during the incident it is meant to explain.
            //
            // The four counters below are what separates the remaining causes
            // at a glance:
            //   rx=0, tx=0, drops=0   → nothing moved: a black hole.
            //   rx=0, tx>0, drops=0   → we are feeding a peer that never
            //                           answers; its socket buffer is absorbing
            //                           our sends. Still a black hole.
            //   rx>0, budget at floor → the queue outran the budget.
            //   drops>0               → local egress is saturated, so the probe
            //                           starved here rather than on the wire.
            //
            // `budget_ms` is the budget **this** round waited with, i.e. the one
            // that just timed out; `next_budget_floor_ms` is the floor the *next*
            // round will start from, already updated above. They are adjacent on
            // purpose — the gap between them is the widening (or the decay) the
            // verdict just bought — but they are not the same number, and naming
            // both `budget*` invited exactly that misreading.
            if degraded && !was_degraded {
                tracing::warn!(
                    peer_id = self.peer_id,
                    misses,
                    ?rtt_us,
                    budget_ms = budget.as_millis() as u64,
                    next_budget_floor_ms = budget_floor.as_millis() as u64,
                    rx_delta = delivery.rx_delta,
                    tx_delta = delivery.tx_delta,
                    egress_drops = delivery.egress_drops,
                    "fast liveness probe: link not round-tripping, publishing degraded loss"
                );
            } else if !degraded && was_degraded {
                tracing::info!(
                    peer_id = self.peer_id,
                    ?rtt_us,
                    budget_ms = budget.as_millis() as u64,
                    "fast liveness probe: link round-trips again"
                );
            }
            // Separate line: this is the one that makes the **local** SPF stop
            // pricing this edge, so it is the log to grep for when asking "why
            // did traffic move off a link that still shows as connected".
            if sustained != was_sustained {
                if sustained {
                    tracing::warn!(
                        peer_id = self.peer_id,
                        misses,
                        ?rtt_us,
                        budget_ms = budget.as_millis() as u64,
                        next_budget_floor_ms = budget_floor.as_millis() as u64,
                        rx_delta = delivery.rx_delta,
                        tx_delta = delivery.tx_delta,
                        egress_drops = delivery.egress_drops,
                        "fast liveness probe: degradation sustained, local edge cost will be overridden"
                    );
                } else {
                    tracing::info!(
                        peer_id = self.peer_id,
                        misses,
                        ?rtt_us,
                        "fast liveness probe: sustained degradation cleared, restoring local edge cost"
                    );
                }
            }
            was_degraded = degraded;
            was_sustained = sustained;
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};

    use super::*;
    use crate::{
        packet::PacketType,
        peers::conn::{peer_conn::PeerConnId, peer_conn_liveness::PeerConnLiveness},
        peers::test_support::NoopPeerContext,
        tunnel::{
            Tunnel, filter::TunnelWithFilter, mpsc::MpscTunnel, ring::create_ring_tunnel_pair,
        },
    };

    /// A round's delivery evidence at an explicit budget, so each test states the
    /// one thing the verdict actually turns on.
    fn delivery(carried_data: bool, budget_ms: u64) -> DeliveryEvidence {
        DeliveryEvidence {
            rx_delta: if carried_data { 1 } else { 0 },
            tx_delta: 0,
            egress_drops: 0,
            budget: Duration::from_millis(budget_ms),
        }
    }

    /// Drives the `fast_probe` loop body round by round — budget from the floor,
    /// fold, move the floor, decide what to publish — with the caller describing
    /// what the path does.
    ///
    /// The unit tests above pin each rule; this pins their **composition**, which
    /// is where the interesting failures live. Every rule can be individually
    /// correct while the loop oscillates: an excused round clears the miss run,
    /// so any rule that lets the excuse come back after the floor has fallen
    /// re-opens the run forever and the degradation is never published at all.
    ///
    /// `rtt_budget` stands in for `fast_probe_timeout(rtt_us)`; `pong_at` is the
    /// delay the path actually imposes (`None` = it never answers).
    struct ProbeSim {
        rtt_budget: Duration,
        floor: Duration,
        excuse: DeliveryExcuse,
        misses: u32,
        hits: u32,
        seen_epoch: u32,
        degraded: bool,
        elapsed: Duration,
        /// Widest budget any round has actually waited with — the property that
        /// matters ("did the probe ever get a window wide enough for the queue"),
        /// as opposed to `floor`, which oscillates and would make an assertion on
        /// it depend on how many rounds the test happens to run.
        max_budget: Duration,
    }

    impl ProbeSim {
        /// Cold start: floor at the minimum, nothing proven yet.
        fn cold() -> Self {
            Self::at_floor(FAST_PROBE_TIMEOUT_MIN)
        }

        fn at_floor(floor: Duration) -> Self {
            Self {
                // The incident conn measured `rtt_us = 6008`, so `3 × RTT` was
                // clamped to the floor and the floor was the whole budget.
                rtt_budget: FAST_PROBE_TIMEOUT_MIN,
                floor,
                excuse: DeliveryExcuse::default(),
                misses: 0,
                hits: 0,
                seen_epoch: 0,
                degraded: false,
                elapsed: Duration::ZERO,
                max_budget: Duration::ZERO,
            }
        }

        fn round(&mut self, carried_data: bool, pong_at: Option<Duration>) -> RoundOutcome {
            let budget = self.rtt_budget.max(self.floor);
            self.max_budget = self.max_budget.max(budget);
            // Wall clock is the interval plus the wait this round actually spent.
            self.elapsed += FAST_PROBE_INTERVAL + budget;

            let probe: Result<PingResponse, Error> = match pong_at {
                Some(delay) if budget >= delay => {
                    self.seen_epoch += 1;
                    Ok(PingResponse::Pong(1_000))
                }
                _ => Err(Error::WaitRespError(
                    "wait ping response timeout".to_owned(),
                )),
            };
            let epoch = self.seen_epoch;
            let outcome = fast_probe_apply_round_evidence(
                &probe,
                epoch,
                &mut self.seen_epoch,
                delivery(carried_data, budget.as_millis() as u64),
                &mut self.excuse,
                &mut self.misses,
                &mut self.hits,
            );
            self.floor = fast_probe_next_budget_floor(self.floor, outcome);
            self.degraded = fast_probe_publish_degraded(self.misses, self.hits, self.degraded);
            outcome
        }
    }

    #[tokio::test]
    async fn full_egress_queue_is_local_congestion_not_a_path_miss() {
        use crate::tunnel::ring::{RingTunnel, create_ring_socket_pair};

        let (left, right) = create_ring_socket_pair(1);
        let tunnel = MpscTunnel::new(RingTunnel::new(left, None), None);
        let _remote = RingTunnel::new(right, None);
        let sender = tunnel.get_sink();
        // Fill both the underlying ring and the MPSC queue without draining.
        assert!(
            timeout(Duration::from_millis(10), async {
                loop {
                    sender
                        .send(ZCPacket::new_with_payload(b"queued business"))
                        .await
                        .unwrap();
                }
            })
            .await
            .is_err()
        );

        let (ctrl_sender, mut receiver) = broadcast::channel(16);
        let pinger = PeerConnPinger::new(
            1,
            2,
            sender,
            ctrl_sender,
            Arc::new(WindowLatency::new(15)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(Throughput::new()),
            Arc::new(NoopPeerContext::default()),
            "test".to_owned(),
            PeerConnLiveness::new(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(PeerConnCloseNotify::new(PeerConnId::new_v4())),
            Duration::from_secs(1),
            5,
            None,
        );
        // try_send + LocalCongestion: must not block behind a full data ring, and
        // must not be scored as a keepalive/path miss by callers.
        let response = timeout(
            Duration::from_millis(50),
            pinger.do_pingpong_once(&mut receiver, 1, None, PING_WAIT_TIMEOUT),
        )
        .await
        .expect("congestion probe must not wait on a full egress queue")
        .unwrap();
        assert!(
            matches!(response, PingResponse::LocalCongestion),
            "expected LocalCongestion, got {response:?}"
        );
    }

    #[cfg(not(target_os = "wasi"))]
    #[tokio::test(start_paused = true)]
    async fn one_second_limit_disables_ping_backoff() {
        let mut controller = PingIntervalController::new(
            Arc::new(Throughput::new()),
            Arc::new(AtomicU32::new(0)),
            Duration::from_secs(1),
        );

        let started_at = tokio::time::Instant::now();
        for second in 0..100 {
            controller.tick().await;
            assert_eq!(started_at.elapsed(), Duration::from_secs(second));
            assert!(controller.should_send_ping());
            assert!(!controller.should_send_ping());
        }
    }

    #[cfg(not(target_os = "wasi"))]
    #[tokio::test(start_paused = true)]
    async fn ping_backoff_respects_non_power_of_two_limit() {
        let mut controller = PingIntervalController::new(
            Arc::new(Throughput::new()),
            Arc::new(AtomicU32::new(0)),
            Duration::from_secs(3),
        );

        for tick in 1..=100 {
            controller.tick().await;
            assert_eq!(controller.should_send_ping(), tick == 1 || tick % 3 == 0);
        }
    }

    #[cfg(not(target_os = "wasi"))]
    #[tokio::test(start_paused = true)]
    async fn default_limit_preserves_backoff_and_loss_retries() {
        let loss_counter = Arc::new(AtomicU32::new(0));
        let mut controller = PingIntervalController::new(
            Arc::new(Throughput::new()),
            loss_counter.clone(),
            Duration::from_secs(32),
        );

        for second in 1..=14 {
            controller.tick().await;
            assert_eq!(controller.should_send_ping(), matches!(second, 1 | 3 | 7));
        }

        loss_counter.store(1, Ordering::Relaxed);
        for _ in 0..3 {
            controller.tick().await;
            assert!(controller.should_send_ping());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ingress_traffic_does_not_mask_failed_round_trips() {
        let (local_tunnel, _remote_tunnel) = create_ring_tunnel_pair();
        let tunnel = MpscTunnel::new(local_tunnel, None);
        let (ctrl_sender, _) = broadcast::channel(16);
        let throughput = Arc::new(Throughput::new());
        let mut pinger = PeerConnPinger::new(
            1,
            2,
            tunnel.get_sink(),
            ctrl_sender,
            Arc::new(WindowLatency::new(15)),
            Arc::new(AtomicU32::new(0)),
            throughput.clone(),
            Arc::new(NoopPeerContext::default()),
            "test".to_owned(),
            PeerConnLiveness::new(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(PeerConnCloseNotify::new(PeerConnId::new_v4())),
            Duration::from_secs(32),
            5,
            None,
        );

        let ingress = tokio::spawn(async move {
            loop {
                crate::foundation::time::sleep(Duration::from_millis(100)).await;
                throughput.record_rx_bytes(1);
            }
        });

        let result = timeout(Duration::from_secs(12), pinger.pingpong()).await;
        ingress.abort();

        assert!(
            result.is_ok(),
            "unrelated ingress traffic kept a failed round-trip alive"
        );
    }

    #[cfg_attr(
        not(target_os = "wasi"),
        tokio::test(flavor = "multi_thread", worker_threads = 2)
    )]
    #[cfg_attr(target_os = "wasi", tokio::test)]
    async fn echoed_business_traffic_keeps_connection_alive_when_pongs_are_lost() {
        let local_liveness = PeerConnLiveness::new();
        let remote_liveness = PeerConnLiveness::new();
        local_liveness.set_enabled(true);
        remote_liveness.set_enabled(true);

        let (local_transport, remote_transport) = create_ring_tunnel_pair();
        let local_transport = TunnelWithFilter::new(local_transport, local_liveness.clone());
        let mut local_tunnel = MpscTunnel::new(local_transport, None);
        let mut local_stream = local_tunnel.get_stream();
        let local_reader =
            tokio::spawn(async move { while local_stream.next().await.is_some() {} });

        let remote_transport = TunnelWithFilter::new(remote_transport, remote_liveness);
        let (mut remote_stream, mut remote_sink) = remote_transport.split();
        let remote = tokio::spawn(async move {
            while let Some(Ok(_packet)) = remote_stream.next().await {
                let mut reply = ZCPacket::new_with_payload(b"business traffic");
                reply.fill_peer_manager_hdr(2, 1, PacketType::Data as u8);
                remote_sink.send(reply).await.unwrap();
            }
        });

        let (ctrl_sender, _) = broadcast::channel(16);
        let mut pinger = PeerConnPinger::new(
            1,
            2,
            local_tunnel.get_sink(),
            ctrl_sender,
            Arc::new(WindowLatency::new(15)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(Throughput::new()),
            Arc::new(NoopPeerContext::default()),
            "test".to_owned(),
            local_liveness,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(PeerConnCloseNotify::new(PeerConnId::new_v4())),
            Duration::from_secs(32),
            5,
            None,
        );

        let result = timeout(Duration::from_secs(12), pinger.pingpong()).await;
        remote.abort();
        local_reader.abort();

        assert!(
            result.is_err(),
            "matching business-packet echoes did not keep the connection alive"
        );
    }

    /// Regression guard for the credential failover timeouts: P1.5 used to hold
    /// the last path for a fixed number of *misses* regardless of whether the
    /// conn had ever worked. A dial attempt that never round-tripped then stayed
    /// in `has_live_conns()` for the whole hold, the direct connector refused to
    /// re-dial, and tests waiting on a fresh direct conn (30s budget) saw
    /// `direct peers=[]`.
    #[tokio::test(flavor = "current_thread")]
    async fn never_round_tripped_conn_closes_even_when_gate_denies() {
        let (local_tunnel, _remote_tunnel) = create_ring_tunnel_pair();
        let tunnel = MpscTunnel::new(local_tunnel, None);
        let (ctrl_sender, _) = broadcast::channel(16);
        // A gate that always denies: "there is no verified alternative" — the
        // exact condition under which the hold used to apply unconditionally.
        let deny_gate: PingFailCloseGate = Arc::new(|| false);
        let mut pinger = PeerConnPinger::new(
            1,
            2,
            tunnel.get_sink(),
            ctrl_sender,
            Arc::new(WindowLatency::new(15)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(Throughput::new()),
            Arc::new(NoopPeerContext::default()),
            "test".to_owned(),
            PeerConnLiveness::new(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(PeerConnCloseNotify::new(PeerConnId::new_v4())),
            Duration::from_secs(32),
            2,
            Some(deny_gate),
        );

        let result = timeout(Duration::from_secs(12), pinger.pingpong()).await;

        assert!(
            result.is_ok(),
            "a conn that never round-tripped must still close at the threshold"
        );
    }

    #[test]
    fn may_tear_down_requires_verified_unfused_alternative() {
        use uuid::Uuid;
        let cfg = ConnSelectConfig::default();
        let healthy = ConnMetrics {
            conn_id: Uuid::from_bytes([1; 16]),
            latency_us: 5_000,
            loss_rate: 0.0,
            jitter_us: 0,
            is_hole_punched: false,
        };
        let unverified_punch = ConnMetrics {
            conn_id: Uuid::from_bytes([2; 16]),
            latency_us: 0,
            loss_rate: 0.0,
            jitter_us: 0,
            is_hole_punched: true,
        };
        let fused = ConnMetrics {
            conn_id: Uuid::from_bytes([3; 16]),
            latency_us: 5_000,
            loss_rate: 0.35,
            jitter_us: 0,
            is_hole_punched: false,
        };

        assert!(
            !may_tear_down_for_ping_failure(std::iter::empty(), cfg),
            "last path must not be torn down"
        );
        assert!(
            !may_tear_down_for_ping_failure([&unverified_punch], cfg),
            "unverified hole-punch is not a real alternative"
        );
        assert!(
            !may_tear_down_for_ping_failure([&fused], cfg),
            "fused path is not a real alternative"
        );
        assert!(
            may_tear_down_for_ping_failure([&healthy], cfg),
            "verified unfused path allows teardown"
        );
        assert!(
            may_tear_down_for_ping_failure([&unverified_punch, &healthy], cfg),
            "one good alternative among junk is enough"
        );
    }

    #[test]
    fn fast_probe_timeout_scales_with_rtt_and_is_clamped() {
        // No sample yet (fresh conn): a conservative fixed budget, NOT the floor.
        // A relayed or long-haul path has a first-probe RTT far above 300ms, and
        // timing it with the floor published a healthy new link as 100% loss.
        assert_eq!(fast_probe_timeout(None), FAST_PROBE_TIMEOUT_UNMEASURED);
        assert!(FAST_PROBE_TIMEOUT_UNMEASURED > FAST_PROBE_TIMEOUT_MIN);
        // Measured at ~0µs (a real sample from a LAN): the floor is correct now.
        assert_eq!(fast_probe_timeout(Some(0)), FAST_PROBE_TIMEOUT_MIN);
        // Sub-millisecond link: still the floor.
        assert_eq!(fast_probe_timeout(Some(500)), FAST_PROBE_TIMEOUT_MIN);
        // 200ms RTT -> 3x, above the floor and below the cap.
        assert_eq!(
            fast_probe_timeout(Some(200_000)),
            Duration::from_millis(600)
        );
        // Very high RTT: capped, because a probe cannot conclude faster than the
        // RTT allows.
        assert_eq!(fast_probe_timeout(Some(2_000_000)), FAST_PROBE_TIMEOUT_MAX);

        // Detection budget for the common case (an established, measured link)
        // stays inside the Phase F target.
        let worst_miss = FAST_PROBE_INTERVAL + fast_probe_timeout(Some(0));
        assert!(
            worst_miss * FAST_PROBE_MISSES_TO_DEGRADE < Duration::from_secs(2),
            "3 misses should land under 2s: {worst_miss:?} each"
        );
    }

    /// Going degraded must be as fast as before; coming back must need proof, so a
    /// lossy-but-usable link cannot flip the published loss rate (and therefore
    /// our edge cost) on nearly every probe.
    #[test]
    fn recovery_from_degraded_needs_a_run_of_successes() {
        // Unchanged detection: three consecutive misses, from a healthy state.
        assert!(!fast_probe_publish_degraded(0, 99, false));
        assert!(!fast_probe_publish_degraded(2, 0, false));
        assert!(fast_probe_publish_degraded(3, 0, false));

        // Recovery needs FAST_PROBE_RECOVER_ROUNDS consecutive successes.
        for hits in 0..FAST_PROBE_RECOVER_ROUNDS {
            assert!(
                fast_probe_publish_degraded(0, hits, true),
                "{hits} successes must not declare the link healthy again"
            );
        }
        assert!(!fast_probe_publish_degraded(
            0,
            FAST_PROBE_RECOVER_ROUNDS,
            true
        ));

        // The guarantee that matters at ~3 probes/s: while successes never run to
        // FAST_PROBE_RECOVER_ROUNDS, a degraded link stays degraded. Bursty loss
        // (miss, miss, ok, miss, miss, ok …) used to clear it on the first ok and
        // re-degrade three probes later, forever.
        for hits in 0..FAST_PROBE_RECOVER_ROUNDS {
            for misses in 0..FAST_PROBE_MISSES_TO_DEGRADE {
                assert!(
                    fast_probe_publish_degraded(misses, hits, true),
                    "{misses} misses / {hits} hits must not clear a degraded link"
                );
            }
        }
    }

    /// A link whose keepalive keeps answering but whose probe keeps timing out
    /// must still be able to recover.
    ///
    /// The probe budget is `3 × EWMA RTT` (clamped), the keepalive's is a flat
    /// `PING_WAIT_TIMEOUT`, so sustained jitter above that multiple leaves the
    /// probe failing every round while the pong arrives every round. The probe
    /// miss must not zero the hit that round earned: with that bug the count
    /// could never exceed 1, and the link stayed published at 100% loss for as
    /// long as the jitter lasted.
    #[test]
    fn keepalive_proof_alone_clears_a_published_degradation() {
        let timeout: Result<PingResponse, Error> = Err(Error::WaitRespError(
            "wait ping response timeout".to_owned(),
        ));
        let mut misses = 0;
        let mut hits = 0;
        let mut seen_epoch = 10;
        let mut published = true;

        for round in 1..=FAST_PROBE_RECOVER_ROUNDS {
            // The probe times out, but the keepalive proved a round trip during
            // the same round.
            let epoch = seen_epoch + 1;
            fast_probe_apply_round_evidence(
                &timeout,
                epoch,
                &mut seen_epoch,
                delivery(false, 300),
                &mut DeliveryExcuse::default(),
                &mut misses,
                &mut hits,
            );

            assert_eq!(hits, round, "each proven round must earn exactly one hit");
            assert_eq!(misses, 0, "a proven round ends the miss run");

            published = fast_probe_publish_degraded(misses, hits, published);
            if round < FAST_PROBE_RECOVER_ROUNDS {
                assert!(
                    published,
                    "{round} rounds of keepalive proof must not clear it yet"
                );
            } else {
                assert!(
                    !published,
                    "a full run of keepalive proof must clear it, not latch forever"
                );
            }
        }
    }

    /// A round in which the probe *and* the keepalive both report a round trip is
    /// still **one** round of proof.
    ///
    /// Counting both would clear a degradation in two rounds instead of
    /// `FAST_PROBE_RECOVER_ROUNDS`, quietly weakening the anti-flap gate.
    #[test]
    fn a_round_counts_once_even_when_both_paths_prove_it() {
        let pong: Result<PingResponse, Error> = Ok(PingResponse::Pong(1_000));
        let mut misses = 0;
        let mut hits = 0;
        let mut seen_epoch = 41;

        for round in 1..=FAST_PROBE_RECOVER_ROUNDS {
            let epoch = seen_epoch + 1;
            fast_probe_apply_round_evidence(
                &pong,
                epoch,
                &mut seen_epoch,
                delivery(false, 300),
                &mut DeliveryExcuse::default(),
                &mut misses,
                &mut hits,
            );
            assert_eq!(hits, round, "one round must be worth one hit, not two");
            assert_eq!(misses, 0);
            assert!(fast_probe_publish_degraded(misses, hits, true) == (round < 3));
        }
    }

    /// Nothing proven is still a black hole: a still epoch must not delay
    /// detection, and a miss with no keepalive round trip must reset the hit run.
    #[test]
    fn nothing_proven_reaches_degraded_and_earns_no_hit() {
        let timeout: Result<PingResponse, Error> = Err(Error::WaitRespError(
            "wait ping response timeout".to_owned(),
        ));
        let mut misses = 0;
        let mut hits = FAST_PROBE_RECOVER_ROUNDS;
        let mut seen_epoch = 5;
        let mut published = false;

        for round in 1..=FAST_PROBE_MISSES_TO_DEGRADE {
            fast_probe_apply_round_evidence(
                &timeout,
                seen_epoch,
                &mut seen_epoch,
                delivery(false, 300),
                &mut DeliveryExcuse::default(),
                &mut misses,
                &mut hits,
            );
            assert_eq!(misses, round);
            assert_eq!(
                hits, 0,
                "a miss with no proven round trip must reset the hit run"
            );
            published = fast_probe_publish_degraded(misses, hits, published);
        }
        assert!(
            published,
            "three unproven misses must publish the degradation"
        );
        // Detection must not jump the local-routing gate, which needs a longer run.
        assert!(!fast_probe_is_sustained(misses));
    }

    /// A conn that is *delivering* bytes has not gone dark, even when every probe
    /// times out.
    ///
    /// Regression for the 2026-11-08 load test: a saturated uplink pushed delivery
    /// delay past the probe's `3 × EWMA RTT` budget, and because a timeout yields
    /// no RTT sample the EWMA never learned about the queue — the budget stayed
    /// pinned at its 300ms floor while the real delay was 663ms. Three misses
    /// published 100% loss, `loss_rate()` reported 1.0, and the edge was priced
    /// out of the local SPF for 4s… while **103 of 103** application requests on
    /// that same conn succeeded.
    ///
    /// The keepalive cannot supply the missing evidence either: its interval backs
    /// off to 32s when idle, so in the ~2s the probe needs to decide it is often
    /// the probe's own word alone. The byte counter is what is left.
    #[test]
    fn a_conn_delivering_bytes_is_never_published_as_lost() {
        let timeout: Result<PingResponse, Error> = Err(Error::WaitRespError(
            "wait ping response timeout".to_owned(),
        ));
        let mut misses = 0;
        let mut hits = 0;
        let mut seen_epoch = 3;

        for round in 1..=30 {
            // Unchanged epoch: no keepalive pong landed in this round.
            let outcome = fast_probe_apply_round_evidence(
                &timeout,
                seen_epoch,
                &mut seen_epoch,
                delivery(true, 300),
                &mut DeliveryExcuse::default(),
                &mut misses,
                &mut hits,
            );
            assert_eq!(outcome, RoundOutcome::DeliveryOnly);
            assert_eq!(
                misses, 0,
                "round {round}: a byte arriving ends the miss run"
            );
            assert!(
                !fast_probe_publish_degraded(misses, hits, false),
                "round {round}: a delivering conn must never be published as 100% loss"
            );
        }

        // The moment the bytes stop, the very same probe outcome is a real miss
        // again — the exemption is per round, not a latch.
        fast_probe_apply_round_evidence(
            &timeout,
            seen_epoch,
            &mut seen_epoch,
            delivery(false, 300),
            &mut DeliveryExcuse::default(),
            &mut misses,
            &mut hits,
        );
        assert_eq!(misses, 1, "a silent conn must still be able to miss");
    }

    /// A delivering round is not proof: on its own it must never clear a
    /// published degradation.
    ///
    /// This is the counterpart to
    /// `a_conn_delivering_bytes_is_never_published_as_lost`. Inbound bytes are
    /// enough to stop the probe *accusing* the path, but they only ever prove the
    /// peer→us direction. Letting them clear the flag is how a one-way outage —
    /// the far side still sending, nothing we send getting through — keeps its
    /// edge priced as healthy forever while every packet we originate is lost.
    #[test]
    fn delivery_alone_never_clears_a_published_degradation() {
        let timeout: Result<PingResponse, Error> = Err(Error::WaitRespError(
            "wait ping response timeout".to_owned(),
        ));
        let mut misses = FAST_PROBE_MISSES_TO_DEGRADE;
        let mut hits = 0;
        let mut seen_epoch = 0;
        let mut published = true;

        for round in 1..=FAST_PROBE_RECOVER_ROUNDS * 2 {
            let outcome = fast_probe_apply_round_evidence(
                &timeout,
                seen_epoch,
                &mut seen_epoch,
                delivery(true, 300),
                &mut DeliveryExcuse::default(),
                &mut misses,
                &mut hits,
            );
            assert_eq!(outcome, RoundOutcome::DeliveryOnly);
            assert_eq!(misses, 0, "round {round}: delivery still ends the miss run");
            assert_eq!(hits, 0, "round {round}: delivery is not proof");
            published = fast_probe_publish_degraded(misses, hits, published);
            assert!(
                published,
                "round {round}: only a round trip may clear the degradation"
            );
        }

        // A proven round trip is what actually turns it around.
        let pong: Result<PingResponse, Error> = Ok(PingResponse::Pong(1_000));
        for round in 1..=FAST_PROBE_RECOVER_ROUNDS {
            let epoch = seen_epoch + 1;
            fast_probe_apply_round_evidence(
                &pong,
                epoch,
                &mut seen_epoch,
                delivery(false, 300),
                &mut DeliveryExcuse::default(),
                &mut misses,
                &mut hits,
            );
            published = fast_probe_publish_degraded(misses, hits, published);
            if round < FAST_PROBE_RECOVER_ROUNDS {
                assert!(published, "round {round}: still not cleared yet");
            }
        }
        assert!(!published, "three proven rounds must clear it");
    }

    /// The delivery excuse is rationed by the probe budget.
    ///
    /// A short budget may be outrun by a queue, so a timeout there is ambiguous.
    /// A budget already at [`FAST_PROBE_EXCUSE_MAX_BUDGET`] or wider cannot be:
    /// if the peer still cannot answer one round trip in that window while only
    /// pushing bytes at us, the path is one-way dead and the edge must stop
    /// carrying our traffic.
    ///
    /// The second half is the part that is easy to get wrong: the excuse is
    /// spent by *reaching* the ceiling, not by staying above it. The floor falls
    /// by half after a miss, so a budget-only test would hand the excuse back on
    /// the very next round — see
    /// `a_one_way_outage_is_published_even_though_the_floor_falls_back`, which
    /// exercises that composition.
    #[test]
    fn inbound_delivery_stops_excusing_a_timeout_once_the_budget_is_wide() {
        let timeout: Result<PingResponse, Error> = Err(Error::WaitRespError(
            "wait ping response timeout".to_owned(),
        ));

        let mut misses = 0;
        let mut hits = 0;
        let mut seen_epoch = 0;
        let mut excuse = DeliveryExcuse::default();
        let outcome = fast_probe_apply_round_evidence(
            &timeout,
            seen_epoch,
            &mut seen_epoch,
            delivery(true, FAST_PROBE_TIMEOUT_MIN.as_millis() as u64),
            &mut excuse,
            &mut misses,
            &mut hits,
        );
        assert_eq!(
            outcome,
            RoundOutcome::DeliveryOnly,
            "a queue can outrun the 300ms floor, so this timeout is ambiguous"
        );
        assert_eq!(misses, 0);
        assert!(
            !excuse.spent,
            "a short budget must not spend the episode's excuse"
        );

        // Same evidence, budget at the excuse ceiling: no longer ambiguous.
        let mut misses = 0;
        let mut hits = FAST_PROBE_RECOVER_ROUNDS;
        let outcome = fast_probe_apply_round_evidence(
            &timeout,
            seen_epoch,
            &mut seen_epoch,
            delivery(true, FAST_PROBE_EXCUSE_MAX_BUDGET.as_millis() as u64),
            &mut excuse,
            &mut misses,
            &mut hits,
        );
        assert_eq!(
            outcome,
            RoundOutcome::Miss,
            "one-way dead: receiving, but never answering a 1.2s round trip"
        );
        assert_eq!(misses, 1);
        assert_eq!(hits, 0);
        assert!(excuse.spent, "reaching the ceiling spends the excuse");

        // And it stays spent at a budget that would have been excused before the
        // floor fell back: otherwise the round after every miss is excused
        // again, which clears the miss run and hides the outage.
        let outcome = fast_probe_apply_round_evidence(
            &timeout,
            seen_epoch,
            &mut seen_epoch,
            delivery(true, FAST_PROBE_TIMEOUT_MIN.as_millis() as u64),
            &mut excuse,
            &mut misses,
            &mut hits,
        );
        assert_eq!(
            outcome,
            RoundOutcome::Miss,
            "a spent excuse cannot be bought back by a cheaper budget"
        );
        assert_eq!(misses, 2);
    }

    /// The incident, end to end: a busily queued conn must never publish a loss.
    ///
    /// 4-way download over a 6ms LAN conn queued ICMP to a 663ms p99 while the
    /// EWMA RTT was 6ms, so `3 × RTT` clamped to the 300ms floor and every
    /// probe timed out — yet 103 of 103 application requests succeeded. The
    /// floor has to widen enough to cover the queue *before* the third miss.
    #[test]
    fn a_queued_but_delivering_conn_never_publishes_a_loss() {
        let queue = Duration::from_millis(663);
        let mut sim = ProbeSim::cold();

        let mut proved_at = None;
        for round in 1..=30 {
            // Bytes keep arriving: the peer's own probe pings come in every
            // round, and the application traffic is flowing.
            let outcome = sim.round(true, Some(queue));
            if outcome == RoundOutcome::Proven && proved_at.is_none() {
                proved_at = Some(round);
            }
            assert!(
                !sim.degraded,
                "round {round}: a conn delivering bytes must never be published as 100% loss"
            );
            assert!(
                !fast_probe_is_sustained(sim.misses),
                "round {round}: and must never reach the sustained gate either"
            );
        }

        // The widening is what does it — being excused forever would leave the
        // next real outage invisible, so the probe has to start *answering*
        // again. It must do so inside the miss window: a queue of 663ms is
        // covered by the second doubling, so round 3 at the latest.
        assert_eq!(
            proved_at,
            Some(FAST_PROBE_MISSES_TO_DEGRADE),
            "the widening must let the probe cover a 663ms queue by round 3"
        );
        assert!(
            sim.max_budget >= queue,
            "some round must actually have waited long enough for the queue, widest was {:?}",
            sim.max_budget
        );
    }

    /// A round whose ping never left the box must not spend the episode's
    /// delivery excuse.
    ///
    /// `Inconclusive` is the verdict for local congestion: our own egress ring
    /// was full, so the round says nothing about the path. Letting it spend the
    /// widening — which the *path* may well have earned — would make the next
    /// three timeouts accuse the peer for our own queue, i.e. publish 100% loss
    /// on a link that is only busy on this side.
    #[test]
    fn inconclusive_rounds_neither_spend_nor_re_arm_the_excuse() {
        let congestion: Result<PingResponse, Error> = Ok(PingResponse::LocalCongestion);
        let mut misses = 0;
        let mut hits = 0;
        let mut seen_epoch = 0;
        let mut excuse = DeliveryExcuse::default();

        // Wide budget, but the probe never left: still nothing spent.
        let outcome = fast_probe_apply_round_evidence(
            &congestion,
            seen_epoch,
            &mut seen_epoch,
            delivery(false, FAST_PROBE_EXCUSE_MAX_BUDGET.as_millis() as u64),
            &mut excuse,
            &mut misses,
            &mut hits,
        );
        assert_eq!(outcome, RoundOutcome::Inconclusive);
        assert!(!excuse.spent, "local congestion must not spend the excuse");

        // A spent excuse is not refunded by local congestion either: the ping
        // still never left the box, so it is not a round trip.
        excuse.spent = true;
        let outcome = fast_probe_apply_round_evidence(
            &congestion,
            seen_epoch,
            &mut seen_epoch,
            delivery(false, FAST_PROBE_TIMEOUT_MIN.as_millis() as u64),
            &mut excuse,
            &mut misses,
            &mut hits,
        );
        assert_eq!(outcome, RoundOutcome::Inconclusive);
        assert!(excuse.spent, "and must not refund one either");
        assert_eq!((misses, hits), (0, 0), "it still moves no counter");
    }

    /// A one-way outage must still be published — *after* the floor has fallen
    /// back, which is when the excuse has to stay spent.
    ///
    /// This is the regression the decay introduces. `DeliveryOnly` clears the
    /// miss run (it must: that is what keeps
    /// `a_conn_delivering_bytes_is_never_published_as_lost` true), and a miss
    /// halves the floor. Compare the budget against the ceiling per round and the
    /// sequence becomes `DeliveryOnly` (300ms) → `DeliveryOnly` (600ms) → `Miss`
    /// (1200ms) → `DeliveryOnly` (600ms) → `Miss` (1200ms) → …: every miss is
    /// followed by an excused round that zeroes the run, so `misses` never
    /// reaches three, nothing is ever published, and a conn that can only
    /// receive keeps its edge while every packet we originate is dropped.
    #[test]
    fn a_one_way_outage_is_published_even_though_the_floor_falls_back() {
        let mut sim = ProbeSim::cold();

        let mut published_at = None;
        for round in 1..=6 {
            // The far side keeps sending; our probe never gets an answer.
            let outcome = sim.round(true, None);
            if sim.degraded && published_at.is_none() {
                published_at = Some((round, outcome, sim.elapsed));
            }
        }

        let (round, outcome, took) = published_at.expect(
            "a one-way outage must be published within the documented window, \
             not excused forever",
        );
        assert_eq!(
            outcome,
            RoundOutcome::Miss,
            "the publishing round is the one that stopped excusing"
        );
        assert!(round <= 5, "expected publication by round 5, took {round}");

        // And it stays published while the far side keeps sending.
        for round in 7..=12 {
            sim.round(true, None);
            assert!(
                sim.degraded,
                "round {round}: the outage must not clear itself"
            );
        }

        // The cost of the widening is bounded: two excused rounds plus the
        // ceiling round, then the floor is back at the minimum.
        assert_eq!(
            sim.floor, FAST_PROBE_TIMEOUT_MIN,
            "the floor must be back at the minimum once the misses have been counted"
        );
        assert!(
            took <= Duration::from_millis(6_000),
            "the one-way path must not be able to stall detection, took {took:?}"
        );
    }

    /// The other half of the same coin: a black hole still starts at the floor
    /// and is published at the documented ~1.9s.
    ///
    /// Kept next to the one-way test because they differ only in whether bytes
    /// arrive, and the whole design turns on that distinction.
    #[test]
    fn a_black_hole_is_published_at_the_cold_start_speed() {
        let mut sim = ProbeSim::cold();
        let mut published_at = None;

        for round in 1..=3 {
            let outcome = sim.round(false, None);
            assert_eq!(outcome, RoundOutcome::Miss);
            if sim.degraded && published_at.is_none() {
                published_at = Some(round);
            }
        }

        assert_eq!(
            published_at,
            Some(FAST_PROBE_MISSES_TO_DEGRADE),
            "a silent conn is published on the third miss"
        );
        assert_eq!(
            sim.floor, FAST_PROBE_TIMEOUT_MIN,
            "a miss must not widen the floor"
        );
        // 3 × (333ms interval + 300ms budget) ≈ 1.9s.
        assert!(
            sim.elapsed <= Duration::from_millis(1_900),
            "black-hole detection must stay at the documented ~1.9s, took {:?}",
            sim.elapsed
        );
    }

    /// The budget floor has to actually move, or `3 × EWMA RTT` stays pinned at
    /// 300ms for the whole incident and the excuse above is all that is left.
    #[test]
    fn budget_floor_climbs_on_excused_timeouts_and_falls_on_proof() {
        let floor = FAST_PROBE_TIMEOUT_MIN;
        let excused = RoundOutcome::DeliveryOnly;
        let proven = RoundOutcome::Proven;

        let one = fast_probe_next_budget_floor(floor, excused);
        let two = fast_probe_next_budget_floor(one, excused);
        assert_eq!(one, floor * 2);
        assert_eq!(two, floor * 4);
        // 300ms -> 600ms is already inside the three-miss window, which is why a
        // merely congested conn never publishes at all.
        assert!(one < FAST_PROBE_EXCUSE_MAX_BUDGET);

        // Capped, never unbounded.
        let mut cur = floor;
        for _ in 0..8 {
            cur = fast_probe_next_budget_floor(cur, excused);
        }
        assert_eq!(cur, FAST_PROBE_TIMEOUT_MAX);

        // And it comes back down, so a recovered conn is not stuck at the ceiling.
        let back = fast_probe_next_budget_floor(cur, proven);
        assert_eq!(back, FAST_PROBE_TIMEOUT_MAX / 2);
        let mut cur = back;
        for _ in 0..8 {
            cur = fast_probe_next_budget_floor(cur, proven);
        }
        assert_eq!(cur, floor, "never below the floor");

        // A miss retracts the widening one step, for the same reason a proven
        // round does: the queue it was bought for is not there. It must **not**
        // go back under `FAST_PROBE_EXCUSE_MAX_BUDGET` and re-arm the excuse,
        // which is what `DeliveryExcuse` exists to prevent — see
        // `a_one_way_outage_is_published_even_though_the_floor_falls_back`.
        assert_eq!(
            fast_probe_next_budget_floor(two, RoundOutcome::Miss),
            floor * 2
        );
        // Inconclusive is evidence of nothing, so it may neither widen nor
        // retract: local congestion is not the path's fault.
        assert_eq!(
            fast_probe_next_budget_floor(two, RoundOutcome::Inconclusive),
            two
        );
    }

    /// A liveness echo proves a round trip just like a pong, and must still be
    /// worth exactly one hit.
    ///
    /// Both used to be counted twice — once by the `Ok(_)` arm and again by the
    /// proven-round arm, which treated the echo as "not a pong" — so recovery
    /// from a degraded state could finish in two rounds instead of
    /// [`FAST_PROBE_RECOVER_ROUNDS`] and quietly weaken the anti-flap gate.
    #[test]
    fn a_liveness_echo_is_one_hit_not_two() {
        let echo: Result<PingResponse, Error> = Ok(PingResponse::LivenessEcho);
        let mut misses = 0;
        let mut hits = 0;
        let mut seen_epoch = 12;

        for round in 1..=FAST_PROBE_RECOVER_ROUNDS {
            let epoch = seen_epoch + 1;
            fast_probe_apply_round_evidence(
                &echo,
                epoch,
                &mut seen_epoch,
                delivery(false, 300),
                &mut DeliveryExcuse::default(),
                &mut misses,
                &mut hits,
            );
            assert_eq!(hits, round, "one proven round is worth one hit");
            assert_eq!(misses, 0);
        }
    }

    /// Local congestion is evidence of nothing: it must not count as a miss, and
    /// a keepalive round proven in the same round still earns its hit.
    #[test]
    fn local_congestion_is_neutral_and_still_credits_a_proven_round() {
        let congested: Result<PingResponse, Error> = Ok(PingResponse::LocalCongestion);
        let mut misses = 2;
        let mut hits = 0;
        let mut seen_epoch = 7;

        fast_probe_apply_round_evidence(
            &congested,
            7,
            &mut seen_epoch,
            delivery(false, 300),
            &mut DeliveryExcuse::default(),
            &mut misses,
            &mut hits,
        );
        assert_eq!(
            misses, 2,
            "a ping that never left the box is not a path miss"
        );
        assert_eq!(hits, 0);

        fast_probe_apply_round_evidence(
            &congested,
            8,
            &mut seen_epoch,
            delivery(false, 300),
            &mut DeliveryExcuse::default(),
            &mut misses,
            &mut hits,
        );
        assert_eq!(misses, 0, "the proven round ends the miss run");
        assert_eq!(hits, 1, "the proven round earns its hit");
    }

    /// The gate that lets degradation re-price our **own** outgoing edge must be
    /// strictly later than the gate that publishes 100% loss, and must not be
    /// reachable by a couple of unlucky probes.
    #[test]
    fn only_a_sustained_run_of_misses_gates_local_routing() {
        // Read through locals so this reads as an invariant about the rule rather
        // than a compile-time tautology.
        let (to_degrade, to_gate_local) =
            (FAST_PROBE_MISSES_TO_DEGRADE, FAST_PROBE_SUSTAINED_ROUNDS);
        assert!(
            to_gate_local > to_degrade,
            "the local override must need more evidence than the metric"
        );
        for misses in 0..to_degrade {
            assert!(!fast_probe_is_sustained(misses));
        }
        // Degraded for the metric, still not enough to move traffic.
        for misses in to_degrade..to_gate_local {
            assert!(
                !fast_probe_is_sustained(misses),
                "{misses} misses must not re-price the local edge"
            );
        }
        assert!(fast_probe_is_sustained(to_gate_local));
        assert!(fast_probe_is_sustained(to_gate_local + 100));

        // Every missed round also burns its timeout budget, so the real cost of
        // the gate is `rounds x (interval + timeout)`, not `rounds x interval`.
        // Assert the real figure so a future retune cannot quietly make the local
        // override slower than the keepalive teardown it exists to avoid waiting
        // for (~15s).
        //
        // Two budgets, because the gate can fire in both states. `Some(0)` is a
        // measured link clamped to the floor — the steady state. `None` is the
        // fresh conn, which is precisely when this gate is reachable: no RTT
        // sample means no successful round trip yet, so every one of the nine
        // rounds probes with the 1s unmeasured budget rather than the floor.
        let measured_round = FAST_PROBE_INTERVAL + fast_probe_timeout(Some(0));
        let gate_cost = measured_round * to_gate_local;
        assert!(
            gate_cost < Duration::from_secs(10),
            "sustained gate takes {gate_cost:?}, which no longer beats keepalive teardown"
        );
        assert!(
            gate_cost > Duration::from_secs(3),
            "gate got cheaper than documented ({gate_cost:?})"
        );
        let unmeasured_round = FAST_PROBE_INTERVAL + fast_probe_timeout(None);
        let fresh_conn_gate = unmeasured_round * to_gate_local;
        assert!(
            fresh_conn_gate < Duration::from_secs(15),
            "sustained gate takes {fresh_conn_gate:?} before any RTT sample, which no \
             longer beats the keepalive teardown it races"
        );
    }

    /// The two probe loops share a broadcast channel and match Pongs by sequence
    /// equality, so their sequence spaces must never intersect.
    #[test]
    fn keepalive_and_fast_probe_sequences_never_collide() {
        let keepalive: Vec<u32> = (0..4).map(keepalive_seq).collect();
        let fast: Vec<u32> = (0..4).map(fast_probe_seq).collect();

        assert_eq!(keepalive, vec![0, 1, 2, 3]);
        assert_eq!(
            fast,
            vec![
                FAST_PROBE_SEQ_BIT,
                FAST_PROBE_SEQ_BIT + 1,
                FAST_PROBE_SEQ_BIT + 2,
                FAST_PROBE_SEQ_BIT + 3
            ]
        );
        for k in &keepalive {
            assert!(fast.iter().all(|f| f != k), "sequence spaces overlapped");
        }

        // The mask wraps the keepalive well before it could reach the fast
        // probe's range, and the fast probe wraps within its own half.
        assert_eq!(keepalive_seq(SEQ_MASK + 1), 0);
        assert_eq!(fast_probe_seq(SEQ_MASK + 1), FAST_PROBE_SEQ_BIT);
    }
}
