//! Probe-only instrumentation for the two paths that can silently eat packets:
//! the **egress** queue and the **ingress filter chain**.
//!
//! Diagnostic code: no queue size, timeout or delivery semantic is changed —
//! only a handful of relaxed atomics per packet plus one 1 Hz reporter task.
//!
//! ## Why this exists
//!
//! The 2026-11-09 capture pair (both ends on this instrumented build) showed
//! the *sender* reporting
//!
//! ```text
//! pingpong egress full for too long, closing the connection  congested_ms=39999
//! ```
//!
//! in every stall, starting 5–6 s **before** the receiver logged anything. So
//! the jam is on the sender's egress path and the receiver's frozen recv loop
//! is a consequence. But "a queue is full" does not say *where* it is stuck,
//! and the candidates need different fixes:
//!
//! 1. the forward task is inside `sink.feed()` / `sink.flush()` — the UDP ring
//!    or the socket write never completes (`feed_age_ms` keeps growing),
//! 2. the forward task is never scheduled (runtime starvation — `writer_idle_ms`
//!    grows while `feed_in_flight` is false), or
//! 3. the queue is genuinely saturated but draining (everything small,
//!    `queued` oscillates) — not a bug.
//!
//! On the receive side the open question is different: when the loop is parked
//! for 25 s with 46–55 MB/s arriving at the NIC and zero kernel drops, did the
//! frames reach this process and get dropped inside the filter chain (decrypt /
//! replay window / sequence gap), or did they never arrive? The `INGRESS`
//! counters answer that by counting what the *inner* stream yielded versus what
//! the filter chain actually let through.

use std::{
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Must match `channel(32)` in [`crate::tunnel::mpsc::MpscTunnel::new`].
pub const EGRESS_QUEUE_CAP: u64 = 32;

/// A single `feed`/`flush` slower than this is reported on its own.
const SLOW_FEED_MS: u64 = 200;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

pub struct EgressProbe {
    name: String,
    registered: bool,
    /// Items currently sitting in the 32-slot egress channel.
    queued: AtomicU64,
    enqueued: AtomicU64,
    /// `try_send` rejected because the channel was full (`BufferFull`) — the
    /// exact failure the pinger turns into `LocalCongestion`.
    enq_full: AtomicU64,
    /// `enq_full` as of the last report, to derive a per-second delta.
    enq_full_reported: AtomicU64,
    /// `send()` had to wait for capacity — the unbounded `await` fallback.
    enq_blocked: AtomicU64,
    enq_blocked_max_ms: AtomicU64,
    feeds: AtomicU64,
    last_feed_ms: AtomicU64,
    max_feed_ms: AtomicU64,
    /// Set while a `feed`/`flush` future is still pending.
    feed_in_flight: AtomicBool,
    feed_started_ms: AtomicU64,
    last_feed_start_ms: AtomicU64,
    /// When the forward task last entered `rx.recv()`.
    last_poll_ms: AtomicU64,
    jammed: AtomicBool,
}

impl EgressProbe {
    fn new(name: String, registered: bool) -> Self {
        Self {
            name,
            registered,
            queued: AtomicU64::new(0),
            enqueued: AtomicU64::new(0),
            enq_full: AtomicU64::new(0),
            enq_full_reported: AtomicU64::new(0),
            enq_blocked: AtomicU64::new(0),
            enq_blocked_max_ms: AtomicU64::new(0),
            feeds: AtomicU64::new(0),
            last_feed_ms: AtomicU64::new(0),
            max_feed_ms: AtomicU64::new(0),
            feed_in_flight: AtomicBool::new(false),
            feed_started_ms: AtomicU64::new(0),
            last_feed_start_ms: AtomicU64::new(0),
            last_poll_ms: AtomicU64::new(now_ms()),
            jammed: AtomicBool::new(false),
        }
    }

    /// Not wired into the 1 Hz reporter (unit tests, throwaway sinks).
    pub fn detached(name: &str) -> Self {
        Self::new(name.to_owned(), false)
    }

    /// Create a probe and start the shared 1 Hz reporter on first use.
    pub fn register(name: String) -> Arc<Self> {
        let probe = Arc::new(Self::new(name, true));
        {
            let mut probes = registry().lock().unwrap();
            // Held weakly: a dropped `MpscTunnel` (conn closed, task aborted)
            // must stop reporting. With strong refs the 2026-11-09 05:48 .16
            // capture re-logged a dead tunnel every second for 108 s with a
            // frozen `feeds` counter, which reads exactly like a 108 s jam.
            probes.retain(|p| p.strong_count() > 0);
            if probes.len() < 256 {
                probes.push(Arc::downgrade(&probe));
            }
        }
        start_reporter();
        probe
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn enq_full(&self) -> u64 {
        self.enq_full.load(Ordering::Relaxed)
    }

    pub fn note_enqueue(&self) {
        self.queued.fetch_add(1, Ordering::Relaxed);
        self.enqueued.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_dequeue(&self) {
        let _ = self
            .queued
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
    }

    pub fn note_enq_full(&self) {
        self.enq_full.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_enq_blocked(&self, waited: Duration) {
        self.enq_blocked.fetch_add(1, Ordering::Relaxed);
        self.enq_blocked_max_ms
            .fetch_max(waited.as_millis() as u64, Ordering::Relaxed);
    }

    /// Longest the data plane has waited to enqueue on this full queue since the
    /// conn was created; zero if it never had to wait.
    ///
    /// This is what the keepalive's control-egress budget is derived from — the
    /// control path deliberately does not report here, see
    /// `MpscTunnelSender::send_control`.
    pub fn enq_blocked_peak(&self) -> Duration {
        Duration::from_millis(self.enq_blocked_max_ms.load(Ordering::Relaxed))
    }

    pub fn note_poll(&self) {
        self.last_poll_ms.store(now_ms(), Ordering::Relaxed);
    }

    /// Call immediately before awaiting `feed`/`flush`.
    pub fn note_feed_begin(&self) {
        let now = now_ms();
        self.feed_started_ms.store(now, Ordering::Relaxed);
        self.last_feed_start_ms.store(now, Ordering::Relaxed);
        self.feed_in_flight.store(true, Ordering::Relaxed);
    }

    /// Call immediately after `feed`/`flush` resolves.
    pub fn note_feed(&self, elapsed: Duration) {
        self.feed_in_flight.store(false, Ordering::Relaxed);
        let ms = elapsed.as_millis() as u64;
        self.feeds.fetch_add(1, Ordering::Relaxed);
        self.last_feed_ms.store(ms, Ordering::Relaxed);
        self.max_feed_ms.fetch_max(ms, Ordering::Relaxed);
    }

    fn report_tick(&self) {
        let queued = self.queued.load(Ordering::Relaxed);
        let max_feed_ms = self.max_feed_ms.swap(0, Ordering::Relaxed);
        let now = now_ms();
        let writer_idle_ms = now.saturating_sub(self.last_poll_ms.load(Ordering::Relaxed));
        // How long the *current* feed/flush has been pending. This is the field
        // that separates "stuck inside the write" from "task never scheduled":
        //   feed_age_ms > 0  -> the future is still pending right now
        //   feed_age_ms == 0 -> nothing is in flight, so the writer is either
        //                       idle (no traffic) or not being scheduled
        let feed_age_ms = if self.feed_in_flight.load(Ordering::Relaxed) {
            now.saturating_sub(self.feed_started_ms.load(Ordering::Relaxed))
        } else {
            0
        };

        // `queued` is deliberately NOT part of the trigger: items still in the
        // channel when a tunnel is dropped never get a matching dequeue, so the
        // counter can drift above the capacity (observed `queued=33 cap=32` on
        // the 2026-11-09 05:48 .16 capture, on tunnels whose writer was
        // demonstrably healthy). The trigger uses only monotonic, self-healing
        // signals.
        let enq_full = self.enq_full.load(Ordering::Relaxed);
        let enq_full_delta =
            enq_full.saturating_sub(self.enq_full_reported.swap(enq_full, Ordering::Relaxed));

        let backed_up =
            max_feed_ms >= SLOW_FEED_MS || feed_age_ms >= SLOW_FEED_MS || enq_full_delta >= 3;
        if backed_up {
            self.jammed.store(true, Ordering::Relaxed);
            tracing::warn!(
                tunnel = %self.name,
                queued,
                cap = EGRESS_QUEUE_CAP,
                writer_idle_ms,
                feed_age_ms,
                max_feed_ms,
                enq_full_delta,
                last_feed_ms = self.last_feed_ms.load(Ordering::Relaxed),
                feeds = self.feeds.load(Ordering::Relaxed),
                enqueued = self.enqueued.load(Ordering::Relaxed),
                enq_full,
                enq_blocked = self.enq_blocked.load(Ordering::Relaxed),
                enq_blocked_max_ms = self.enq_blocked_max_ms.load(Ordering::Relaxed),
                "EGRESS PROBE: egress queue backed up"
            );
        } else if self.jammed.swap(false, Ordering::Relaxed) {
            tracing::warn!(
                tunnel = %self.name,
                queued,
                feeds = self.feeds.load(Ordering::Relaxed),
                "EGRESS PROBE: egress queue cleared"
            );
        }
    }
}

/// Counters for the ingress filter chain.
///
/// `inner_ready` is bumped for every item the *raw* tunnel stream yields;
/// `swallowed` for every item a filter dropped (`after_received` returned
/// `None`); `passed` for every item actually handed to the recv loop. If
/// `inner_ready` advances while `passed` does not, frames are being eaten
/// inside the process — the one hypothesis the kernel counters cannot test.
pub mod ingress {
    use super::{AtomicBool, AtomicU64, Ordering, now_ms};

    pub static INNER_READY: AtomicU64 = AtomicU64::new(0);
    pub static SWALLOWED: AtomicU64 = AtomicU64::new(0);
    pub static PASSED: AtomicU64 = AtomicU64::new(0);

    static STUCK: AtomicBool = AtomicBool::new(false);

    pub fn note_inner_ready() {
        INNER_READY.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_swallowed() {
        SWALLOWED.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_passed() {
        PASSED.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn report_tick(prev: &mut (u64, u64, u64)) {
        let inner = INNER_READY.load(Ordering::Relaxed);
        let swallowed = SWALLOWED.load(Ordering::Relaxed);
        let passed = PASSED.load(Ordering::Relaxed);
        let (p_inner, p_swallowed, p_passed) = *prev;
        *prev = (inner, swallowed, passed);

        let d_inner = inner.saturating_sub(p_inner);
        let d_swallowed = swallowed.saturating_sub(p_swallowed);
        let d_passed = passed.saturating_sub(p_passed);

        // Frames are arriving from the raw stream but none of them reaches the
        // recv loop: something in the chain is eating every packet.
        let stuck = d_inner > 0 && d_passed == 0 && d_swallowed > 0;
        if stuck {
            STUCK.store(true, Ordering::Relaxed);
            tracing::warn!(
                inner_ready = d_inner,
                passed = d_passed,
                swallowed = d_swallowed,
                total_inner = inner,
                total_swallowed = swallowed,
                ts_ms = now_ms(),
                "INGRESS PROBE: filter chain swallowed every packet"
            );
        } else if STUCK.swap(false, Ordering::Relaxed) {
            tracing::warn!(
                inner_ready = d_inner,
                passed = d_passed,
                swallowed = d_swallowed,
                "INGRESS PROBE: filter chain flowing again"
            );
        }
    }
}

fn registry() -> &'static Mutex<Vec<std::sync::Weak<EgressProbe>>> {
    static PROBES: OnceLock<Mutex<Vec<std::sync::Weak<EgressProbe>>>> = OnceLock::new();
    PROBES.get_or_init(|| Mutex::new(Vec::new()))
}

fn start_reporter() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    // `register` is called from async contexts only; if that ever changes,
    // simply skip the reporter rather than panicking.
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async {
        let mut ingress_prev = (0u64, 0u64, 0u64);
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let probes: Vec<Arc<EgressProbe>> = {
                let registry = registry().lock().unwrap();
                registry
                    .iter()
                    .filter_map(|weak| weak.upgrade())
                    .filter(|probe| probe.registered)
                    .collect()
            };
            for probe in probes {
                probe.report_tick();
            }
            ingress::report_tick(&mut ingress_prev);
        }
    });
}
