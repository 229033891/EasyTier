//! Short-lived holding area for packets whose destination has no route yet.
//!
//! ## Why this exists
//!
//! `PeerManagerCore::send_msg_by_ip` used to drop a packet outright when the
//! destination resolved to no peer (`data_plane_no_dst_peer_drops`). That is
//! correct for a permanently unknown destination, but wrong for the first
//! packet of a flow that is racing route installation: when a peer has just
//! connected and advertised a subnet, the OS route to that subnet lands a few
//! hundred milliseconds later, and every packet sent in between is thrown away.
//! The application then waits out its own retry timer (TCP's initial RTO is
//! ~1s) before the connection even starts.
//!
//! So a packet that finds no route is parked here for at most [`PENDING_TTL`]
//! and retried by the flusher until the route appears. Once it does, the
//! original first packet goes out and the connection starts immediately.
//!
//! ## Boundaries (deliberate)
//!
//! - **Bounded**: at most [`MAX_PER_DST`] per destination and [`MAX_TOTAL`]
//!   overall. Over the cap the packet is dropped and counted exactly as before,
//!   so a permanently unroutable destination costs a bounded amount of memory
//!   for a bounded time and nothing else.
//! - **Never blocks the sender.** This is not the backpressure change that
//!   P-AUDIT.6 rejected: nothing here waits, and no send path is delayed.
//! - **Policy `Block` is not buffered** — see `send_msg_by_ip`. That path is a
//!   deliberate "drop rather than leak" decision, and parking a packet there
//!   would quietly soften a security boundary.
//! - **Broadcast / multicast is not buffered**: an empty fanout there means
//!   there is nobody to send to, not that a route is missing.
//! - **`not_send_to_self` travels with the packet.** Re-sending with a
//!   different value would change loop-prevention flags on the flushed packet,
//!   so the caller's original choice is preserved.

use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::Mutex,
    time::Duration,
};

use quanta::Instant;
use tokio::sync::Notify;

use crate::packet::ZCPacket;

/// How long an unroutable packet is held before it is finally dropped.
///
/// Sized against the route-install window: one route-sync RPC round trip plus
/// the OS route call. 500ms covers a slow relay path; beyond that the
/// application's own retransmit (TCP initial RTO ~1s) is the better backstop,
/// and holding longer only delays the drop without saving the packet.
pub(crate) const PENDING_TTL: Duration = Duration::from_millis(500);

/// Per-destination cap. A TCP handshake burst is a handful of packets; more than
/// that for one destination means the route is not coming back soon.
const MAX_PER_DST: usize = 4;

/// Global cap across all destinations.
const MAX_TOTAL: usize = 64;

/// One parked packet plus everything needed to re-send it faithfully.
pub(crate) struct Parked {
    pub dst: IpAddr,
    pub packet: ZCPacket,
    pub expires_at: Instant,
    pub not_send_to_self: bool,
}

struct Entry {
    packet: ZCPacket,
    expires_at: Instant,
    not_send_to_self: bool,
}

#[derive(Default)]
struct Inner {
    by_dst: HashMap<IpAddr, VecDeque<Entry>>,
    total: usize,
}

/// Packets removed by [`PendingPackets::take_all`].
pub(crate) struct Taken {
    /// Still within TTL. Carries its original expiry so an attempt that is
    /// still unroutable can be put back without extending the packet's life.
    pub ready: Vec<Parked>,
    /// TTL elapsed while parked — these are real drops.
    pub expired: usize,
}

pub(crate) struct PendingPackets {
    inner: Mutex<Inner>,
    wake: Notify,
}

impl PendingPackets {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            wake: Notify::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().total == 0
    }

    /// Park `packet` until `expires_at`.
    ///
    /// Returns `false` when the queue is full; the caller then counts a real
    /// drop, so the existing `data_plane_no_dst_peer_drops` signal keeps its
    /// meaning.
    pub(crate) fn push(&self, parked: Parked) -> bool {
        let now = Instant::now();
        let dst = parked.dst;
        {
            let mut inner = self.inner.lock().unwrap();
            // Reclaim this destination's expired slots first so a stale queue
            // can never block a fresh packet.
            let mut reclaimed = 0;
            if let Some(queue) = inner.by_dst.get_mut(&dst) {
                let before = queue.len();
                queue.retain(|entry| entry.expires_at > now);
                reclaimed = before - queue.len();
            }
            inner.total -= reclaimed;
            if inner.total >= MAX_TOTAL {
                return false;
            }
            let mut evicted = 0;
            {
                let queue = inner.by_dst.entry(dst).or_default();
                if queue.len() >= MAX_PER_DST {
                    // Keep the newest: a retransmit is a fresher statement of
                    // intent than the packet it replaces.
                    queue.pop_front();
                    evicted = 1;
                }
                queue.push_back(Entry {
                    packet: parked.packet,
                    expires_at: parked.expires_at,
                    not_send_to_self: parked.not_send_to_self,
                });
            }
            inner.total = inner.total - evicted + 1;
        }
        // `notify_one` stores a permit when the flusher is not waiting, so a
        // push that races with the flusher's idle wait is never lost.
        self.wake.notify_one();
        true
    }

    /// Remove everything, splitting out entries whose TTL has elapsed.
    pub(crate) fn take_all(&self, now: Instant) -> Taken {
        let mut inner = self.inner.lock().unwrap();
        let by_dst = std::mem::take(&mut inner.by_dst);
        inner.total = 0;

        let mut ready = Vec::new();
        let mut expired = 0usize;
        for (dst, queue) in by_dst {
            for entry in queue {
                if entry.expires_at > now {
                    ready.push(Parked {
                        dst,
                        packet: entry.packet,
                        expires_at: entry.expires_at,
                        not_send_to_self: entry.not_send_to_self,
                    });
                } else {
                    expired += 1;
                }
            }
        }
        Taken { ready, expired }
    }

    /// Re-park a packet that is still unroutable, keeping its original expiry.
    ///
    /// Bypasses the per-destination cap on purpose: the packet was already
    /// admitted, and dropping it here would silently shorten its TTL. Returns
    /// `false` only if the global cap was reached in the meantime.
    pub(crate) fn put_back(&self, parked: Parked) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.total >= MAX_TOTAL {
            return false;
        }
        inner
            .by_dst
            .entry(parked.dst)
            .or_default()
            .push_back(Entry {
                packet: parked.packet,
                expires_at: parked.expires_at,
                not_send_to_self: parked.not_send_to_self,
            });
        inner.total += 1;
        true
    }

    /// Resolve once at least one packet is parked.
    pub(crate) async fn wait_for_work(&self) {
        loop {
            if !self.is_empty() {
                return;
            }
            self.wake.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, last))
    }

    fn packet() -> ZCPacket {
        ZCPacket::new_with_payload(b"hello")
    }

    fn parked(dst: IpAddr, deadline: Instant) -> Parked {
        Parked {
            dst,
            packet: packet(),
            expires_at: deadline,
            not_send_to_self: true,
        }
    }

    #[test]
    fn push_then_take_returns_the_packet() {
        let pending = PendingPackets::new();
        let now = Instant::now();
        assert!(pending.push(parked(ip(1), now + PENDING_TTL)));
        assert!(!pending.is_empty());

        let taken = pending.take_all(now);
        assert_eq!(taken.ready.len(), 1);
        assert_eq!(taken.ready[0].dst, ip(1));
        assert_eq!(taken.expired, 0);
        assert!(pending.is_empty());
    }

    #[test]
    fn entries_past_their_ttl_are_reported_as_expired() {
        let pending = PendingPackets::new();
        let now = Instant::now();
        pending.push(parked(ip(1), now + PENDING_TTL));
        pending.push(parked(ip(2), now + PENDING_TTL));

        let taken = pending.take_all(now + PENDING_TTL + Duration::from_millis(1));
        assert!(taken.ready.is_empty());
        assert_eq!(taken.expired, 2);
    }

    #[test]
    fn per_destination_cap_keeps_the_newest_packets() {
        let pending = PendingPackets::new();
        let now = Instant::now();
        for _ in 0..MAX_PER_DST + 3 {
            assert!(pending.push(parked(ip(1), now + PENDING_TTL)));
        }

        let taken = pending.take_all(now);
        assert_eq!(taken.ready.len(), MAX_PER_DST);
    }

    #[test]
    fn global_cap_rejects_and_reports_a_drop() {
        let pending = PendingPackets::new();
        let now = Instant::now();
        for i in 0..MAX_TOTAL {
            // One destination per packet so the per-destination cap never binds.
            assert!(pending.push(parked(ip(i as u8), now + PENDING_TTL)));
        }
        assert!(
            !pending.push(parked(ip(200), now + PENDING_TTL)),
            "over the global cap the caller must fall back to a counted drop"
        );
    }

    #[test]
    fn put_back_preserves_the_original_expiry() {
        let pending = PendingPackets::new();
        let now = Instant::now();
        let deadline = now + PENDING_TTL;
        pending.push(parked(ip(1), deadline));

        let taken = pending.take_all(now);
        let mut parked = taken.ready.into_iter().next().unwrap();
        // Re-send attempt failed; put the very same packet back.
        parked.packet = packet();
        assert!(pending.put_back(parked));

        // The re-parked packet must expire on the original schedule, not on a
        // fresh one — otherwise a still-unroutable destination could be held
        // far beyond PENDING_TTL.
        let later = pending.take_all(deadline + Duration::from_millis(1));
        assert_eq!(later.expired, 1);
    }

    #[test]
    fn expired_slots_do_not_block_a_fresh_packet() {
        let pending = PendingPackets::new();
        let now = Instant::now();
        let stale = now - PENDING_TTL - Duration::from_millis(1);
        for _ in 0..MAX_PER_DST {
            pending.push(parked(ip(1), stale));
        }
        assert!(pending.push(parked(ip(1), now + PENDING_TTL)));
    }

    #[test]
    fn not_send_to_self_survives_a_round_trip() {
        let pending = PendingPackets::new();
        let now = Instant::now();
        let mut entry = parked(ip(1), now + PENDING_TTL);
        entry.not_send_to_self = false;
        pending.push(entry);

        let taken = pending.take_all(now);
        assert!(
            !taken.ready[0].not_send_to_self,
            "flushing with a different value would change loop-prevention flags"
        );
    }
}
