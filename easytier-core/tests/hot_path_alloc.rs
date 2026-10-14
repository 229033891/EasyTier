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
use std::hint::black_box;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};

use easytier_core::peers::bench_support::{
    LocalRouteTable, POLICY_ROUTE_HIT_IP, empty_local_route_table, local_route_table_with_rules,
};

/// Counts every allocation made by any thread. The test body is single-threaded
/// (one `#[test]`), so the global counter is unambiguous.
struct CountingAlloc;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

const MISS_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
const FLOW_KEY: u64 = 0xdead_beef;
const ITERATIONS: usize = 20_000;
const WARMUP: usize = 256;

/// Assert that `table.lookup` allocates nothing across `ITERATIONS` rounds.
fn assert_no_allocations(label: &str, table: &LocalRouteTable) {
    // Warm up first: ArcSwap's per-thread slot and similar lazy machinery settle
    // here rather than inside the measured window.
    for _ in 0..WARMUP {
        black_box(table.lookup(black_box(&POLICY_ROUTE_HIT_IP), None));
        black_box(table.lookup(black_box(&MISS_IP), None));
    }

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for _ in 0..ITERATIONS {
        black_box(table.lookup(black_box(&POLICY_ROUTE_HIT_IP), None));
        black_box(table.lookup(black_box(&MISS_IP), None));
        // `balance` reads the published candidate list — still allocation-free.
        black_box(table.lookup(black_box(&POLICY_ROUTE_HIT_IP), Some(FLOW_KEY)));
    }
    let after = ALLOCATIONS.load(Ordering::Relaxed);

    assert_eq!(
        after - before,
        0,
        "{label}: hot-path lookup allocated {} times over {ITERATIONS} iterations",
        after - before,
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
