//! Process-global lazy init and atomic Ordering contracts.
//!
//! Lumia RT historically mixed three lazy patterns and two “cache once”
//! styles. New globals should pick from this table and document Ordering here
//! (Todo: RT 全局初始化三轨).
//!
//! # Lazy patterns
//!
//! | Pattern | Used for | Notes |
//! |---------|----------|-------|
//! | [`std::sync::OnceLock`] | [`crate::heap::PROCESS_HEAP`], [`crate::task::sched_core`] `SCHED`, [`crate::memo`] `MEMO_REGISTRY`, [`crate::mutator`] `REGISTRY`, [`set_before_trap`] hook, [`par_worker_count`], [`fiber_stack_bytes`] | Preferred for process singletons; ownership stays in home modules |
//! | [`std::sync::Once`] | sched pool spawn, trap hook install (task) | One-shot side effects |
//! | Bare [`std::sync::Mutex::new`] | [`crate::adt_show`] `ADT_SHOW`, [`crate::dict`] `DICTS`, TLS memo tables, [`crate::task::sched_env`] `SCHED_ENV` (pool sizes; tests may clear) | Always constructed; lock for content / reloadable cache |
//!
//! # “Cache once” / feature probes
//!
//! | Style | Used for | Ordering |
//! |-------|----------|----------|
//! | `OnceLock<T>` | [`par_worker_count`], [`fiber_stack_bytes`], host parallelism | Init once; readers see published value |
//! | `AtomicU8` + `Relaxed` | [`simd_f64_available`] CPU probe (`0` unknown / `1` no / `2` yes) | Probe is idempotent; `Relaxed` is enough |
//! | `AtomicBool` + `Release`/`Acquire` | GC soft pressure / full-mark mirrors in [`crate::gc::pressure`]; [`note_task_runtime_used`] / [`task_runtime_used_latched`] (Task/Channel latch) | Updated under heap lock or one-shot latch; hot paths load without Mutex |
//! | `Mutex<Option<(usize, usize)>>` | `SCHED_ENV` in `task/sched_env.rs` (`LUMIA_SCHED_WORKERS` / `LUMIA_SCHED_IO`) | Write-once in production; tests call `reload_sched_env_for_test` |
//! | `AtomicU64` + `Relaxed` | [`note_par_task_demotion`] / [`par_task_demotions`] (list-par demotion counter) | Diagnostics / tests only |
//! | Env parse (no cache) | [`parse_gc_incremental_env`] / `LUMIA_GC_INCREMENTAL` via [`crate::gc::limits`] | Read each collect decision; unknown tokens → keep heap default |
//! | TLS (not process-global) | [`crate::common`] `PAR_WORKER` / `CALL_STACK`; [`crate::task::scheduler`] `CURRENT_FIBER` / `SCOPE_STACK*` / `FIBER_STACK_FREELIST` / `SCOPE_KIND_CACHE`; [`crate::mutator`] `ROOTS` / `LAB`; [`crate::memo`] `MEMO_TF` / `MEMO_IDX` / `MEMO_REGISTRATION` | Thread-local; documented here so new TLS does not look like a missing OnceLock |
//! | Test-only process `Mutex<()>` | [`crate::task::scheduler`] `SCHED_UNIT_TEST_LOCK` | Serializes sched unit tests; not a production lock |
//!
//! # Lock order
//!
//! Mutex nesting is documented on the crate root (`heap → sched → …`). Do **not**
//! invent new process mutexes that nest under heap/sched without updating that
//! table and `scripts/check_rt_lock_order.sh`.
//!
//! Policy predicates that cross Task/GC/list-par live in
//! [`crate::concurrency_policy`].

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;

/// Best-effort cleanup before fatal abort (e.g. cancel sibling tasks).
/// Process-global so pool OS threads share the same trap hook.
static BEFORE_TRAP: OnceLock<fn()> = OnceLock::new();

/// Install a hook invoked at the start of [`crate::common::trap_abort`] (once per process).
pub(crate) fn set_before_trap(hook: fn()) {
    let _ = BEFORE_TRAP.set(hook);
}

#[inline]
pub(crate) fn before_trap_hook() -> Option<fn()> {
    BEFORE_TRAP.get().copied()
}

/// Cached `available_parallelism` for list-par workers (OnceLock contract).
pub(crate) fn par_worker_count() -> usize {
    static CACHED: OnceLock<usize> = OnceLock::new();
    *CACHED.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
    })
}

/// Fiber coroutine stack size in bytes (`LUMIA_FIBER_STACK_KB`, default 64KiB, min 16KiB).
pub(crate) fn fiber_stack_bytes() -> usize {
    static CACHED: OnceLock<usize> = OnceLock::new();
    *CACHED.get_or_init(|| {
        let default = 64 * 1024;
        match std::env::var("LUMIA_FIBER_STACK_KB") {
            Ok(v) => match v.trim().parse::<usize>() {
                Ok(kb) => kb.saturating_mul(1024).max(16 * 1024),
                Err(_) => {
                    eprintln!(
                        "lumia: warning: LUMIA_FIBER_STACK_KB={v:?} ignored \
                         (expected a positive integer KiB); keeping default {default}"
                    );
                    default
                }
            },
            Err(_) => default,
        }
    })
}

/// Whether AVX2+FMA f64 kernels may run (`AtomicU8` probe contract).
#[inline]
pub(crate) fn simd_f64_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        static CACHED: AtomicU8 = AtomicU8::new(0); // 0 unknown, 1 no, 2 yes
        let v = CACHED.load(Ordering::Relaxed);
        if v != 0 {
            return v == 2;
        }
        let yes = is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma");
        CACHED.store(if yes { 2 } else { 1 }, Ordering::Relaxed);
        yes
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Latched when Task/Channel APIs run (`AtomicBool` Release/Acquire).
///
/// Avoids fiber-table scans on every `par_map` in programs that never use the
/// scheduler. Owned here so Ordering contracts stay in one table.
static TASK_RUNTIME_USED: AtomicBool = AtomicBool::new(false);

#[inline]
pub(crate) fn note_task_runtime_used() {
    TASK_RUNTIME_USED.store(true, Ordering::Release);
}

#[inline]
pub(crate) fn task_runtime_used_latched() -> bool {
    TASK_RUNTIME_USED.load(Ordering::Acquire)
}

/// How many times `list_par_*` fell back to sequential under an active Task/Channel
/// runtime (`AtomicU64` Relaxed — diagnostics / tests only).
static PAR_TASK_DEMOTIONS: AtomicU64 = AtomicU64::new(0);

#[inline]
pub(crate) fn note_par_task_demotion() {
    PAR_TASK_DEMOTIONS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
#[allow(dead_code)] // tests / diagnostics after compile-time demote
pub(crate) fn par_task_demotions() -> u64 {
    PAR_TASK_DEMOTIONS.load(Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn reset_par_task_demotions() {
    PAR_TASK_DEMOTIONS.store(0, Ordering::Relaxed);
}

/// Marker that this module is the Ordering / lazy-init contract home.
#[cfg(test)]
pub(crate) fn contracts_documented() -> bool {
    true
}

/// Parse `LUMIA_GC_INCREMENTAL`. Unknown tokens are `None` (caller keeps default).
///
/// Lived next to GC limits historically; registered here so new env knobs have a
/// single contract table (Todo: RT 全局初始化三轨).
pub(crate) fn parse_gc_incremental_env(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "off" | "stw" | "no" => Some(false),
        "1" | "true" | "on" | "yes" | "incremental" => Some(true),
        "" => None,
        _ => None,
    }
}

#[cfg(test)]
#[path = "globals_tests.rs"]
mod tests;
