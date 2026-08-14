//! Per-mutator shadow-stack roots + process registry (BUILD §7.7 phase C).
//!
//! Each OS thread keeps a TLS root vector. The process registry holds raw
//! pointers to those vectors so GC (holding the heap lock) can mark every
//! mutator. `lumia_root_push` / `pop` take the heap lock so root mutation
//! cannot race with mark.

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, OnceLock};

use crate::heap::with_heap;

thread_local! {
    pub(crate) static ROOTS: RefCell<Vec<*mut *mut u8>> = const { RefCell::new(Vec::new()) };
    static REGISTRATION: Registration = Registration::new();
}

struct MutatorEntry {
    /// Points at this thread's [`ROOTS`] `RefCell`.
    roots: *const RefCell<Vec<*mut *mut u8>>,
}

// Entries are only used while the owning thread is alive (Drop unregisters)
// and while the heap lock serializes mutators with GC.
unsafe impl Send for MutatorEntry {}
unsafe impl Sync for MutatorEntry {}

static REGISTRY: OnceLock<Mutex<Vec<MutatorEntry>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<MutatorEntry>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

struct Registration {
    active: Cell<bool>,
    roots: *const RefCell<Vec<*mut *mut u8>>,
}

impl Registration {
    fn new() -> Self {
        // Touch ROOTS so the TLS slot exists, then publish under the heap lock
        // so GC's registry snapshot cannot miss this mutator.
        let roots_ptr = ROOTS.with(|r| r as *const RefCell<Vec<*mut *mut u8>>);
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
        // Heap then registry: same order as push/pop so GC never sees a dying mutator.
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

/// Visit every registered mutator's root slots.
///
/// Caller must hold the process heap lock (`with_heap`) so other threads
/// cannot be inside [`push_root`] / [`pop_root`].
pub(crate) fn for_each_mutator_root(mut f: impl FnMut(*mut *mut u8)) {
    let entries: Vec<*const RefCell<Vec<*mut *mut u8>>> = registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|e| e.roots)
        .collect();
    for ptr in entries {
        // Safety: entry stays registered until the owning thread drops
        // `Registration`; that Drop takes the registry lock, and we hold
        // the heap lock so the owner cannot be mid push/pop.
        let roots = unsafe { &*ptr };
        // Heap lock held by caller ⇒ no concurrent push/pop on these RefCells.
        for &slot in roots.borrow().iter() {
            f(slot);
        }
    }
}

#[inline]
pub(crate) fn push_root(slot: *mut *mut u8) {
    ensure_mutator_registered();
    with_heap(|_| {
        ROOTS.with(|r| r.borrow_mut().push(slot));
    });
}

#[inline]
pub(crate) fn pop_root() {
    ensure_mutator_registered();
    with_heap(|_| {
        ROOTS.with(|r| {
            let _ = r.borrow_mut().pop();
        });
    });
}

/// Replace the current thread's root stack (fiber park / host swap).
pub(crate) fn take_local_roots() -> Vec<*mut *mut u8> {
    ensure_mutator_registered();
    with_heap(|_| ROOTS.with(|r| std::mem::take(&mut *r.borrow_mut())))
}

pub(crate) fn set_local_roots(roots: Vec<*mut *mut u8>) {
    ensure_mutator_registered();
    with_heap(|_| {
        ROOTS.with(|r| *r.borrow_mut() = roots);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{is_heap_payload, TYPE_BYTES};
    use crate::gc::{lumia_alloc, lumia_gc_collect, lumia_root_pop, lumia_root_push};
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn gc_sees_other_thread_roots() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let barrier = Arc::new(Barrier::new(2));
        let barrier_main = Arc::clone(&barrier);
        let kept = Arc::new(AtomicUsize::new(0));
        let kept_t = Arc::clone(&kept);

        let child = thread::spawn(move || {
            ensure_mutator_registered();
            let mut slot = lumia_alloc(16, TYPE_BYTES);
            assert!(!slot.is_null());
            lumia_root_push(&mut slot as *mut *mut u8);
            kept_t.store(slot as usize, Ordering::SeqCst);
            barrier.wait();
            // Parent runs GC while we stay rooted.
            barrier.wait();
            assert!(is_heap_payload(slot));
            lumia_root_pop();
        });

        barrier_main.wait();
        lumia_gc_collect();
        let p = kept.load(Ordering::SeqCst) as *mut u8;
        assert!(is_heap_payload(p), "child root must survive parent GC");
        barrier_main.wait();
        child.join().unwrap();
    }
}
