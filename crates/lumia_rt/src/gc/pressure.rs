//! Lock-free GC pressure / full-mark mirrors for hot paths.
//!
//! These atomics mirror [`Heap`] fields so alloc / channel / memo can skip the
//! heap Mutex when soft GC is idle. Updates happen only while holding the heap
//! lock (or immediately after releasing it with published values).
//!
//! Ordering: `Release` on store / `Acquire` on load (see crate [`globals`](crate::globals)).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::heap::Heap;

/// Soft GC pressure: young/old over limit or full mark in flight.
/// Alloc skips soft-GC work when this is false; under pressure it uses a single
/// heap lock that either inserts or signals collect (no separate peek lock).
static ALLOC_PRESSURE_FAST: AtomicBool = AtomicBool::new(false);

/// Mirrors [`Heap::full_marking`] for channel/join / Dijkstra shade hot paths.
static FULL_MARKING_FAST: AtomicBool = AtomicBool::new(false);

#[inline]
pub(crate) fn full_marking_fast() -> bool {
    FULL_MARKING_FAST.load(Ordering::Acquire)
}

#[inline]
pub(crate) fn set_full_marking_fast(v: bool) {
    FULL_MARKING_FAST.store(v, Ordering::Release);
    if v {
        // Full mark ⇒ always consider collect on the next alloc peek.
        ALLOC_PRESSURE_FAST.store(true, Ordering::Release);
    }
    // Clearing: callers refresh via [`refresh_from_heap`] after updating
    // `full_marking` / live bytes (do not leave pressure stuck true).
}

#[inline]
pub(crate) fn alloc_pressure_fast() -> bool {
    ALLOC_PRESSURE_FAST.load(Ordering::Acquire)
}

/// Update [`alloc_pressure_fast`] from current bytes / full-mark flag.
#[inline]
pub(crate) fn refresh_from_heap(h: &Heap) {
    let pressure =
        h.full_marking || h.bytes_young >= h.young_limit || h.bytes_old >= h.old_limit;
    ALLOC_PRESSURE_FAST.store(pressure, Ordering::Release);
}

#[cfg(test)]
#[path = "pressure_tests.rs"]
mod tests;
