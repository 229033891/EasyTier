use easytier::core;

#[cfg(all(feature = "mimalloc", not(feature = "jemalloc")))]
use mimalloc::MiMalloc;

#[cfg(all(feature = "mimalloc", not(feature = "jemalloc")))]
#[global_allocator]
static GLOBAL_MIMALLOC: MiMalloc = MiMalloc;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[cfg(feature = "jemalloc-prof")]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "malloc_conf")]
pub static malloc_conf: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:19,dirty_decay_ms:1000,muzzy_decay_ms:1000,retain:false\0";

#[cfg(not(feature = "jemalloc-prof"))]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "malloc_conf")]
pub static malloc_conf: &[u8] = b"dirty_decay_ms:1000,muzzy_decay_ms:1000,retain:false\0";

rust_i18n::i18n!("locales", fallback = "en");

// Multi-thread runtime (same as Windows service `Runtime::new()`): a single
// current_thread worker serializes every peer's dataplane and magnifies HOL.
// No extra tokio feature is needed here: this bin only builds under
// `required-features = ["management"]`, which already enables `tokio/full`.
#[tokio::main]
async fn main() -> std::process::ExitCode {
    core::main().await
}
