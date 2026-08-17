//! Soft GC thresholds, incremental-mark env, and test knobs.
//!
//! Keeps policy out of mark/sweep orchestration (`mod.rs`) and out of the
//! allocation FFI surface (`alloc_ffi.rs`). Soft-pressure / full-mark **atomics**
//! live in [`super::pressure`] (GC-owned mirrors of live bytes under the heap lock).

use crate::globals::parse_gc_incremental_env;
use crate::heap::with_heap;

pub(super) fn incremental_full_enabled() -> bool {
    match std::env::var("LUMIA_GC_INCREMENTAL") {
        Ok(v) => match parse_gc_incremental_env(&v) {
            Some(on) => on,
            None => {
                // Typos like `flase` used to silently enable incremental.
                eprintln!(
                    "lumia: warning: LUMIA_GC_INCREMENTAL={v:?} ignored \
                     (use 0|false|off|stw or 1|true|on|yes); keeping heap default"
                );
                with_heap(|h| h.incremental_full)
            }
        },
        Err(_) => with_heap(|h| h.incremental_full),
    }
}

#[cfg(test)]
pub(crate) fn set_gc_limits_for_test(young: usize, old: usize) {
    with_heap(|heap| {
        heap.young_limit = young;
        heap.old_limit = old;
        heap.refresh_alloc_pressure_fast();
    });
}

#[cfg(test)]
pub(crate) fn gc_set_incremental_full_for_test(on: bool) {
    with_heap(|h| h.incremental_full = on);
}

#[cfg(test)]
pub(crate) fn gc_set_mark_quantum_for_test(n: usize) {
    with_heap(|h| h.mark_quantum = n.max(1));
}

#[cfg(test)]
#[path = "limits_tests.rs"]
mod incremental_env_tests;
