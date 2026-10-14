//! CI gate: the outbound hot path must not allocate (unified-routing §12.1 /
//! §16.2 P2).
//!
//! `LocalRouteTable::lookup` runs once per packet, so an allocation there is a
//! per-packet cost. This test installs a counting global allocator, warms the
//! path up, then asserts that a long run of lookups performs **zero**
//! allocations — on the empty table (the zero-regression baseline) as well as at
//! 1K / 10K rules, for both `hit` / `miss` and the `balance` flow-hash branch.
//!
//! Only built with `test-utils`, which is what exposes the crate-internal table
//! to an integration test (same convention as `feature_profiles.rs`).

#![cfg(feature = "test-utils")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};

use easytier_core::peers::bench_support::{
    LocalRouteTable, POLICY_ROUTE_HIT_IP, empty_local_route_table, local_route_table_with_rules,
};

/// Counts allocations on the measuring thread only, and only while [`COUNTING`]
/// is enabled.
///
/// A process-global counter is flaky on shared CI runners: other threads in the
/// test binary (or the harness) can allocate a handful of times during the
/// measured window. Real per-packet allocations would show up as O(iterations)
/// on this thread — a few stray process-wide allocs must not fail the gate.
struct CountingAlloc;

/// When false, the hook is a pure passthrough (avoids counting setup / teardown).
static COUNTING: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Re-entrancy guard: TLS / hook bookkeeping must not count as a hit.
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn bump_if_counting() {
    let _ = IN_HOOK.try_with(|in_hook| {
        if in_hook.get() {
            return;
        }
        in_hook.set(true);
        if COUNTING.load(Ordering::Relaxed) {
            let _ = ALLOCATIONS.try_with(|c| c.set(c.get().saturating_add(1)));
        }
        in_hook.set(false);
    });
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump_if_counting();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump_if_counting();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump_if_counting();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

const MISS_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
const FLOW_KEY: u64 = 0xdead_beef;
const ITERATIONS: usize = 20_000;
const WARMUP: usize = 256;

fn take_thread_allocations() -> usize {
    ALLOCATIONS
        .try_with(|c| {
            let n = c.get();
            c.set(0);
            n
        })
        .unwrap_or(0)
}

/// Assert that `table.lookup` allocates nothing across `ITERATIONS` rounds.
fn assert_no_allocations(label: &str, table: &LocalRouteTable) {
    // Warm up first: ArcSwap's per-thread slot and similar lazy machinery settle
    // here rather than inside the measured window. Also touch the TLS counters
    // so their first access cannot land inside the gated window.
    for _ in 0..WARMUP {
        black_box(table.lookup(black_box(&POLICY_ROUTE_HIT_IP), None));
        black_box(table.lookup(black_box(&MISS_IP), None));
    }
    let _ = take_thread_allocations();

    COUNTING.store(true, Ordering::Relaxed);
    for _ in 0..ITERATIONS {
        black_box(table.lookup(black_box(&POLICY_ROUTE_HIT_IP), None));
        black_box(table.lookup(black_box(&MISS_IP), None));
        // `balance` reads the published candidate list — still allocation-free.
        black_box(table.lookup(black_box(&POLICY_ROUTE_HIT_IP), Some(FLOW_KEY)));
    }
    COUNTING.store(false, Ordering::Relaxed);

    let allocated = take_thread_allocations();
    assert_eq!(
        allocated, 0,
        "{label}: hot-path lookup allocated {allocated} times over {ITERATIONS} iterations",
    );
    println!("{label}: 0 allocations over {ITERATIONS} iterations");
}

#[test]
fn hot_path_lookup_does_not_allocate() {
    // Zero-regression baseline: no `[[policy_routes]]` configured.
    assert_no_allocations("empty table", &empty_local_route_table());
    assert_no_allocations("1k rules", &local_route_table_with_rules(1_000));
    assert_no_allocations("10k rules", &local_route_table_with_rules(10_000));
}
