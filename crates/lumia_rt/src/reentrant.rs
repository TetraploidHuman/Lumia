//! Shared re-entrant `Mutex` access (heap / sched lock nesting).
//!
//! Same-thread re-entry reborrows the pinned guard pointer instead of locking
//! again (would deadlock). Callers keep their own `thread_local!` depth + pin
//! cells; this helper owns the lock/reborrow protocol.

use std::cell::Cell;
use std::ptr;
use std::sync::{Mutex, MutexGuard};
use std::thread::LocalKey;

/// Run `f` with exclusive access to `mutex`, reentrant on the same thread.
///
/// # Safety protocol
/// When `depth > 0`, `pin` must hold a live `*mut T` from the outer guard on
/// this thread. Cleared when the outermost call returns.
pub(crate) fn with_mutex_reentrant<T, R>(
    mutex: &'static Mutex<T>,
    depth_key: &'static LocalKey<Cell<u32>>,
    pin_key: &'static LocalKey<Cell<*mut T>>,
    f: impl FnOnce(&mut T) -> R,
) -> R {
    depth_key.with(|depth| {
        if depth.get() > 0 {
            return pin_key.with(|p| {
                let raw = p.get();
                debug_assert!(!raw.is_null());
                // Safety: same thread still holds the MutexGuard that pinned this.
                f(unsafe { &mut *raw })
            });
        }
        let mut guard: MutexGuard<'static, T> = mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let raw = &mut *guard as *mut T;
        pin_key.with(|p| p.set(raw));
        depth.set(1);
        struct Clear<T: 'static> {
            depth_key: &'static LocalKey<Cell<u32>>,
            pin_key: &'static LocalKey<Cell<*mut T>>,
        }
        impl<T: 'static> Drop for Clear<T> {
            fn drop(&mut self) {
                self.depth_key.with(|d| d.set(0));
                self.pin_key.with(|p| p.set(ptr::null_mut()));
            }
        }
        let _clear = Clear { depth_key, pin_key };
        f(&mut *guard)
    })
}
