//! Deferred cycle-candidate buffer for `--mm arc`.
//!
//! When an Arc release leaves `rc > 0` on a pointer-bearing object, enqueue it.
//! Crossing a threshold requests a STW full collect (Arc already disables
//! concurrent mark). Not Bacon–Rajan trial-delete — reclaim is still mark-sweep.

use crate::common::{
    tid_base, ObjectHeader, GC_INHIBIT, RC_SHARED, TYPE_ADT, TYPE_CLOSURE, TYPE_LIST,
    TYPE_LIST_SLICE, TYPE_MAP, TYPE_SET,
};
use crate::mm::{current_mm_mode, MmMode};
use rustc_hash::FxHashSet;
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

const DEFAULT_THRESH: usize = 64;

thread_local! {
    static CANDS: RefCell<FxHashSet<*mut ObjectHeader>> =
        RefCell::new(FxHashSet::default());
    static THRESH: Cell<usize> = const { Cell::new(DEFAULT_THRESH) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
    /// Nesting depth inside [`crate::arc_free`] free (avoid collect reentrancy).
    static IN_ARC_FREE: Cell<u32> = const { Cell::new(0) };
}

static THRESH_ENV: OnceLock<()> = OnceLock::new();

fn init_thresh_from_env() {
    THRESH_ENV.get_or_init(|| {
        if let Ok(v) = std::env::var("LUMI_ARC_CYCLE_THRESH") {
            if let Ok(n) = v.parse::<usize>() {
                THRESH.with(|c| c.set(n.max(1)));
            }
        }
    });
}

fn can_hold_ptrs(tid: u32) -> bool {
    matches!(
        tid_base(tid),
        TYPE_LIST | TYPE_LIST_SLICE | TYPE_MAP | TYPE_SET | TYPE_ADT | TYPE_CLOSURE
    )
}

/// Enter / leave Arc free-on-zero so flushes do not nest inside dealloc.
pub(crate) fn arc_free_enter() {
    IN_ARC_FREE.set(IN_ARC_FREE.get().saturating_add(1));
}

pub(crate) fn arc_free_leave() {
    IN_ARC_FREE.set(IN_ARC_FREE.get().saturating_sub(1));
}

/// After Arc decrement with `rc > 0`, record a possible cycle participant.
pub(crate) fn note_cycle_candidate(obj: *mut ObjectHeader) {
    if current_mm_mode() != MmMode::Arc || obj.is_null() {
        return;
    }
    init_thresh_from_env();
    unsafe {
        let rc = (*obj).rc;
        if rc == 0 || rc == RC_SHARED || !can_hold_ptrs((*obj).type_id) {
            return;
        }
    }
    CANDS.with(|c| {
        let mut set = c.borrow_mut();
        set.insert(obj);
        if set.len() >= THRESH.get() {
            set.clear();
            PENDING.set(true);
        }
    });
}

pub(crate) fn clear_cycle_candidates() {
    CANDS.with(|c| c.borrow_mut().clear());
    PENDING.set(false);
}

/// Consume a pending cycle-collect request (clears the candidate set).
pub(crate) fn take_cycle_collect_pending() -> bool {
    if !PENDING.get() {
        return false;
    }
    PENDING.set(false);
    CANDS.with(|c| c.borrow_mut().clear());
    true
}

/// Run STW collect if a threshold flush is pending and it is safe to do so.
pub(crate) fn try_flush_cycle_collect() {
    if IN_ARC_FREE.get() > 0 || GC_INHIBIT.get() > 0 || crate::gc::is_full_marking() {
        return;
    }
    if !take_cycle_collect_pending() {
        return;
    }
    crate::gc::lumi_gc_collect();
}

#[cfg(test)]
pub(crate) fn cycle_cand_set_threshold_for_test(n: usize) {
    THRESH.with(|c| c.set(n.max(1)));
    clear_cycle_candidates();
}

#[cfg(test)]
pub(crate) fn cycle_cand_len_for_test() -> usize {
    CANDS.with(|c| c.borrow().len())
}

#[cfg(test)]
pub(crate) fn cycle_collect_pending_for_test() -> bool {
    PENDING.get()
}
