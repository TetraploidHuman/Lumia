//! Process-shared heap metadata (BUILD §7.7 phase B/C).
//!
//! Phase C registers per-thread root sets so GC can mark every mutator while
//! holding this heap lock. Independent `cargo test` cases still share one
//! process heap and must run with `RUST_TEST_THREADS=1` (see `scripts/check.sh`).

use rustc_hash::FxHashSet;
use std::cell::Cell;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::common::ObjectHeader;
use crate::reentrant::with_mutex_reentrant;

/// Soft young / old live-byte thresholds (defaults match historical TLS values).
/// Young objects live in `Heap.young` (generation list — not a bump nursery).
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
        self.heap_set.contains(&h)
    }

    #[inline]
    pub(crate) fn is_old_header(&self, h: *mut ObjectHeader) -> bool {
        self.old_set.contains(&h)
    }
}

static PROCESS_HEAP: OnceLock<Mutex<Heap>> = OnceLock::new();

/// Mirrors [`Heap::full_marking`] for channel/join hot paths that otherwise
/// take the heap Mutex only to read the flag. Updated under the heap lock
/// whenever `Heap::full_marking` changes (Release/Acquire).
static FULL_MARKING_FAST: AtomicBool = AtomicBool::new(false);

#[inline]
pub(crate) fn full_marking_fast() -> bool {
    FULL_MARKING_FAST.load(Ordering::Acquire)
}

#[inline]
pub(crate) fn set_full_marking_fast(v: bool) {
    FULL_MARKING_FAST.store(v, Ordering::Release);
}

fn process_heap() -> &'static Mutex<Heap> {
    PROCESS_HEAP.get_or_init(|| Mutex::new(Heap::new()))
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
mod tests {
    use super::*;

    #[test]
    fn with_heap_reentrant() {
        with_heap(|h| {
            h.bytes_young = 42;
            with_heap(|inner| {
                assert_eq!(inner.bytes_young, 42);
                inner.bytes_young = 7;
            });
            assert_eq!(h.bytes_young, 7);
            h.bytes_young = 0;
        });
    }
}
