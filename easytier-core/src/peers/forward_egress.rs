//! Per-destination forward egress queues for the shared [`PeerPacketRouter`][super::peer_manager].
//!
//! ## Problem
//!
//! `PeerPacketRouter` is a single serial loop. When a transit packet's egress
//! awaits `PeerConn::send_msg` (up to 100ms on a full ring), **every** peer's
//! ingress stalls — including destinations whose next hop is healthy. On an
//! asymmetric relay (`A→R` fast, `R→B` slow) that turns into bandwidth collapse
//! plus TCP RTO spikes for unrelated flows.
//!
//! ## What this is (and is not)
//!
//! - **Is**: a bounded per-`dst_peer_id` queue so the router can return
//!   immediately after `try_send`. Each destination has its own drain task that
//!   awaits the real send; a slow B does not HOL-block traffic toward C.
//! - **Is not**: UDP-ring / NIC backpressure (P-AUDIT.6 still rejected). Full
//!   queues still drop with `BufferFull` and
//!   `data_plane_tunnel_buffer_full` — same observability as before.
//!
//! ## Cap
//!
//! [`FORWARD_EGRESS_QUEUE_CAP`] packets per destination. Under sustained
//! overload toward one peer the queue fills and further packets are dropped
//! without parking the router.
//!
//! ## Lane lifetime
//!
//! Drain tasks idle-out after [`DEFAULT_LANE_IDLE_TIMEOUT`] and remove their
//! DashMap entry before exiting. Without that GC, the map-held `Sender` would
//! keep `recv()` alive forever and pin `ForwardEgress` (and its `PeerMap`)
//! across instance rebuilds.

use std::{future::Future, sync::Arc, time::Duration};

use dashmap::DashMap;
use tokio::sync::mpsc::{self, error::TrySendError};

use crate::{
    config::PeerId,
    foundation::stats::CounterHandle,
    packet::ZCPacket,
    peers::{
        conn::peer_map::PeerMap,
        error::{Error, take_congestion_warn_token, take_path_unavailable_warn_token},
        foreign_network::client::ForeignNetworkClient,
        peer_manager::{note_tunnel_buffer_full, send_msg_internal},
        relay_peer_map::RelayPeerMap,
        traffic_metrics::TrafficMetricRecorder,
    },
    tunnel::TunnelError,
};

/// Per-destination forward queue depth.
///
/// Sized to absorb a short TCP burst without holding unbounded memory when the
/// next hop is stuck. Larger than the PeerConn mpsc (32) so the lane can keep a
/// little work ready while the 100ms egress wait drains; small enough that a
/// dead peer cannot pin megabytes.
pub(crate) const FORWARD_EGRESS_QUEUE_CAP: usize = 64;

/// How long a drain task waits for the next packet before reclaiming its lane.
///
/// Must be finite: the DashMap holds a `Sender` clone, so `rx.recv()` alone
/// never sees channel close while the entry remains.
pub(crate) const DEFAULT_LANE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

struct ForwardJob {
    packet: ZCPacket,
    from_peer_id: PeerId,
    /// When true, `send_msg_internal` records self-originated TX metrics.
    record_self_tx_metrics: bool,
    /// Test-only sequence tag; production always leaves this at 0.
    #[cfg(test)]
    seq: u64,
}

/// Per-dst sender table + idle reclaim. Extracted so create-race and GC can be
/// unit-tested without wiring PeerMap / encrypt / route.
struct ForwardLaneTable {
    lanes: DashMap<PeerId, mpsc::Sender<ForwardJob>>,
    capacity: usize,
    idle_timeout: Duration,
}

impl ForwardLaneTable {
    fn new(capacity: usize, idle_timeout: Duration) -> Self {
        Self {
            lanes: DashMap::new(),
            capacity,
            idle_timeout,
        }
    }

    #[cfg(test)]
    fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// Non-blocking enqueue. `spawn_drain` runs only when this call creates the
    /// lane (Vacant); Occupied create-races drop the unused receiver.
    fn try_enqueue<F>(
        &self,
        dst_peer_id: PeerId,
        mut job: ForwardJob,
        spawn_drain: F,
    ) -> Result<(), Error>
    where
        F: Fn(PeerId, mpsc::Receiver<ForwardJob>),
    {
        // Retry once if a closed sender races with idle reclaim / worker exit.
        for _ in 0..2 {
            let sender = self.sender_for(dst_peer_id, &spawn_drain);
            match sender.try_send(job) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(_)) => {
                    return Err(Error::Tunnel(TunnelError::BufferFull));
                }
                Err(TrySendError::Closed(rejected)) => {
                    self.lanes.remove(&dst_peer_id);
                    job = rejected;
                }
            }
        }
        Err(Error::Tunnel(TunnelError::BufferFull))
    }

    fn sender_for<F>(&self, dst_peer_id: PeerId, spawn_drain: &F) -> mpsc::Sender<ForwardJob>
    where
        F: Fn(PeerId, mpsc::Receiver<ForwardJob>),
    {
        if let Some(existing) = self.lanes.get(&dst_peer_id) {
            return existing.clone();
        }
        let (tx, rx) = mpsc::channel(self.capacity);
        match self.lanes.entry(dst_peer_id) {
            dashmap::mapref::entry::Entry::Occupied(o) => {
                // Lost the create race: use the winner. Do not spawn a worker
                // for this unused channel.
                drop(rx);
                o.get().clone()
            }
            dashmap::mapref::entry::Entry::Vacant(v) => {
                spawn_drain(dst_peer_id, rx);
                v.insert(tx.clone());
                tx
            }
        }
    }

    /// Drain until the channel closes or the lane is idle long enough to GC.
    ///
    /// On idle timeout the map entry is removed **before** exiting so concurrent
    /// `try_send` observes `Closed` and `try_enqueue`'s retry can rebuild.
    async fn drain_with<H, Fut>(
        &self,
        dst_peer_id: PeerId,
        mut rx: mpsc::Receiver<ForwardJob>,
        mut handle: H,
    ) where
        H: FnMut(ForwardJob) -> Fut,
        Fut: Future<Output = ()>,
    {
        loop {
            let job = match tokio::time::timeout(self.idle_timeout, rx.recv()).await {
                Ok(Some(job)) => job,
                Ok(None) => break, // all senders dropped
                Err(_) => {
                    // Idle reclaim: unpin the Sender so this task (and anything
                    // it holds) can exit. Race packets that landed after the
                    // timeout are drained below while `rx` is still ours.
                    self.lanes.remove(&dst_peer_id);
                    while let Ok(residual) = rx.try_recv() {
                        handle(residual).await;
                    }
                    return;
                }
            };
            handle(job).await;
        }
        self.lanes.remove(&dst_peer_id);
    }
}

/// Shared by the router (enqueue) and per-dst workers (drain).
pub(crate) struct ForwardEgress {
    peers: Arc<PeerMap>,
    foreign_client: Arc<ForeignNetworkClient>,
    relay_peer_map: Arc<RelayPeerMap>,
    traffic_metrics: Arc<TrafficMetricRecorder>,
    tunnel_buffer_full: CounterHandle,
    table: ForwardLaneTable,
}

impl ForwardEgress {
    pub(crate) fn new(
        peers: Arc<PeerMap>,
        foreign_client: Arc<ForeignNetworkClient>,
        relay_peer_map: Arc<RelayPeerMap>,
        traffic_metrics: Arc<TrafficMetricRecorder>,
        tunnel_buffer_full: CounterHandle,
    ) -> Arc<Self> {
        Self::with_idle_timeout(
            peers,
            foreign_client,
            relay_peer_map,
            traffic_metrics,
            tunnel_buffer_full,
            DEFAULT_LANE_IDLE_TIMEOUT,
        )
    }

    pub(crate) fn with_idle_timeout(
        peers: Arc<PeerMap>,
        foreign_client: Arc<ForeignNetworkClient>,
        relay_peer_map: Arc<RelayPeerMap>,
        traffic_metrics: Arc<TrafficMetricRecorder>,
        tunnel_buffer_full: CounterHandle,
        lane_idle_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            peers,
            foreign_client,
            relay_peer_map,
            traffic_metrics,
            tunnel_buffer_full,
            table: ForwardLaneTable::new(FORWARD_EGRESS_QUEUE_CAP, lane_idle_timeout),
        })
    }

    #[cfg(test)]
    fn lane_count(&self) -> usize {
        self.table.lane_count()
    }

    /// Non-blocking enqueue toward `dst_peer_id`.
    ///
    /// Returns `Ok(())` once the packet is accepted by that destination's lane.
    /// Returns `Err(BufferFull)` when the lane is at capacity — caller must
    /// count and drop; the shared router must not await.
    pub(crate) fn try_enqueue(
        self: &Arc<Self>,
        dst_peer_id: PeerId,
        from_peer_id: PeerId,
        packet: ZCPacket,
        record_self_tx_metrics: bool,
    ) -> Result<(), Error> {
        let job = ForwardJob {
            packet,
            from_peer_id,
            record_self_tx_metrics,
            #[cfg(test)]
            seq: 0,
        };
        let egress = Arc::clone(self);
        self.table.try_enqueue(dst_peer_id, job, move |dst, rx| {
            let egress = Arc::clone(&egress);
            tokio::spawn(async move {
                egress.drain_lane(dst, rx).await;
            });
        })
    }

    async fn drain_lane(self: Arc<Self>, dst_peer_id: PeerId, rx: mpsc::Receiver<ForwardJob>) {
        let this = Arc::clone(&self);
        self.table
            .drain_with(dst_peer_id, rx, move |job| {
                let this = Arc::clone(&this);
                async move {
                    this.dispatch_job(dst_peer_id, job).await;
                }
            })
            .await;
    }

    async fn dispatch_job(&self, dst_peer_id: PeerId, job: ForwardJob) {
        let metrics = if job.record_self_tx_metrics {
            Some(&self.traffic_metrics)
        } else {
            None
        };
        let from_peer_id = job.from_peer_id;
        let result = send_msg_internal(
            self.peers.as_ref(),
            &self.foreign_client,
            &self.relay_peer_map,
            metrics,
            job.packet,
            dst_peer_id,
        )
        .await;
        if let Err(ref e) = result {
            note_tunnel_buffer_full(&self.tunnel_buffer_full, e);
            if e.is_expected_path_unavailable() {
                if take_path_unavailable_warn_token(dst_peer_id) {
                    tracing::warn!(?result, ?dst_peer_id, ?from_peer_id, "forward packet error");
                } else {
                    tracing::debug!(?result, ?dst_peer_id, ?from_peer_id, "forward packet error");
                }
            } else if e.is_congestion() {
                if take_congestion_warn_token(dst_peer_id) {
                    tracing::warn!(
                        ?result,
                        ?dst_peer_id,
                        ?from_peer_id,
                        "forward packet dropped by local congestion"
                    );
                } else {
                    tracing::debug!(
                        ?result,
                        ?dst_peer_id,
                        ?from_peer_id,
                        "forward packet dropped by local congestion"
                    );
                }
            } else {
                tracing::error!(?result, ?dst_peer_id, ?from_peer_id, "forward packet error");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        foundation::stats::{LabelSet, MetricName, StatsManager},
        packet::{PacketType, ZCPacket},
        peers::{
            conn::peer_session::PeerSessionStore,
            create_packet_recv_chan,
            flow_table::{FlowTable, FlowTableConfig},
            foreign_network::client::ForeignNetworkClient,
            relay_peer_map::new_relay_peer_map,
            test_support::NoopPeerContext,
            traffic_metrics::{InstanceLabelKind, LogicalTrafficMetrics, TrafficMetricRecorder},
        },
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::Mutex;

    fn blank_packet() -> ZCPacket {
        let mut pkt = ZCPacket::new_with_payload(&[0u8; 64]);
        pkt.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);
        pkt
    }

    fn job_with_seq(seq: u64) -> ForwardJob {
        ForwardJob {
            packet: blank_packet(),
            from_peer_id: 1,
            record_self_tx_metrics: false,
            seq,
        }
    }

    /// Minimal `ForwardEgress` so `try_enqueue` can run without a live next hop
    /// (dispatch fails with a route error; enqueue semantics still hold).
    fn test_egress(idle: Duration) -> Arc<ForwardEgress> {
        let context = Arc::new(NoopPeerContext::default());
        let (packet_send, _rx) = create_packet_recv_chan();
        let peers = Arc::new(PeerMap::new(
            packet_send.clone(),
            Arc::clone(&context) as _,
            1,
            Arc::new(FlowTable::new(FlowTableConfig::default())),
        ));
        let foreign_client = Arc::new(ForeignNetworkClient::new(
            Arc::clone(&context) as _,
            packet_send,
            1,
        ));
        let relay_peer_map = new_relay_peer_map(
            Arc::clone(&peers),
            Some(Arc::clone(&foreign_client)),
            Arc::clone(&context) as _,
            1,
            Arc::new(PeerSessionStore::new()),
        );
        let stats = Arc::new(StatsManager::new());
        let logical = |bytes: MetricName, packets: MetricName, ib: MetricName, ip: MetricName| {
            Arc::new(LogicalTrafficMetrics::new(
                Arc::clone(&stats),
                "test".to_owned(),
                bytes,
                packets,
                ib,
                ip,
                InstanceLabelKind::To,
            ))
        };
        let traffic_metrics = Arc::new(TrafficMetricRecorder::new(
            1,
            logical(
                MetricName::TrafficBytesTx,
                MetricName::TrafficPacketsTx,
                MetricName::TrafficBytesTxByInstance,
                MetricName::TrafficPacketsTxByInstance,
            ),
            logical(
                MetricName::TrafficControlBytesTx,
                MetricName::TrafficControlPacketsTx,
                MetricName::TrafficControlBytesTxByInstance,
                MetricName::TrafficControlPacketsTxByInstance,
            ),
            logical(
                MetricName::TrafficBytesRx,
                MetricName::TrafficPacketsRx,
                MetricName::TrafficBytesRxByInstance,
                MetricName::TrafficPacketsRxByInstance,
            ),
            logical(
                MetricName::TrafficControlBytesRx,
                MetricName::TrafficControlPacketsRx,
                MetricName::TrafficControlBytesRxByInstance,
                MetricName::TrafficControlPacketsRxByInstance,
            ),
            |_peer_id| async { None },
        ));
        let tunnel_buffer_full =
            stats.get_counter(MetricName::DataPlaneTunnelBufferFull, LabelSet::new());
        ForwardEgress::with_idle_timeout(
            peers,
            foreign_client,
            relay_peer_map,
            traffic_metrics,
            tunnel_buffer_full,
            idle,
        )
    }

    #[test]
    fn queue_cap_constant_is_above_peerconn_mpsc() {
        // PeerConn MpscTunnel uses 32; lane must be able to hold a little more
        // so try_enqueue absorbs bursts the 100ms egress wait can drain.
        assert!(FORWARD_EGRESS_QUEUE_CAP > 32);
    }

    #[tokio::test]
    async fn full_lane_try_enqueue_returns_buffer_full_without_blocking() {
        let table = ForwardLaneTable::new(2, Duration::from_secs(60));
        let dst = 42u32;
        // Park a receiver so the lane never drains — fills and stays full.
        let parked_rx = Arc::new(Mutex::new(None));
        for seq in 0..2 {
            let parked_rx = Arc::clone(&parked_rx);
            table
                .try_enqueue(dst, job_with_seq(seq), move |_d, rx| {
                    let parked_rx = Arc::clone(&parked_rx);
                    tokio::spawn(async move {
                        *parked_rx.lock().await = Some(rx);
                        std::future::pending::<()>().await;
                    });
                })
                .unwrap();
        }
        let err = table
            .try_enqueue(dst, job_with_seq(99), move |_, rx| {
                drop(rx);
            })
            .unwrap_err();
        assert!(matches!(err, Error::Tunnel(TunnelError::BufferFull)));
        // Drop parked receiver so the test runtime can shut down cleanly.
        drop(parked_rx.lock().await.take());
    }

    #[tokio::test]
    async fn same_destination_preserves_enqueue_order() {
        let table = ForwardLaneTable::new(8, Duration::from_secs(60));
        let dst = 7u32;
        let order = Arc::new(Mutex::new(Vec::new()));

        for seq in 1..=5u64 {
            let order = Arc::clone(&order);
            table
                .try_enqueue(dst, job_with_seq(seq), move |_d, rx| {
                    let order = Arc::clone(&order);
                    tokio::spawn(async move {
                        let mut rx = rx;
                        while let Some(job) = rx.recv().await {
                            order.lock().await.push(job.seq);
                        }
                    });
                })
                .unwrap();
        }

        // Drop the map-held Sender so the drain loop observes channel close.
        table.lanes.remove(&dst);
        for _ in 0..50 {
            if order.lock().await.len() == 5 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(*order.lock().await, vec![1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn idle_lane_is_reclaimed_and_closed_retry_rebuilds() {
        // Not ZERO: spawn_drain runs before insert/try_send; with ZERO idle the
        // drain can reclaim twice before try_send and trip the Closed→BufferFull
        // retry ceiling. 5ms is still far below the 50×10ms poll window.
        let table = Arc::new(ForwardLaneTable::new(4, Duration::from_millis(5)));
        let dst = 99u32;
        let handled = Arc::new(AtomicUsize::new(0));

        {
            let table_drain = Arc::clone(&table);
            let handled = Arc::clone(&handled);
            table
                .try_enqueue(dst, job_with_seq(1), move |d, rx| {
                    let table_drain = Arc::clone(&table_drain);
                    let handled = Arc::clone(&handled);
                    tokio::spawn(async move {
                        table_drain
                            .drain_with(d, rx, |_job| {
                                let handled = Arc::clone(&handled);
                                async move {
                                    handled.fetch_add(1, Ordering::SeqCst);
                                }
                            })
                            .await;
                    });
                })
                .unwrap();
        }

        // Let drain process the job and hit the short idle timeout.
        for _ in 0..50 {
            if table.lane_count() == 0 && handled.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(handled.load(Ordering::SeqCst), 1);
        assert_eq!(
            table.lane_count(),
            0,
            "idle timeout must remove the DashMap entry so drain can exit"
        );

        // After reclaim, a new enqueue must rebuild (Closed retry path).
        {
            let table_drain = Arc::clone(&table);
            table
                .try_enqueue(dst, job_with_seq(2), move |d, rx| {
                    let table_drain = Arc::clone(&table_drain);
                    tokio::spawn(async move {
                        table_drain.drain_with(d, rx, |_job| async {}).await;
                    });
                })
                .unwrap();
        }
        for _ in 0..50 {
            if table.lane_count() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(table.lane_count(), 0);
    }

    #[tokio::test]
    async fn independent_destinations_do_not_share_a_lane() {
        let table = ForwardLaneTable::new(1, Duration::from_secs(60));
        let parked = Arc::new(Mutex::new(Vec::new()));

        for dst in [10u32, 20u32] {
            let parked = Arc::clone(&parked);
            table
                .try_enqueue(dst, job_with_seq(dst as u64), move |_d, rx| {
                    let parked = parked.clone();
                    tokio::spawn(async move {
                        parked.lock().await.push(rx);
                        std::future::pending::<()>().await;
                    });
                })
                .unwrap();
        }
        assert_eq!(table.lane_count(), 2);

        let err = table
            .try_enqueue(10, job_with_seq(100), |_, rx| drop(rx))
            .unwrap_err();
        assert!(matches!(err, Error::Tunnel(TunnelError::BufferFull)));

        // A different destination still accepts — fairness across destinations.
        table
            .try_enqueue(11, job_with_seq(11), {
                let parked = Arc::clone(&parked);
                move |_d, rx| {
                    let parked = parked.clone();
                    tokio::spawn(async move {
                        parked.lock().await.push(rx);
                        std::future::pending::<()>().await;
                    });
                }
            })
            .unwrap();
        assert_eq!(table.lane_count(), 3);
        parked.lock().await.clear();
    }

    #[tokio::test]
    async fn forward_egress_try_enqueue_does_not_await_send() {
        let egress = test_egress(Duration::from_millis(5));
        let start = std::time::Instant::now();
        // No route → drain fails quickly; enqueue itself must stay non-blocking.
        egress
            .try_enqueue(55, 1, blank_packet(), false)
            .expect("empty lane accepts");
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "try_enqueue must not await PeerConn send"
        );
        for _ in 0..50 {
            if egress.lane_count() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            egress.lane_count(),
            0,
            "short idle must reclaim the lane after dispatch"
        );
    }
}
