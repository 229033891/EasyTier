//! Tokio time facade.
//!
//! Native builds use Tokio directly. WASI builds use the deadline-tracking
//! implementation in [`crate::wasi::time`] so an external runtime can drive
//! the guest without polling.

// Re-export the full Tokio/WASI time surface for API parity across targets.
// Some helpers (e.g. `sleep_until`) are only referenced under optional features.
#[cfg(not(any(test, target_os = "wasi")))]
#[allow(unused_imports)]
pub use tokio::time::{Duration, Instant, Interval, error, interval, sleep, sleep_until, timeout};

#[cfg(any(test, target_os = "wasi"))]
#[allow(unused_imports)]
pub use crate::wasi::time::{
    Duration, Instant, Interval, error, interval, sleep, sleep_until, timeout,
};

#[cfg(target_os = "wasi")]
pub(crate) use crate::wasi::time::{clear_domain, enter_domain, next_deadline_millis};
