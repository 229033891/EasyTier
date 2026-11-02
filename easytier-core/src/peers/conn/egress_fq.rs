//! Egress scheduling primitives shared by the PeerConn fair queue and the
//! source-side flow-hash dispatch bridges. See docs/current/egress-fair-queue.md.
//!
//! Two shapes of head-of-line blocking are addressed here:
//!
//! 1. **Intra-conn**: the tunnel ring is a single 32-slot FIFO; a bulk flow
//!    filling it stalls interactive flows behind it for up to `EGRESS_WAIT`
//!    per packet. [`EgressFairQueue`] re-schedules egress into per-flow lanes
//!    drained round-robin, with a priority lane for control packets.
//! 2. **Source-side serial bridges**: the TUN reader and the smoltcp→peer
//!    bridge each used to be a single `read → send().await` loop, so one full
//!    conn ring parked *every* flow of that plane. [`FlowHashDispatcher`]
//!    partitions packets across parallel workers by the same inner IP 5-tuple
//!    hash the bond uses — same flow ⇒ same worker ⇒ ordering preserved.

use std::{
    collections::VecDeque,
    sync::{Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use crate::{
    packet::ZCPacket, peers::conn::conn_bond::flow_key_from_payload, tunnel::mpsc::MpscTunnelSender,
};

/// Control packets (everything not matched by `packet::is_data_packet`) go
/// here and are drained first. Keepalives and route RPCs must not queue
/// behind bulk data once fair queueing is on. Sized to match the historical
/// single 32-slot ring so route-convergence / RPC bursts hold no less
/// in-flight control traffic than before fair queueing.
pub(crate) const EGRESS_PRIORITY_CAP: usize = 32;

/// Number of data-flow lanes. A lane is keyed by `flow_key % EGRESS_FLOW_LANES`;
/// collisions merge two flows into one FIFO lane, which degrades gracefully
/// towards FIFO between those two flows only.
pub(crate) const EGRESS_FLOW_LANES: usize = 8;

/// Per-lane queue depth. Sized to match the historical single 32-slot ring so
/// a single bulk flow keeps the same in-flight depth (and slow-link
/// throughput) it had before fair queueing. Total queued packets per conn are
/// bounded by `EGRESS_PRIORITY_CAP + EGRESS_FLOW_LANES * EGRESS_LANE_CAP`
/// (= 288).
pub(crate) const EGRESS_LANE_CAP: usize = 32;

/// Workers in a [`FlowHashDispatcher`]. Sized so a single saturated flow
/// (one worker) cannot stall unrelated flows of the same source plane.
pub(crate) const EGRESS_DISPATCH_WORKERS: usize = 4;

/// Channel depth per dispatcher worker. Matches the scale of the peer
/// dispatch chan (128); a saturated worker parks the dispatcher, which is the
/// same backpressure the old serial bridge had, just flow-scoped.
pub(crate) const EGRESS_DISPATCH_CHAN_CAP: usize = 128;

fn is_control_packet(msg: &ZCPacket) -> bool {
    match msg.peer_manager_header() {
        // Header not yet parseable (handshake-stage packets) is treated as
        // control: small, rare, and must never wait behind bulk data.
        None => true,
        Some(header) => !crate::packet::is_data_packet(header.packet_type),
    }
}

fn flow_lane_of(msg: &ZCPacket) -> usize {
    let key = msg
        .bond_flow_key()
        .unwrap_or_else(|| flow_key_from_payload(msg.payload()));
    (key % EGRESS_FLOW_LANES as u64) as usize
}

struct Lane {
    queue: Mutex<VecDeque<ZCPacket>>,
    /// Signalled (notify_one, permit-storing) whenever the drain task removes
    /// a packet from this lane, so a parked pusher re-checks for space. The
    /// stored-permit guarantee makes the check/wait window race-free without
    /// holding the lock: a permit emitted before registration is still
    /// delivered.
    space_freed: tokio::sync::Notify,
}

impl Lane {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            space_freed: tokio::sync::Notify::new(),
        }
    }

    fn try_pop(&self) -> Option<ZCPacket> {
        // Poison-tolerant: a panic in a pusher/drain must not turn every
        // later push into a panic cascade; the lock guard data stays valid.
        let popped = self
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front();
        if popped.is_some() {
            self.space_freed.notify_one();
        }
        popped
    }
}

/// Per-conn egress fair queue. `push` replaces the direct `sink.send` call
/// site; [`EgressFairQueue::run`] is the drain task that owns the real sink.
///
/// Wake-up protocol:
/// - pusher → drain: permit-storing `work_arrived.notify_one()` after a
///   successful enqueue (a stored permit is consumed by the parked drain;
///   spurious wakeups are absorbed by the emptiness re-check in `pick_next`
///   → park loop).
/// - drain → pusher: permit-storing `lane.space_freed.notify_one()` after a
///   successful pop.
/// - close → everyone: `notify_waiters()` first (wake every parked waiter),
///   then one permit-storing `notify_one()` per `Notify` (covers a pusher
///   racing in the window between its `closed` check and its `notified()`
///   registration; it consumes the permit and fails fast with `Shutdown`
///   instead of timing out with `BufferFull`).
pub(crate) struct EgressFairQueue {
    priority: Lane,
    lanes: Vec<Lane>,
    /// Round-robin cursor: starts at a random lane so no lane index is
    /// structurally favoured across conns.
    rr_cursor: Mutex<usize>,
    /// Set when the drain task observed a dead sink; pushers fail fast after.
    closed: std::sync::atomic::AtomicBool,
    /// Packets rejected after the bounded wait. Observability only: a non-zero
    /// value means this conn's egress is saturated (tunnel slower than load).
    buffer_full_drops: std::sync::atomic::AtomicU64,
    work_arrived: tokio::sync::Notify,
}

impl EgressFairQueue {
    pub(crate) fn new() -> Self {
        Self {
            priority: Lane::new(),
            lanes: (0..EGRESS_FLOW_LANES).map(|_| Lane::new()).collect(),
            rr_cursor: Mutex::new((rand::random::<u64>() as usize) % EGRESS_FLOW_LANES),
            closed: std::sync::atomic::AtomicBool::new(false),
            buffer_full_drops: std::sync::atomic::AtomicU64::new(0),
            work_arrived: tokio::sync::Notify::new(),
        }
    }

    /// Enqueue a packet, waiting up to `wait` for space (same bounded-wait
    /// contract as the direct egress path). `TunnelError::BufferFull` on
    /// timeout; `Shutdown` once the drain task observed a dead sink.
    pub(crate) async fn push(
        &self,
        msg: ZCPacket,
        wait: Duration,
    ) -> Result<(), crate::tunnel::TunnelError> {
        use crate::tunnel::TunnelError;

        let deadline = Instant::now() + wait;
        let (lane, cap) = if is_control_packet(&msg) {
            (&self.priority, EGRESS_PRIORITY_CAP)
        } else {
            let idx = flow_lane_of(&msg);
            (&self.lanes[idx], EGRESS_LANE_CAP)
        };

        loop {
            {
                let mut queue = lane.queue.lock().unwrap_or_else(|e| e.into_inner());
                if self.closed.load(Ordering::Relaxed) {
                    return Err(TunnelError::Shutdown);
                }
                if queue.len() < cap {
                    queue.push_back(msg);
                    drop(queue);
                    self.work_arrived.notify_one();
                    return Ok(());
                }
            }
            let space_freed = lane.space_freed.notified();
            if tokio::time::timeout_at(deadline.into(), space_freed)
                .await
                .is_err()
            {
                self.note_buffer_full();
                return Err(TunnelError::BufferFull);
            }
        }
    }

    /// Count a packet that could not be enqueued within its wait budget.
    ///
    /// Logging is sparse (first drop, powers of two, then every 1000) so a conn
    /// whose tunnel is persistently slower than the offered load surfaces the
    /// fact without flooding the log on every packet. The counter is the
    /// per-conn counterpart of the upstream `BufferFull` tally, which cannot
    /// say *which* conn saturated.
    fn note_buffer_full(&self) {
        let total = self
            .buffer_full_drops
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if total == 1 || total.is_power_of_two() || total.is_multiple_of(1000) {
            tracing::debug!(
                buffer_full_drops = total,
                "egress fair queue: packet dropped after bounded wait (egress saturated)"
            );
        }
    }

    /// Test-only: packets dropped after the bounded wait.
    #[cfg(test)]
    pub(crate) fn buffer_full_drops(&self) -> u64 {
        self.buffer_full_drops.load(Ordering::Relaxed)
    }

    /// Pop the next packet to send: fully drain the priority lane first
    /// (control packets are few and tiny), then one packet per data lane in
    /// round-robin order. `None` when everything is empty.
    fn pick_next(&self) -> Option<ZCPacket> {
        if let Some(p) = self.priority.try_pop() {
            return Some(p);
        }
        let mut cursor = self.rr_cursor.lock().unwrap_or_else(|e| e.into_inner());
        for offset in 0..EGRESS_FLOW_LANES {
            let idx = (*cursor + offset) % EGRESS_FLOW_LANES;
            if let Some(p) = self.lanes[idx].try_pop() {
                *cursor = (idx + 1) % EGRESS_FLOW_LANES;
                return Some(p);
            }
        }
        None
    }

    /// Drain task body: owns the conn's real sink until it errors out.
    pub(crate) async fn run(self: std::sync::Arc<Self>, sink: MpscTunnelSender) {
        loop {
            match self.pick_next() {
                Some(packet) => {
                    if sink.send(packet).await.is_err() {
                        self.close();
                        return;
                    }
                }
                None => {
                    // Empty: park until a pusher signals work. `notify_one`
                    // stores a permit when nobody waits, so a push landing
                    // between the empty check and registration still wakes us.
                    self.work_arrived.notified().await;
                }
            }
        }
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        // Two-phase wake: `notify_waiters` first, so every currently parked
        // pusher observes `closed` and fails fast with Shutdown instead of
        // timing out with BufferFull; then one `notify_one` per Notify, which
        // stores a permit (no waiter is left at this point) for a pusher
        // racing between its `closed` check and its `notified()`
        // registration — `notify_waiters` alone misses that window.
        self.work_arrived.notify_waiters();
        self.work_arrived.notify_one();
        self.priority.space_freed.notify_waiters();
        self.priority.space_freed.notify_one();
        for lane in &self.lanes {
            lane.space_freed.notify_waiters();
            lane.space_freed.notify_one();
        }
    }
}

impl Default for EgressFairQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Parallel workers partitioned by flow hash. Each worker owns a bounded
/// channel; `dispatch` blocks (await) when *that flow's* worker channel is
/// full — the same backpressure the old serial bridge had, flow-scoped.
///
/// Re-exported at `peers::FlowHashDispatcher` for the TUN reader in the
/// `easytier` lib crate.
pub struct FlowHashDispatcher<T> {
    txs: Vec<mpsc::Sender<T>>,
}

impl<T: Send + 'static> FlowHashDispatcher<T> {
    /// Spawn `EGRESS_DISPATCH_WORKERS` workers running `worker(rx)`. The
    /// worker future must loop on `rx.recv()` until the channel closes (all
    /// senders dropped).
    pub fn new<W, Fut>(mut worker: W) -> Self
    where
        W: FnMut(mpsc::Receiver<T>) -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let txs = (0..EGRESS_DISPATCH_WORKERS)
            .map(|_| {
                let (tx, rx) = mpsc::channel::<T>(EGRESS_DISPATCH_CHAN_CAP);
                tokio::spawn(worker(rx));
                tx
            })
            .collect();
        Self { txs }
    }

    /// Route `item` to `flow_key % workers`. Same key ⇒ same worker ⇒ FIFO
    /// ordering within the flow is preserved across workers.
    pub async fn dispatch(&self, flow_key: u64, item: T) -> Result<(), mpsc::error::SendError<T>> {
        let worker = (flow_key % EGRESS_DISPATCH_WORKERS as u64) as usize;
        self.txs[worker].send(item).await
    }
}

/// Public alias for the bond flow-key hash (inner IP 5-tuple), for
/// source-side dispatchers living outside this crate (the TUN reader in the
/// `easytier` lib). Must stay identical to the bond key so the same flow
/// hashes to the same lane/worker everywhere.
pub fn egress_flow_key_for_payload(payload: &[u8]) -> u64 {
    flow_key_from_payload(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::PacketType;

    fn data_packet(seq: u8, flow_key_salt: u64) -> ZCPacket {
        // Payload: a fake IP-ish blob whose hash is deterministic per salt;
        // the exact lane matters less than its stability, so we force the lane
        // via `bond_flow_key` instead of relying on hash distribution.
        let mut p = ZCPacket::new_with_payload(&[seq; 64]);
        p.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);
        p.set_bond_flow_key(flow_key_salt);
        p
    }

    fn control_packet() -> ZCPacket {
        let mut p = ZCPacket::new_with_payload(&[0u8; 16]);
        p.fill_peer_manager_hdr(1, 2, PacketType::Ping as u8);
        p
    }

    /// Control packets must leave before any queued data packet, even when
    /// every data lane is full.
    #[tokio::test]
    async fn fair_queue_priority_lane_bypasses_data_backlog() {
        let q = std::sync::Arc::new(EgressFairQueue::new());
        // Fill every data lane to capacity with data packets.
        for salt in 0..EGRESS_FLOW_LANES as u64 {
            for seq in 0..EGRESS_LANE_CAP as u8 {
                q.push(data_packet(seq, salt), Duration::from_millis(1))
                    .await
                    .unwrap();
            }
        }
        // Control packet pushed last must be picked first.
        let ctrl = control_packet();
        q.push(ctrl, Duration::from_millis(1)).await.unwrap();

        let picked = q.pick_next().unwrap();
        assert_eq!(
            picked.peer_manager_header().unwrap().packet_type,
            PacketType::Ping as u8,
            "priority lane must drain before any data lane"
        );
        // And the data lanes follow, one packet per lane per round.
        let mut types = Vec::new();
        while let Some(p) = q.pick_next() {
            types.push(p.peer_manager_header().unwrap().packet_type);
        }
        assert!(types.iter().all(|t| *t == PacketType::Data as u8));
    }

    /// A flow's packets must come out in the order they were pushed, even
    /// while the round-robin cursor interleaves other lanes.
    #[tokio::test]
    async fn fair_queue_preserves_per_flow_ordering() {
        let q = EgressFairQueue::new();
        let salt = 7; // maps to lane 7 % 8
        for seq in 0..EGRESS_LANE_CAP as u8 {
            q.push(data_packet(seq, salt), Duration::from_millis(1))
                .await
                .unwrap();
        }
        // Give other lanes traffic too, so a per-flow bug cannot hide behind
        // an empty-queue fast path.
        for salt_other in 0..EGRESS_FLOW_LANES as u64 {
            if salt_other != salt {
                q.push(data_packet(255, salt_other), Duration::from_millis(1))
                    .await
                    .unwrap();
            }
        }
        let mut seen_seq_of_salt7 = Vec::new();
        while let Some(p) = q.pick_next() {
            if p.bond_flow_key() == Some(salt) {
                seen_seq_of_salt7.push(p.payload()[0]);
            }
        }
        assert_eq!(
            seen_seq_of_salt7,
            (0..EGRESS_LANE_CAP as u8).collect::<Vec<_>>(),
            "same-flow packets must stay strictly FIFO"
        );
    }

    /// Fairness: a packet from a fresh flow must not wait for the backlog of
    /// a previously queued flow — it leaves within one round-robin round.
    #[tokio::test]
    async fn fair_queue_interleaves_flows() {
        let q = EgressFairQueue::new();
        // Saturate lane 0 with flow A.
        let salt_a = 0;
        for seq in 0..EGRESS_LANE_CAP as u8 {
            q.push(data_packet(seq, salt_a), Duration::from_millis(1))
                .await
                .unwrap();
        }
        // Flow B arrives after the backlog; it lives in lane 3.
        let salt_b = 3;
        q.push(data_packet(99, salt_b), Duration::from_millis(1))
            .await
            .unwrap();

        let mut a_before_b = 0usize;
        let mut b_seen = false;
        for _ in 0..EGRESS_FLOW_LANES {
            let p = q.pick_next().expect("packets queued");
            if p.bond_flow_key() == Some(salt_b) {
                b_seen = true;
                break;
            }
            a_before_b += 1;
        }
        assert!(
            b_seen,
            "flow B's packet must leave within one round-robin round"
        );
        assert!(
            a_before_b < EGRESS_FLOW_LANES,
            "at most one packet per other lane may precede flow B, got {a_before_b}"
        );
    }

    /// A full lane must surface BufferFull after the bounded wait, and the
    /// wait must not exceed the budget.
    #[tokio::test]
    async fn fair_queue_buffer_full_after_bounded_wait() {
        let q = EgressFairQueue::new();
        let salt = 1;
        for seq in 0..EGRESS_LANE_CAP as u8 {
            q.push(data_packet(seq, salt), Duration::from_millis(1))
                .await
                .unwrap();
        }
        let started = Instant::now();
        let result = q
            .push(data_packet(9, salt), Duration::from_millis(20))
            .await;
        assert!(matches!(
            result,
            Err(crate::tunnel::TunnelError::BufferFull)
        ));
        assert_eq!(
            q.buffer_full_drops(),
            1,
            "the rejected packet must be counted for observability"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(15),
            "must honour the wait budget before giving up"
        );
    }

    /// P0 regression guard: `run()` must actually drain the lanes — packets
    /// pushed while the drain task is alive reach the sink — and once the
    /// sink dies, later `push`es must fail fast with `Shutdown` (not hang,
    /// not `BufferFull`, not silently vanish).
    #[tokio::test]
    async fn fair_queue_drain_delivers_and_fails_fast_after_sink_death() {
        let (tx, mut rx) = mpsc::channel::<ZCPacket>(8);
        let sink = MpscTunnelSender::from_tokio_sender(tx);
        let fq = std::sync::Arc::new(EgressFairQueue::new());
        let drain_fq = fq.clone();
        let drain = tokio::spawn(async move { drain_fq.run(sink).await });

        const N: usize = 10;
        for seq in 0..N as u8 {
            fq.push(data_packet(seq, seq as u64), Duration::from_millis(10))
                .await
                .expect("fresh queue must accept packets");
        }
        let mut got = 0;
        while got < N && (rx.recv().await).is_some() {
            got += 1;
        }
        assert_eq!(got, N, "drain task must deliver every pushed packet");

        // Kill the sink: the drain must observe the send error, set `closed`,
        // and subsequent pushes must surface `Shutdown`.
        rx.close();
        let mut saw_shutdown = false;
        for _ in 0..100 {
            match fq.push(data_packet(1, 1), Duration::from_millis(5)).await {
                Err(crate::tunnel::TunnelError::Shutdown) => {
                    saw_shutdown = true;
                    break;
                }
                _ => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
        assert!(
            saw_shutdown,
            "push must return Shutdown after the sink died, not hang or drop"
        );
        drain.abort();
    }

    /// Dispatcher affinity: same flow key always lands on the same worker, in
    /// send order; different keys may spread.
    #[tokio::test]
    async fn flow_hash_dispatcher_keeps_flow_affinity_and_order() {
        use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

        let records: std::sync::Arc<std::sync::Mutex<Vec<(u64, u64, u64)>>> =
            std::sync::Arc::default(); // (flow_key, send_seq, worker_id)
        let worker_id_counter = std::sync::Arc::new(AtomicU64::new(0));

        let dispatcher: FlowHashDispatcher<(u64, u64)> = FlowHashDispatcher::new({
            let records = records.clone();
            let worker_id_counter = worker_id_counter.clone();
            move |mut rx| {
                let records = records.clone();
                let worker_id = worker_id_counter.fetch_add(1, AtomicOrdering::Relaxed);
                async move {
                    while let Some((key, seq)) = rx.recv().await {
                        records.lock().unwrap().push((key, seq, worker_id));
                    }
                }
            }
        });

        for key in 0..16u64 {
            for seq in 0..32u64 {
                dispatcher.dispatch(key, (key, seq)).await.unwrap();
            }
        }
        drop(dispatcher);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if records.lock().unwrap().len() == 16 * 32 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("all dispatched items must be recorded");

        let mut per_flow_worker: std::collections::HashMap<u64, u64> = Default::default();
        let mut per_flow_last_seq: std::collections::HashMap<u64, u64> = Default::default();
        for (key, seq, worker) in records.lock().unwrap().iter() {
            if let Some(prev_worker) = per_flow_worker.insert(*key, *worker) {
                assert_eq!(prev_worker, *worker, "flow {key} changed workers");
            }
            if let Some(last) = per_flow_last_seq.insert(*key, *seq) {
                assert!(seq > &last, "flow {key} out of order: {seq} after {last}");
            }
        }
        for key in 0..16u64 {
            assert_eq!(per_flow_last_seq[&key], 31, "flow {key} missing tail");
        }
    }
}
