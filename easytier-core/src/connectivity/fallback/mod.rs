//! Ordered peer-URL fallback orchestration (connection-stability P0.2).
//!
//! The ordered list is the same peer URL set as ManualConnector
//! (`[[peer]]` / `peer_urls` / `public_server_url`). Dial eligibility is gated by
//! `ConnectionPathTier` with hysteresis when escalating the active index.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use easytier_proto::common::ConnectionPathTier;
use url::Url;

use crate::config::{infer_connection_path_tier, normalize_connection_path_tier};

/// Poison-tolerant mutex guard: a poisoned mutex (previous holder panicked)
/// still yields usable state for the reconnect loop instead of panicking it.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Why the controller sits at the current fallback index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    Initial,
    /// Escalated because every eligible peer URL was dead for N ticks.
    AllActiveDead,
    /// Tier PreferRelay / RelayOnly keeps all configured URLs eligible.
    TierDialAll,
}

impl FallbackReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::AllActiveDead => "all_active_dead",
            Self::TierDialAll => "tier_dial_all",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackStatus {
    pub current_fallback_index: usize,
    pub reason: FallbackReason,
    pub connection_path_tier: ConnectionPathTier,
    pub ordered_urls: Vec<Url>,
}

/// Tracks ordered peer URLs and which prefix is eligible to dial.
pub struct FallbackController {
    ordered: Mutex<Vec<Url>>,
    /// Runtime-added URLs that stay eligible under DirectFirst even when the
    /// fallback index has not escalated to them (and after recover shrinks).
    pinned: Mutex<HashSet<Url>>,
    index: AtomicUsize,
    reason: Mutex<FallbackReason>,
    unhealthy_ticks: AtomicU32,
    healthy_ticks: AtomicU32,
    escalate_after: u32,
    recover_after: u32,
}

impl Default for FallbackController {
    fn default() -> Self {
        Self::new()
    }
}

impl FallbackController {
    pub fn new() -> Self {
        Self {
            ordered: Mutex::new(Vec::new()),
            pinned: Mutex::new(HashSet::new()),
            index: AtomicUsize::new(0),
            reason: Mutex::new(FallbackReason::Initial),
            unhealthy_ticks: AtomicU32::new(0),
            healthy_ticks: AtomicU32::new(0),
            // ~3s / ~5s with ManualConnector's 1s reconnect interval.
            escalate_after: 3,
            recover_after: 5,
        }
    }

    #[cfg(test)]
    fn with_hysteresis(escalate_after: u32, recover_after: u32) -> Self {
        Self {
            escalate_after,
            recover_after,
            ..Self::new()
        }
    }

    pub fn add_url(&self, url: Url) {
        let mut ordered = lock(&self.ordered);
        if !ordered.iter().any(|existing| existing == &url) {
            ordered.push(url);
        }
    }

    /// Append `url` and keep it dialable under DirectFirst regardless of index.
    /// Used for runtime `add_connector` after ManualConnector has started.
    pub fn pin_url(&self, url: Url) {
        // Lock order: `ordered` before `pinned`.
        {
            let mut ordered = lock(&self.ordered);
            if !ordered.iter().any(|existing| existing == &url) {
                ordered.push(url.clone());
            }
        }
        lock(&self.pinned).insert(url);
    }

    pub fn remove_url(&self, url: &Url) {
        // Lock order: `ordered` before `pinned` (never nest the reverse).
        {
            let mut ordered = lock(&self.ordered);
            if let Some(pos) = ordered.iter().position(|existing| existing == url) {
                ordered.remove(pos);
                let cur = self.index.load(Ordering::Relaxed);
                let new_idx = if ordered.is_empty() {
                    0
                } else if pos < cur {
                    // List shifted left; keep pointing at the same logical URL.
                    cur - 1
                } else if pos == cur {
                    // Removed the active entry; stay at this slot (next URL) or clamp.
                    cur.min(ordered.len() - 1)
                } else {
                    cur.min(ordered.len() - 1)
                };
                self.index.store(new_idx, Ordering::Relaxed);
                self.unhealthy_ticks.store(0, Ordering::Relaxed);
                self.healthy_ticks.store(0, Ordering::Relaxed);
            }
        }
        lock(&self.pinned).remove(url);
    }

    pub fn clear(&self) {
        // Lock order: `ordered` before `pinned`, then `reason` alone.
        lock(&self.ordered).clear();
        lock(&self.pinned).clear();
        self.index.store(0, Ordering::Relaxed);
        *lock(&self.reason) = FallbackReason::Initial;
        self.unhealthy_ticks.store(0, Ordering::Relaxed);
        self.healthy_ticks.store(0, Ordering::Relaxed);
    }

    pub fn ordered_urls(&self) -> Vec<Url> {
        lock(&self.ordered).clone()
    }

    pub fn current_index(&self) -> usize {
        self.index.load(Ordering::Relaxed)
    }

    pub fn reason(&self) -> FallbackReason {
        *lock(&self.reason)
    }

    pub fn status(&self, tier: ConnectionPathTier) -> FallbackStatus {
        // Lock order (when both needed): never nest `ordered` + `reason`.
        // Snapshot ordered first, then reason, so this cannot deadlock with
        // `on_reconnect_tick` (which releases `ordered` before taking `reason`).
        let ordered_urls = self.ordered_urls();
        let current_fallback_index = self.current_index();
        let reason = self.reason();
        FallbackStatus {
            current_fallback_index,
            reason,
            connection_path_tier: normalize_connection_path_tier(tier),
            ordered_urls,
        }
    }

    /// URLs that ManualConnector may dial / reconnect for this tier.
    pub fn eligible_urls(&self, tier: ConnectionPathTier) -> HashSet<Url> {
        // Snapshot without nesting locks: never hold `ordered` while taking `pinned`
        // (remove_url / clear use ordered-then-pinned; nesting the reverse deadlocks).
        let ordered = lock(&self.ordered).clone();
        if ordered.is_empty() {
            return HashSet::new();
        }
        let tier = normalize_connection_path_tier(tier);
        let max_idx = match tier {
            ConnectionPathTier::PreferRelay | ConnectionPathTier::RelayOnly => {
                ordered.len().saturating_sub(1)
            }
            ConnectionPathTier::DirectFirst | ConnectionPathTier::Unspecified => {
                self.index.load(Ordering::Relaxed).min(ordered.len() - 1)
            }
        };
        let mut eligible: HashSet<Url> = ordered.into_iter().take(max_idx + 1).collect();
        // Pinned URLs stay dialable even when DirectFirst index has not reached them.
        eligible.extend(lock(&self.pinned).iter().cloned());
        eligible
    }

    /// Update index/reason from one reconnect tick.
    ///
    /// `alive` = peer URLs that currently have a live client tunnel.
    pub fn on_reconnect_tick(&self, alive: &HashSet<Url>, tier: ConnectionPathTier) {
        let tier = normalize_connection_path_tier(tier);
        // Read reason alone first — never nest with `ordered`.
        let prev_reason = self.reason();
        // Compute under `ordered`, then drop it before locking `reason`
        // (avoids deadlock with `status()` / management readers).
        let mut next_reason: Option<FallbackReason> = None;
        {
            let ordered = lock(&self.ordered);
            if ordered.is_empty() {
                return;
            }

            if matches!(
                tier,
                ConnectionPathTier::PreferRelay | ConnectionPathTier::RelayOnly
            ) {
                let target = ordered.len() - 1;
                self.index.store(target, Ordering::Relaxed);
                self.unhealthy_ticks.store(0, Ordering::Relaxed);
                self.healthy_ticks.store(0, Ordering::Relaxed);
                next_reason = Some(FallbackReason::TierDialAll);
            } else {
                // PreferRelay/RelayOnly → DirectFirst: shrink immediately to the
                // lowest alive URL (or 0). Do not keep dialing the full list
                // until recover_after hysteresis elapses.
                let mut cur = self.index.load(Ordering::Relaxed).min(ordered.len() - 1);
                if prev_reason == FallbackReason::TierDialAll {
                    let target = ordered
                        .iter()
                        .position(|url| alive.contains(url))
                        .unwrap_or(0)
                        .min(ordered.len() - 1);
                    if target != cur {
                        tracing::info!(
                            from = cur,
                            to = target,
                            "fallback controller shrunk index after leaving dial-all tier"
                        );
                    }
                    self.index.store(target, Ordering::Relaxed);
                    self.unhealthy_ticks.store(0, Ordering::Relaxed);
                    self.healthy_ticks.store(0, Ordering::Relaxed);
                    next_reason = Some(FallbackReason::Initial);
                    cur = target;
                }

                // DirectFirst: escalate when every currently eligible URL is dead.
                let eligible_all_dead =
                    ordered.iter().take(cur + 1).all(|url| !alive.contains(url));
                let any_eligible_alive =
                    ordered.iter().take(cur + 1).any(|url| alive.contains(url));

                if eligible_all_dead {
                    self.healthy_ticks.store(0, Ordering::Relaxed);
                    let n = self.unhealthy_ticks.fetch_add(1, Ordering::Relaxed) + 1;
                    if n >= self.escalate_after && cur + 1 < ordered.len() {
                        self.index.store(cur + 1, Ordering::Relaxed);
                        self.unhealthy_ticks.store(0, Ordering::Relaxed);
                        next_reason = Some(FallbackReason::AllActiveDead);
                        tracing::info!(
                            from = cur,
                            to = cur + 1,
                            reason = "all_active_dead",
                            "fallback controller escalated peer URL index"
                        );
                    }
                } else if any_eligible_alive {
                    self.unhealthy_ticks.store(0, Ordering::Relaxed);
                    let n = self.healthy_ticks.fetch_add(1, Ordering::Relaxed) + 1;
                    // Optional de-escalate: shrink toward lowest alive index after recover_after.
                    if n >= self.recover_after
                        && let Some(lowest_alive) =
                            ordered.iter().position(|url| alive.contains(url))
                        && lowest_alive < cur
                    {
                        self.index.store(lowest_alive, Ordering::Relaxed);
                        self.healthy_ticks.store(0, Ordering::Relaxed);
                        next_reason = Some(FallbackReason::Initial);
                        tracing::info!(
                            from = cur,
                            to = lowest_alive,
                            "fallback controller recovered to lower peer URL index"
                        );
                    }
                }
            }
        }

        if let Some(reason) = next_reason {
            *lock(&self.reason) = reason;
        }
    }
}

/// Resolve tier from FlagsInConfig i32 field (0 / unknown → DirectFirst via normalize).
pub fn tier_from_flags_i32(raw: i32) -> ConnectionPathTier {
    ConnectionPathTier::try_from(raw)
        .map(normalize_connection_path_tier)
        .unwrap_or(ConnectionPathTier::DirectFirst)
}

/// Infer tier when flags still carry only legacy bools.
pub fn tier_from_flags(flags: &easytier_proto::common::FlagsInConfig) -> ConnectionPathTier {
    let stored = ConnectionPathTier::try_from(flags.connection_path_tier)
        .unwrap_or(ConnectionPathTier::Unspecified);
    if stored != ConnectionPathTier::Unspecified {
        normalize_connection_path_tier(stored)
    } else {
        infer_connection_path_tier(flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        s.parse().unwrap()
    }

    #[test]
    fn preserves_order_and_eligible_prefix() {
        let c = FallbackController::new();
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        c.add_url(url("wss://c:443/et"));
        assert_eq!(c.ordered_urls().len(), 3);

        let eligible = c.eligible_urls(ConnectionPathTier::DirectFirst);
        assert_eq!(eligible.len(), 1);
        assert!(eligible.contains(&url("tcp://a:1")));
    }

    #[test]
    fn prefer_relay_dials_all() {
        let c = FallbackController::new();
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        let eligible = c.eligible_urls(ConnectionPathTier::PreferRelay);
        assert_eq!(eligible.len(), 2);
    }

    #[test]
    fn pinned_url_stays_eligible_under_direct_first() {
        let c = FallbackController::new();
        c.add_url(url("tcp://a:1"));
        c.pin_url(url("tcp://b:1"));
        let eligible = c.eligible_urls(ConnectionPathTier::DirectFirst);
        assert_eq!(eligible.len(), 2);
        assert!(eligible.contains(&url("tcp://a:1")));
        assert!(eligible.contains(&url("tcp://b:1")));
        // Recover shrink must not drop the pin.
        assert_eq!(c.current_index(), 0);
        let mut alive = HashSet::new();
        alive.insert(url("tcp://a:1"));
        for _ in 0..5 {
            c.on_reconnect_tick(&alive, ConnectionPathTier::DirectFirst);
        }
        assert_eq!(c.current_index(), 0);
        assert!(
            c.eligible_urls(ConnectionPathTier::DirectFirst)
                .contains(&url("tcp://b:1"))
        );
    }

    #[test]
    fn remove_unpins() {
        let c = FallbackController::new();
        c.pin_url(url("tcp://a:1"));
        c.pin_url(url("tcp://b:1"));
        c.remove_url(&url("tcp://b:1"));
        let eligible = c.eligible_urls(ConnectionPathTier::DirectFirst);
        assert_eq!(eligible.len(), 1);
        assert!(eligible.contains(&url("tcp://a:1")));
    }

    #[test]
    fn escalates_after_hysteresis_when_all_dead() {
        let c = FallbackController::with_hysteresis(2, 5);
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        let empty = HashSet::new();
        c.on_reconnect_tick(&empty, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 0);
        c.on_reconnect_tick(&empty, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 1);
        assert_eq!(c.reason(), FallbackReason::AllActiveDead);
        assert_eq!(c.eligible_urls(ConnectionPathTier::DirectFirst).len(), 2);
    }

    #[test]
    fn recover_shrinks_index() {
        let c = FallbackController::with_hysteresis(1, 2);
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        let empty = HashSet::new();
        c.on_reconnect_tick(&empty, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 1);

        let mut alive = HashSet::new();
        alive.insert(url("tcp://a:1"));
        c.on_reconnect_tick(&alive, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 1);
        c.on_reconnect_tick(&alive, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 0);
    }

    #[test]
    fn remove_url_before_index_shifts_down() {
        let c = FallbackController::with_hysteresis(1, 5);
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        c.add_url(url("tcp://c:1"));
        let empty = HashSet::new();
        c.on_reconnect_tick(&empty, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 1); // b
        c.on_reconnect_tick(&empty, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 2); // c

        c.remove_url(&url("tcp://a:1"));
        // Was pointing at c (index 2); after removing a, c is at index 1.
        assert_eq!(c.current_index(), 1);
        assert_eq!(c.ordered_urls()[1], url("tcp://c:1"));
    }

    #[test]
    fn remove_url_before_active_keeps_same_url() {
        let c = FallbackController::with_hysteresis(1, 5);
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        c.add_url(url("tcp://c:1"));
        let empty = HashSet::new();
        c.on_reconnect_tick(&empty, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 1); // b

        c.remove_url(&url("tcp://a:1"));
        // Must keep pointing at b (now index 0), not silently jump to c.
        assert_eq!(c.current_index(), 0);
        assert_eq!(c.ordered_urls()[0], url("tcp://b:1"));
    }

    #[test]
    fn remove_active_url_stays_at_slot() {
        let c = FallbackController::with_hysteresis(1, 5);
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        c.add_url(url("tcp://c:1"));
        let empty = HashSet::new();
        c.on_reconnect_tick(&empty, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 1); // b

        c.remove_url(&url("tcp://b:1"));
        assert_eq!(c.current_index(), 1);
        assert_eq!(c.ordered_urls()[1], url("tcp://c:1"));
    }

    #[test]
    fn remove_active_at_index_zero_stays_zero() {
        let c = FallbackController::new();
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        assert_eq!(c.current_index(), 0);
        c.remove_url(&url("tcp://a:1"));
        assert_eq!(c.current_index(), 0);
        assert_eq!(c.ordered_urls()[0], url("tcp://b:1"));
    }

    #[test]
    fn leave_dial_all_tier_shrinks_index_immediately() {
        let c = FallbackController::with_hysteresis(3, 5);
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        c.add_url(url("tcp://c:1"));

        let empty = HashSet::new();
        c.on_reconnect_tick(&empty, ConnectionPathTier::PreferRelay);
        assert_eq!(c.current_index(), 2);
        assert_eq!(c.reason(), FallbackReason::TierDialAll);
        assert_eq!(c.eligible_urls(ConnectionPathTier::PreferRelay).len(), 3);

        // Only the first URL is alive; leaving PreferRelay must shrink to it
        // on the same tick (not wait for recover_after).
        let mut alive = HashSet::new();
        alive.insert(url("tcp://a:1"));
        c.on_reconnect_tick(&alive, ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 0);
        assert_eq!(c.reason(), FallbackReason::Initial);
        assert_eq!(c.eligible_urls(ConnectionPathTier::DirectFirst).len(), 1);
        assert!(
            c.eligible_urls(ConnectionPathTier::DirectFirst)
                .contains(&url("tcp://a:1"))
        );
    }

    #[test]
    fn leave_dial_all_with_no_alive_shrinks_to_zero() {
        let c = FallbackController::with_hysteresis(3, 5);
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));
        c.on_reconnect_tick(&HashSet::new(), ConnectionPathTier::RelayOnly);
        assert_eq!(c.current_index(), 1);

        c.on_reconnect_tick(&HashSet::new(), ConnectionPathTier::DirectFirst);
        assert_eq!(c.current_index(), 0);
        assert_eq!(c.reason(), FallbackReason::Initial);
        assert_eq!(c.eligible_urls(ConnectionPathTier::DirectFirst).len(), 1);
    }

    /// Regression: status readers must not deadlock against reconnect ticks
    /// (previously nested ordered→reason vs reason→ordered).
    #[test]
    fn status_and_tick_do_not_deadlock() {
        use std::sync::Arc;
        use std::thread;

        let c = Arc::new(FallbackController::with_hysteresis(1, 2));
        c.add_url(url("tcp://a:1"));
        c.add_url(url("tcp://b:1"));

        let tick_ctrl = c.clone();
        let ticker = thread::spawn(move || {
            let empty = HashSet::new();
            let mut alive = HashSet::new();
            alive.insert(url("tcp://a:1"));
            for i in 0..200 {
                if i % 3 == 0 {
                    tick_ctrl.on_reconnect_tick(&empty, ConnectionPathTier::DirectFirst);
                } else if i % 3 == 1 {
                    tick_ctrl.on_reconnect_tick(&alive, ConnectionPathTier::DirectFirst);
                } else {
                    tick_ctrl.on_reconnect_tick(&empty, ConnectionPathTier::PreferRelay);
                }
            }
        });

        let status_ctrl = c.clone();
        let reader = thread::spawn(move || {
            for _ in 0..200 {
                let _ = status_ctrl.status(ConnectionPathTier::DirectFirst);
                let _ = status_ctrl.eligible_urls(ConnectionPathTier::DirectFirst);
                let _ = status_ctrl.reason();
            }
        });

        ticker.join().expect("ticker panicked");
        reader.join().expect("status reader panicked");
        // If locks nest the wrong way, the test hangs until the harness times out.
        let _ = c.status(ConnectionPathTier::DirectFirst);
    }
}
