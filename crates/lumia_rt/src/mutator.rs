//! Per-mutator shadow-stack roots + TLS nursery LAB + process registry.
//!
//! Each OS thread keeps:
//! - a TLS root vector behind a **per-mutator** [`Mutex`]
//! - a TLS **LAB** ([`LabState`]) for bump allocation without the heap Mutex
//!
//! Hot `push` / `pop` take only the local roots lock. TLS LAB bump takes only
//! the local LAB lock. GC walks roots / flushes LABs while holding the heap
//! lock, then briefly locks each mutator mutex (order: **heap → roots/lab**).
//! Shade of newly pushed slots during incremental full mark uses
//! [`crate::gc::full_marking_fast`].

use std::cell::Cell;
use std::sync::{Mutex, OnceLock};

use crate::common::ObjectHeader;
use crate::heap::{with_heap, Heap};

thread_local! {
    pub(crate) static ROOTS: Mutex<Vec<*mut *mut u8>> = const { Mutex::new(Vec::new()) };
    static LAB: Mutex<LabState> = Mutex::new(LabState::empty());
    static REGISTRATION: Registration = Registration::new();
}

/// TLS local allocation buffer carved from the process nursery.
pub(crate) struct LabState {
    /// Claimed `[base, end)` byte range inside the process nursery; `base == 0` ⇒ none.
    pub(crate) base: usize,
    pub(crate) end: usize,
    /// Next bump address (`base..end`).
    pub(crate) cursor: usize,
    /// Payload bytes in `pending` (soft-GC budget accounting).
    pub(crate) pending_bytes: usize,
    /// Max pending payload bytes before forcing a flush (snapshotted at claim).
    pub(crate) pending_budget: usize,
    /// Headers allocated in this LAB but not yet published into `h.young`.
    pub(crate) pending: Vec<*mut ObjectHeader>,
}

impl LabState {
    pub(crate) const fn empty() -> Self {
        Self {
            base: 0,
            end: 0,
            cursor: 0,
            pending_bytes: 0,
            pending_budget: 0,
            pending: Vec::new(),
        }
    }

    pub(crate) fn clear_claim(&mut self) {
        self.base = 0;
        self.end = 0;
        self.cursor = 0;
        self.pending_bytes = 0;
        self.pending_budget = 0;
        self.pending.clear();
    }
}

struct MutatorEntry {
    /// Points at this thread's [`ROOTS`] `Mutex`.
    roots: *const Mutex<Vec<*mut *mut u8>>,
    /// Points at this thread's [`LAB`] `Mutex`.
    lab: *const Mutex<LabState>,
}

// Entries are only used while the owning thread is alive (Drop unregisters).
// Cross-thread access locks the pointed-to Mutex under the heap lock.
unsafe impl Send for MutatorEntry {}
unsafe impl Sync for MutatorEntry {}

static REGISTRY: OnceLock<Mutex<Vec<MutatorEntry>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<MutatorEntry>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

struct Registration {
    active: Cell<bool>,
    roots: *const Mutex<Vec<*mut *mut u8>>,
    lab: *const Mutex<LabState>,
}

impl Registration {
    fn new() -> Self {
        // Touch ROOTS/LAB so TLS slots exist, then publish under the heap lock
        // so GC's registry snapshot cannot miss this mutator.
        let roots_ptr = ROOTS.with(|r| r as *const Mutex<Vec<*mut *mut u8>>);
        let lab_ptr = LAB.with(|l| l as *const Mutex<LabState>);
        with_heap(|_| {
            registry()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(MutatorEntry {
                    roots: roots_ptr,
                    lab: lab_ptr,
                });
        });
        Self {
            active: Cell::new(true),
            roots: roots_ptr,
            lab: lab_ptr,
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if !self.active.get() {
            return;
        }
        self.active.set(false);
        let roots_ptr = self.roots;
        let lab_ptr = self.lab;
        // Heap then registry: same order as GC so a dying mutator is never walked.
        with_heap(|h| {
            // SAFETY: lab_ptr is this thread's TLS LAB until Drop finishes.
            let lab = unsafe { &*lab_ptr };
            crate::gc::flush_lab_into_heap(h, lab);
            if let Ok(mut reg) = registry().lock() {
                reg.retain(|e| e.roots != roots_ptr);
            }
        });
    }
}

/// Ensure this thread is in the mutator registry (lazy TLS init).
#[inline]
pub(crate) fn ensure_mutator_registered() {
    REGISTRATION.with(|_| {});
}

fn lock_roots(m: &Mutex<Vec<*mut *mut u8>>) -> std::sync::MutexGuard<'_, Vec<*mut *mut u8>> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub(crate) fn lock_lab(m: &Mutex<LabState>) -> std::sync::MutexGuard<'_, LabState> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Visit every registered mutator's root slots.
///
/// Caller must hold the process heap lock (`with_heap`) for registry stability
/// and heap metadata. Each mutator's vector is then locked separately
/// (lock order: heap → that mutator's roots).
pub(crate) fn for_each_mutator_root(mut f: impl FnMut(*mut *mut u8)) {
    let entries: Vec<*const Mutex<Vec<*mut *mut u8>>> = registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|e| e.roots)
        .collect();
    for ptr in entries {
        // Safety: entry stays registered until the owning thread drops
        // `Registration`; that Drop takes the heap + registry locks, and we
        // already hold the heap lock.
        let roots = unsafe { &*ptr };
        for &slot in lock_roots(roots).iter() {
            f(slot);
        }
    }
}

/// Snapshot registered LAB mutex pointers (caller holds heap).
pub(crate) fn lab_mutexes() -> Vec<*const Mutex<LabState>> {
    registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|e| e.lab)
        .collect()
}

/// This thread's LAB mutex (for refill / bump).
#[inline]
pub(crate) fn local_lab() -> &'static Mutex<LabState> {
    ensure_mutator_registered();
    // SAFETY: TLS LAB lives for the thread; we only use it while registered.
    LAB.with(|l| {
        // Extend lifetime: LAB is thread-local for this thread's lifetime.
        // Callers must not stash across threads.
        unsafe { &*(l as *const Mutex<LabState>) }
    })
}

/// Publish all mutators' TLS LAB pending objects into `h.young` and drop LAB claims.
///
/// Caller holds the heap lock. Must run before mark / evacuate so `h.young` is
/// complete and no thread keeps bumping into a slab about to rewind.
pub(crate) fn flush_all_labs(h: &mut Heap) {
    for ptr in lab_mutexes() {
        // SAFETY: same registration lifetime as `for_each_mutator_root`.
        let lab = unsafe { &*ptr };
        crate::gc::flush_lab_into_heap(h, lab);
    }
}

/// Invalidate LAB claims after nursery rewind (pending must already be flushed).
pub(crate) fn invalidate_all_labs() {
    for ptr in lab_mutexes() {
        let lab = unsafe { &*ptr };
        lock_lab(lab).clear_claim();
    }
}

/// True when this thread currently holds an unflushed TLS LAB object (tests).
#[cfg(test)]
pub(crate) fn tls_lab_active_for_test() -> bool {
    ensure_mutator_registered();
    LAB.with(|lab| {
        let g = lock_lab(lab);
        g.base != 0 && !g.pending.is_empty()
    })
}

#[inline]
pub(crate) fn push_root(slot: *mut *mut u8) {
    ensure_mutator_registered();
    ROOTS.with(|r| lock_roots(r).push(slot));
}

#[inline]
pub(crate) fn pop_root() {
    ensure_mutator_registered();
    ROOTS.with(|r| {
        let _ = lock_roots(r).pop();
    });
}

/// Replace the current thread's root stack (fiber park / host swap).
pub(crate) fn take_local_roots() -> Vec<*mut *mut u8> {
    ensure_mutator_registered();
    ROOTS.with(|r| std::mem::take(&mut *lock_roots(r)))
}

pub(crate) fn set_local_roots(roots: Vec<*mut *mut u8>) {
    ensure_mutator_registered();
    ROOTS.with(|r| *lock_roots(r) = roots);
}

#[cfg(test)]
#[path = "mutator_tests.rs"]
mod tests;
