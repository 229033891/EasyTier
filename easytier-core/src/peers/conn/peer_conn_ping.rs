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
        conn::{peer_conn::PeerConnCloseNotify, peer_conn_liveness::PeerConnLiveness},
        context::ArcPeerContext,
        error::Error,
    },
    tunnel::{
        mpsc::MpscTunnelSender,
        stats::{Throughput, WindowLatency},
    },
};

/// Wait budget for one ordinary keepalive ping.
const PING_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Fast liveness probe (roadmap `live-next-hop.md` Phase F — "BFD-style").
///
/// This is an *extra* short round trip whose only job is to publish the loss
/// rate early. It never closes the conn: teardown stays with the ordinary
/// keepalive and its `ping_fail_close_count`, so the deliberate flap tolerance
/// around a reconnect (which costs ~30s to rebuild) is unchanged.
const FAST_PROBE_INTERVAL: Duration = Duration::from_millis(333);
/// Consecutive misses before the conn is published as degraded (100% loss).
const FAST_PROBE_MISSES_TO_DEGRADE: u32 = 3;
/// Per-probe wait budget derived from the measured RTT, clamped to this range.
const FAST_PROBE_TIMEOUT_MIN: Duration = Duration::from_millis(300);
const FAST_PROBE_TIMEOUT_MAX: Duration = Duration::from_secs(2);
const FAST_PROBE_TIMEOUT_RTT_MULT: u32 = 3;

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
/// The floor covers a fresh conn with no sample yet (see `get_latency_us`
/// returning 0), so an unmeasured link is not declared dead on its first probe.
/// The cap exists because a probe cannot conclude faster than the RTT allows;
/// on a very high-RTT link detection is slower than the typical case by design.
fn fast_probe_timeout(rtt_us: u32) -> Duration {
    let scaled = Duration::from_micros(u64::from(rtt_us) * u64::from(FAST_PROBE_TIMEOUT_RTT_MULT));
    scaled.clamp(FAST_PROBE_TIMEOUT_MIN, FAST_PROBE_TIMEOUT_MAX)
}

#[derive(Debug)]
enum PingResponse {
    Pong(u128),
    LivenessEcho,
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
    close_notify: Arc<PeerConnCloseNotify>,
    max_interval: Duration,
    /// Consecutive ping failures before closing; default 5.
    fail_close_count: u32,
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
        close_notify: Arc<PeerConnCloseNotify>,
        max_interval: Duration,
        fail_close_count: u32,
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
            close_notify,
            max_interval,
            fail_close_count: fail_close_count.max(1),
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
        self.sink.send(req).await?;
        self.context.record_control_tx(&self.network_name, req_len);

        let now = Instant::now();
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

            if let Ok(response) = &ret {
                if let PingResponse::Pong(lat) = response {
                    self.latency_stats.record_latency(*lat as u32);
                }
                if let Some(token) = liveness_token {
                    self.liveness.finish_probe(token);
                }
                loss_rate_stats_1.record_latency(0);
                loss_counter.store(0, Ordering::Relaxed);
                // Any successful round trip on this conn also clears the fast
                // probe's degraded reading: the flag must never outlive its
                // evidence, even if the probe task is gone (it only ever raises
                // the flag, and this is the independent second opinion).
                self.fast_probe_degraded.store(false, Ordering::Relaxed);
            } else {
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

            if loss_counter.load(Ordering::Relaxed) >= fail_close_count {
                tracing::warn!(
                    ?ret,
                    ?self,
                    ?loss_rate_1,
                    ?loss_counter,
                    fail_close_count,
                    "too many consecutive pingpong failures, closing the connection",
                );
                break;
            }

            self.loss_rate_stats
                .store((loss_rate_1 * 100.0) as u32, Ordering::Relaxed);
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
    pub async fn fast_probe(&mut self) {
        let mut seq = 0u32;
        let mut misses = 0u32;
        let mut was_degraded = false;
        loop {
            sleep(FAST_PROBE_INTERVAL).await;

            if self.close_notify.is_closed() {
                break;
            }
            if !self.liveness.is_enabled() {
                // Not negotiated (or not yet): stay out of the way and make sure
                // we never leave a stale degraded reading behind.
                self.fast_probe_degraded.store(false, Ordering::Relaxed);
                misses = 0;
                was_degraded = false;
                continue;
            }

            let mut receiver = self.ctrl_sender.subscribe();
            let rtt_us = self.latency_stats.get_latency_us::<u32>();
            // No liveness token: the echo slot is single-slot and shared with the
            // keepalive, and a plain Ping/Pong answers the same question.
            let ok = self
                .do_pingpong_once(
                    &mut receiver,
                    fast_probe_seq(seq),
                    None,
                    fast_probe_timeout(rtt_us),
                )
                .await
                .is_ok();
            seq = seq.wrapping_add(1);

            misses = if ok { 0 } else { misses + 1 };
            let degraded = misses >= FAST_PROBE_MISSES_TO_DEGRADE;
            self.fast_probe_degraded.store(degraded, Ordering::Relaxed);

            // Log the transitions only: at ~3 probes/s a per-probe log would
            // flood exactly during the incident it is meant to explain.
            if degraded && !was_degraded {
                tracing::warn!(
                    peer_id = self.peer_id,
                    misses,
                    rtt_us,
                    "fast liveness probe: link not round-tripping, publishing degraded loss"
                );
            } else if !degraded && was_degraded {
                tracing::info!(
                    peer_id = self.peer_id,
                    rtt_us,
                    "fast liveness probe: link round-trips again"
                );
            }
            was_degraded = degraded;
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
            Arc::new(PeerConnCloseNotify::new(PeerConnId::new_v4())),
            Duration::from_secs(32),
            5,
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
            Arc::new(PeerConnCloseNotify::new(PeerConnId::new_v4())),
            Duration::from_secs(32),
            5,
        );

        let result = timeout(Duration::from_secs(12), pinger.pingpong()).await;
        remote.abort();
        local_reader.abort();

        assert!(
            result.is_err(),
            "matching business-packet echoes did not keep the connection alive"
        );
    }

    #[test]
    fn fast_probe_timeout_scales_with_rtt_and_is_clamped() {
        // No sample yet (fresh conn): the floor, so a link is never declared dead
        // before it has been measured.
        assert_eq!(fast_probe_timeout(0), FAST_PROBE_TIMEOUT_MIN);
        // Sub-millisecond link: still the floor.
        assert_eq!(fast_probe_timeout(500), FAST_PROBE_TIMEOUT_MIN);
        // 200ms RTT -> 3x, above the floor and below the cap.
        assert_eq!(fast_probe_timeout(200_000), Duration::from_millis(600));
        // Very high RTT: capped, because a probe cannot conclude faster than the
        // RTT allows.
        assert_eq!(fast_probe_timeout(2_000_000), FAST_PROBE_TIMEOUT_MAX);

        // Detection budget for the common case stays inside the Phase F target.
        let worst_miss = FAST_PROBE_INTERVAL + fast_probe_timeout(0);
        assert!(
            worst_miss * FAST_PROBE_MISSES_TO_DEGRADE < Duration::from_secs(2),
            "3 misses should land under 2s: {worst_miss:?} each"
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
