//! A9 startup grace for Android underlay reconnect.
//!
//! Extracted so host `cargo test` can cover arm / seed behavior without an
//! Android target. Wired from `android_vpn_watchdog` on device builds.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// After VpnService establish / TUN attach, ignore underlay "switches" that are
/// really establish echoes (`setUnderlyingNetworks` + NetworkCallback).
pub(crate) const UNDERLAY_RECONNECT_GRACE: Duration = Duration::from_secs(5);

/// Last underlay generation observed (`-1` = never seen).
static LAST_UNDERLAY_GENERATION: AtomicI64 = AtomicI64::new(-1);

/// Unix-ms deadline; while `now < deadline`, generation bumps only seed.
static UNDERLAY_GRACE_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn in_underlay_reconnect_grace() -> bool {
    now_unix_ms() < UNDERLAY_GRACE_UNTIL_MS.load(Ordering::SeqCst)
}

/// Arm (or extend) the startup grace window.
///
/// Resets the generation seed only when **entering** grace (not when extending),
/// so a `set_tun_fd` re-arm does not wipe a generation already noted by the JS
/// fast-path during the same establish window.
pub(crate) fn arm_underlay_reconnect_grace() {
    let until = now_unix_ms() + UNDERLAY_RECONNECT_GRACE.as_millis() as i64;
    let extending = in_underlay_reconnect_grace();
    UNDERLAY_GRACE_UNTIL_MS.fetch_max(until, Ordering::SeqCst);
    if !extending {
        LAST_UNDERLAY_GENERATION.store(-1, Ordering::SeqCst);
    }
    tracing::info!(
        grace_ms = UNDERLAY_RECONNECT_GRACE.as_millis() as u64,
        extending,
        "armed underlay reconnect grace"
    );
}

/// Seed / advance the generation tracker (JS fast-path already acted).
pub(crate) fn note_underlay_generation(generation: i64) {
    LAST_UNDERLAY_GENERATION.fetch_max(generation, Ordering::SeqCst);
}

/// Atomically replace the last observed generation; returns the previous value.
pub(crate) fn swap_underlay_generation(generation: i64) -> i64 {
    LAST_UNDERLAY_GENERATION.swap(generation, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that mutate process-wide atomics.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset() {
        LAST_UNDERLAY_GENERATION.store(-1, Ordering::SeqCst);
        UNDERLAY_GRACE_UNTIL_MS.store(0, Ordering::SeqCst);
    }

    fn last_generation() -> i64 {
        LAST_UNDERLAY_GENERATION.load(Ordering::SeqCst)
    }

    fn grace_until_ms() -> i64 {
        UNDERLAY_GRACE_UNTIL_MS.load(Ordering::SeqCst)
    }

    #[test]
    fn arm_enters_grace_and_resets_seed() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset();
        note_underlay_generation(7);
        assert_eq!(last_generation(), 7);

        arm_underlay_reconnect_grace();

        assert!(in_underlay_reconnect_grace());
        assert_eq!(last_generation(), -1);
        assert!(grace_until_ms() > now_unix_ms());
    }

    #[test]
    fn arm_while_in_grace_preserves_noted_generation() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset();
        arm_underlay_reconnect_grace();
        note_underlay_generation(3);
        assert_eq!(last_generation(), 3);
        let until_before = grace_until_ms();

        arm_underlay_reconnect_grace();

        assert!(in_underlay_reconnect_grace());
        assert_eq!(last_generation(), 3, "extend must not wipe JS-noted generation");
        assert!(grace_until_ms() >= until_before);
    }

    #[test]
    fn expired_grace_is_inactive() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset();
        UNDERLAY_GRACE_UNTIL_MS.store(1, Ordering::SeqCst);
        assert!(!in_underlay_reconnect_grace());
    }

    #[test]
    fn note_is_monotonic() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset();
        note_underlay_generation(2);
        note_underlay_generation(1);
        assert_eq!(last_generation(), 2);
        note_underlay_generation(5);
        assert_eq!(last_generation(), 5);
    }

    #[test]
    fn swap_returns_previous_and_updates() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset();
        note_underlay_generation(4);
        assert_eq!(swap_underlay_generation(9), 4);
        assert_eq!(last_generation(), 9);
    }

    #[test]
    fn reenter_grace_after_expiry_resets_seed() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset();
        arm_underlay_reconnect_grace();
        note_underlay_generation(2);
        UNDERLAY_GRACE_UNTIL_MS.store(1, Ordering::SeqCst);
        assert!(!in_underlay_reconnect_grace());

        arm_underlay_reconnect_grace();

        assert!(in_underlay_reconnect_grace());
        assert_eq!(last_generation(), -1);
    }
}
