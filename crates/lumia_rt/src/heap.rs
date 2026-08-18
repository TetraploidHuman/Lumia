//! Process-shared heap metadata (BUILD §7.7 phase B/C).
//!
//! Phase C registers per-thread root sets so GC can mark every mutator while
//! holding this heap lock. Independent `cargo test` cases still share one
//! process heap and must run with `RUST_TEST_THREADS=1` (see `scripts/check.sh`).

use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use std::cell::Cell;
use std::ptr;
use std::sync::{Mutex, OnceLock};

use crate::common::ObjectHeader;
use crate::reentrant::with_mutex_reentrant;

/// Soft young / old live-byte thresholds (defaults match historical TLS values).
/// Small young allocs bump from [`Heap::nursery`] (dual-published to `live_set` +
/// `heap_set`). Large allocs tenure via [`crate::gc::alloc_ffi::insert_old`].
pub(crate) const DEFAULT_YOUNG_LIMIT: usize = 1024 * 1024;
pub(crate) const DEFAULT_HEAP_LIMIT: usize = 8 * 1024 * 1024;

/// Generational heap + incremental-mark control (one process instance).
pub(crate) struct Heap {
    pub young: Vec<*mut ObjectHeader>,
    pub old: Vec<*mut ObjectHeader>,
    pub old_set: FxHashSet<*mut ObjectHeader>,
    pub remembered: FxHashSet<*mut ObjectHeader>,
    pub heap_set: FxHashSet<*mut ObjectHeader>,
    pub perm: Vec<*mut u8>,
    pub bytes_young: usize,
    pub bytes_old: usize,
    pub young_limit: usize,
    pub old_limit: usize,
    /// Immortal empty `List` singleton payload (or null until first use).
    pub empty_list: *mut u8,
    /// Immortal empty `Map` singleton (`mapOf()` / remove-to-empty); null until first use.
    pub empty_map: *mut u8,
    /// Immortal empty `Set` singleton (`setOf()` / remove-to-empty); null until first use.
    pub empty_set: *mut u8,
    /// Immortal `None` Option ADTs keyed by constructor tag (map_get miss path).
    pub option_none: FxHashMap<u64, *mut u8>,
    /// Process bump nursery for small young objects (see [`crate::gc::nursery`]).
    pub nursery: crate::gc::nursery::Nursery,
    /// When true, mark helpers only follow young payloads.
    pub mark_minor: bool,
    /// Incremental full-heap mark in progress.
    pub full_marking: bool,
    pub mark_work: Vec<*mut ObjectHeader>,
    pub mark_quantum: usize,
    /// Default for incremental full mark (overridable via env / tests).
    pub incremental_full: bool,
    /// Process-wide soft-GC inhibit (multi-mutator safe).
    pub gc_inhibit: u32,
}

// Heap holds raw object pointers; exclusive access is via `with_heap` / Mutex.
unsafe impl Send for Heap {}
unsafe impl Sync for Heap {}

impl Heap {
    pub(crate) fn new() -> Self {
        Self {
            young: Vec::new(),
            old: Vec::new(),
            old_set: FxHashSet::default(),
            remembered: FxHashSet::default(),
            heap_set: FxHashSet::default(),
            perm: Vec::new(),
            bytes_young: 0,
            bytes_old: 0,
            young_limit: DEFAULT_YOUNG_LIMIT,
            old_limit: DEFAULT_HEAP_LIMIT,
            empty_list: ptr::null_mut(),
            empty_map: ptr::null_mut(),
            empty_set: ptr::null_mut(),
            option_none: FxHashMap::default(),
            nursery: crate::gc::nursery::Nursery::new(),
            mark_minor: false,
            full_marking: false,
            mark_work: Vec::new(),
            mark_quantum: 256,
            incremental_full: true,
            gc_inhibit: 0,
        }
    }

    #[inline]
    pub(crate) fn contains_header(&self, h: *mut ObjectHeader) -> bool {
        // Nursery bump objects live only in `live_set` (not dual-written to `heap_set`).
        if self.nursery.contains_header(h) {
            return self.nursery.is_live(h);
        }
        self.heap_set.contains(&h)
    }

    #[inline]
    pub(crate) fn is_old_header(&self, h: *mut ObjectHeader) -> bool {
        self.old_set.contains(&h)
    }

    /// Update GC soft-pressure atomics from current bytes / full-mark flag.
    /// Implementation lives in [`crate::gc::pressure`] (GC owns the mirrors).
    #[inline]
    pub(crate) fn refresh_alloc_pressure_fast(&self) {
        crate::gc::refresh_alloc_pressure_fast(self);
    }
}

static PROCESS_HEAP: OnceLock<Mutex<Heap>> = OnceLock::new();

fn process_heap() -> &'static Mutex<Heap> {
    PROCESS_HEAP.get_or_init(|| {
        let mut h = Heap::new();
        // Only the process-wide nursery publishes the global range atomics.
        h.nursery.publish_range();
        Mutex::new(h)
    })
}

thread_local! {
    /// Re-entrancy: `with_heap` → alloc → `with_heap` (same thread).
    static HEAP_RECURSION: Cell<u32> = const { Cell::new(0) };
    static HEAP_REBORROW: Cell<*mut Heap> = const { Cell::new(ptr::null_mut()) };
}

/// Run `f` with exclusive access to the process heap (reentrant on the same thread).
pub(crate) fn with_heap<R>(f: impl FnOnce(&mut Heap) -> R) -> R {
    with_mutex_reentrant(process_heap(), &HEAP_RECURSION, &HEAP_REBORROW, f)
}

#[cfg(test)]
#[path = "heap_tests.rs"]
mod tests;
