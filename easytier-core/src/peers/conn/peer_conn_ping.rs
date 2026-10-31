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
/// probe got no answer within its own budget.
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
/// A round of proof is one probe round in which the path was shown to carry a
/// packet: the probe's own round trip **or** a keepalive round trip that landed
/// during that round. When both happen they still count once — this constant is
/// about rounds, not samples.
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

/// Folds one fast-probe round — the probe's own outcome plus any keepalive proof
/// that landed during it — into the miss/hit window.
///
/// This is the only place either counter moves, and the only way keepalive
/// evidence may affect the published flag: writing `false` into
/// `fast_probe_degraded` directly is what used to bypass
/// [`FAST_PROBE_RECOVER_ROUNDS`] and make the edge cost flip on every keepalive.
///
/// * The probe's outcome moves the counters the obvious way: a round trip is a
///   hit, a timeout (or a failed send) is a miss, local congestion is neither —
///   the ping never left the box, so it is evidence of nothing.
/// * A keepalive round trip that happened during this round outranks a probe
///   miss: the probe only proved "no answer within 3×RTT" while the pong proved
///   the path carries packets *now*, so the miss run ends here instead of
///   counting toward [`FAST_PROBE_MISSES_TO_DEGRADE`]. A real black hole
///   produces no pong either, so the probe still reaches three misses on its own.
/// * That keepalive hit is **not** wiped by the same round's probe miss, and it
///   is **not** added on top of the probe's own hit. The first matters because
///   the probe budget is `3 × EWMA RTT`: on a link whose jitter keeps putting
///   the real RTT above that (while the keepalive's fixed 2s budget still
///   answers every second) the probe can miss every single round, and if each
///   miss zeroed `consecutive_hits` the count could never climb past 1 — a link
///   proven alive every second would stay published as 100% loss forever. The
///   second matters because [`FAST_PROBE_RECOVER_ROUNDS`] counts *rounds*: two
///   hits per round would shorten recovery to two rounds and quietly weaken the
///   anti-flap gate this whole mechanism exists for.
fn fast_probe_apply_round_evidence(
    probe: &Result<PingResponse, Error>,
    roundtrip_epoch: u32,
    seen_epoch: &mut u32,
    consecutive_misses: &mut u32,
    consecutive_hits: &mut u32,
) {
    // The epoch is bumped on the keepalive path for every proven round trip. An
    // unchanged value means nothing was proven since the previous round (or the
    // epoch wrapped all the way back, in which case treating it as "nothing new"
    // only delays recovery by one round).
    let roundtrip_proven = *seen_epoch != roundtrip_epoch;
    if roundtrip_proven {
        *seen_epoch = roundtrip_epoch;
    }

    let probe_round_tripped = matches!(probe, Ok(PingResponse::Pong(_)));
    match probe {
        Ok(PingResponse::LocalCongestion) => {}
        Ok(_) => {
            *consecutive_misses = 0;
            *consecutive_hits = consecutive_hits.saturating_add(1);
        }
        Err(_) => {
            *consecutive_misses = consecutive_misses.saturating_add(1);
            if !roundtrip_proven {
                *consecutive_hits = 0;
            }
        }
    }

    if roundtrip_proven {
        *consecutive_misses = 0;
        // One round is one unit of evidence, whichever side reported it.
        if !probe_round_tripped {
            *consecutive_hits = consecutive_hits.saturating_add(1);
        }
    }
}

/// Per-probe wait budget derived from the measured RTT, clamped to this range.
const FAST_PROBE_TIMEOUT_MIN: Duration = Duration::from_millis(300);
const FAST_PROBE_TIMEOUT_MAX: Duration = Duration::from_secs(2);
const FAST_PROBE_TIMEOUT_RTT_MULT: u32 = 3;

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
        // try_send so a full data ring does not stall keepalive behind HOL, and
        // so local congestion is not scored as a path miss.
        match self.sink.try_send(req) {
            Ok(()) => {}
            Err(TunnelError::BufferFull) => return Ok(PingResponse::LocalCongestion),
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
                continue;
            }

            let mut receiver = self.ctrl_sender.subscribe();
            // `None` until the conn has produced its first sample: an unmeasured
            // link must not be timed with a budget shorter than its own RTT.
            let rtt_us = (self.latency_stats.sample_count() > 0)
                .then(|| self.latency_stats.get_latency_us::<u32>());
            // No liveness token: the echo slot is single-slot and shared with the
            // keepalive, and a plain Ping/Pong answers the same question.
            let probe = self
                .do_pingpong_once(
                    &mut receiver,
                    fast_probe_seq(seq),
                    None,
                    fast_probe_timeout(rtt_us),
                )
                .await;
            seq = seq.wrapping_add(1);

            // One round of evidence: the probe's own outcome plus any keepalive
            // proof that landed while it was in flight. Folded through the same
            // gate rather than written straight into the flag — without this, one
            // pong cleared a published degradation outright and the probe
            // re-published it three rounds later, the flip-flop that kept
            // re-pricing the edge. See `fast_probe_apply_round_evidence` for why
            // a pong outranks this round's probe miss and why both together still
            // count as a single hit.
            fast_probe_apply_round_evidence(
                &probe,
                self.roundtrip_epoch.load(Ordering::Relaxed),
                &mut seen_roundtrip_epoch,
                &mut misses,
                &mut hits,
            );

            let degraded = fast_probe_publish_degraded(misses, hits, was_degraded);
            self.fast_probe_degraded.store(degraded, Ordering::Relaxed);
            // `misses` already restarts at 0 on any success, so the sustained
            // flag can only be reached by one unbroken run of misses, and is
            // dropped by the same evidence that drops `degraded`.
            let sustained = fast_probe_is_sustained(misses);
            self.fast_probe_sustained
                .store(sustained, Ordering::Relaxed);

            // Log the transitions only: at ~3 probes/s a per-probe log would
            // flood exactly during the incident it is meant to explain.
            if degraded && !was_degraded {
                tracing::warn!(
                    peer_id = self.peer_id,
                    misses,
                    ?rtt_us,
                    "fast liveness probe: link not round-tripping, publishing degraded loss"
                );
            } else if !degraded && was_degraded {
                tracing::info!(
                    peer_id = self.peer_id,
                    ?rtt_us,
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
            fast_probe_apply_round_evidence(&pong, epoch, &mut seen_epoch, &mut misses, &mut hits);
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

    /// Local congestion is evidence of nothing: it must not count as a miss, and
    /// a keepalive round proven in the same round still earns its hit.
    #[test]
    fn local_congestion_is_neutral_and_still_credits_a_proven_round() {
        let congested: Result<PingResponse, Error> = Ok(PingResponse::LocalCongestion);
        let mut misses = 2;
        let mut hits = 0;
        let mut seen_epoch = 7;

        fast_probe_apply_round_evidence(&congested, 7, &mut seen_epoch, &mut misses, &mut hits);
        assert_eq!(
            misses, 2,
            "a ping that never left the box is not a path miss"
        );
        assert_eq!(hits, 0);

        fast_probe_apply_round_evidence(&congested, 8, &mut seen_epoch, &mut misses, &mut hits);
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
