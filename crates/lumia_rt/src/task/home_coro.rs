//! Home-thread storage for `!Send` corosensei stacks.
//!
//! Keeps [`Coroutine`] out of process-shared [`super::sched_core::SchedCore`] so
//! the scheduler maps are naturally `Send`. Live stacks are created, resumed,
//! parked, and disposed only on the fiber's **home** OS thread (see cancel
//! reclaim / `reclaim_home`).

use corosensei::Coroutine;
use rustc_hash::FxHashMap;
use std::cell::RefCell;

use super::sched_core::FiberId;

thread_local! {
    static HOME_COROS: RefCell<FxHashMap<FiberId, Coroutine<(), (), i64>>> =
        RefCell::new(FxHashMap::default());
}

/// Park a suspended coroutine on this OS thread (fiber `home` must be current).
pub(super) fn park(fid: FiberId, coro: Coroutine<(), (), i64>) {
    HOME_COROS.with(|m| {
        let prev = m.borrow_mut().insert(fid, coro);
        debug_assert!(
            prev.is_none(),
            "lumia: double-park coroutine for fiber {fid}"
        );
    });
}

/// Take a parked coroutine from this OS thread's TLS (if any).
pub(super) fn take(fid: FiberId) -> Option<Coroutine<(), (), i64>> {
    HOME_COROS.with(|m| m.borrow_mut().remove(&fid))
}
