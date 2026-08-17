//! Per-mutator shadow-stack roots + process registry (BUILD §7.7 phase C).
//!
//! Each OS thread keeps a TLS root vector behind a **per-mutator** [`Mutex`].
//! Hot `push` / `pop` take only that local lock (not the process heap Mutex).
//! GC walks roots while holding the heap lock, then briefly locks each
//! mutator's roots mutex (order: **heap → roots**). Shade of newly pushed
//! slots during incremental full mark uses [`crate::gc::full_marking_fast`].

use std::cell::Cell;
use std::sync::{Mutex, OnceLock};

use crate::heap::with_heap;

thread_local! {
    pub(crate) static ROOTS: Mutex<Vec<*mut *mut u8>> = const { Mutex::new(Vec::new()) };
    static REGISTRATION: Registration = Registration::new();
}

struct MutatorEntry {
    /// Points at this thread's [`ROOTS`] `Mutex`.
    roots: *const Mutex<Vec<*mut *mut u8>>,
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
}

impl Registration {
    fn new() -> Self {
        // Touch ROOTS so the TLS slot exists, then publish under the heap lock
        // so GC's registry snapshot cannot miss this mutator.
        let roots_ptr = ROOTS.with(|r| r as *const Mutex<Vec<*mut *mut u8>>);
        with_heap(|_| {
            registry()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(MutatorEntry { roots: roots_ptr });
        });
        Self {
            active: Cell::new(true),
            roots: roots_ptr,
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
        // Heap then registry: same order as GC so a dying mutator is never walked.
        with_heap(|_| {
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
