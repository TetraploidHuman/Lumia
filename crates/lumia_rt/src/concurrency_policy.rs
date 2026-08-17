//! Narrow concurrency policy contracts shared across RT subsystems.
//!
//! Keeps Task/Channel ↔ GC ↔ list-par coupling behind named facades instead of
//! scattering scheduler / GC internals (Todo: Task ↔ GC ↔ list-par).

use crate::gc::{lumia_root_pop, lumia_root_push};
use crate::task::sched_core::ScopeId;

/// Whether list data-parallel workers must not run (DESIGN: no mix with
/// Task/Channel). Today this is exactly [`crate::task::task_runtime_active`];
/// callers should depend on this predicate, not the Task internals.
#[inline]
pub fn forbid_list_parallel() -> bool {
    crate::task::task_runtime_active()
}

/// Sched GC roots for shade/remark (parked fiber slots + channel buffers).
///
/// GC must take roots only through this facade — not by reaching into
/// `task::scheduler` — so lock order (heap → sched) stays auditable at one edge.
#[inline]
pub fn snapshot_sched_gc_roots() -> (Vec<i64>, Vec<i64>) {
    crate::task::snapshot_sched_gc_roots()
}

/// Snapshot the TLS scope stack for spawn inheritance (recyclable buffer).
#[inline]
pub fn snapshot_scope_stack() -> Vec<ScopeId> {
    crate::task::scheduler::snapshot_scope_stack()
}

/// Return a scope-stack buffer to the freelist after spawn / abort.
#[inline]
pub fn recycle_scope_stack(stack: Vec<ScopeId>) {
    crate::task::scheduler::recycle_scope_stack(stack)
}

/// Root a heap payload across a short critical section (sched publish / enqueue).
///
/// Fiber/channel use this instead of open-coding `root_push`/`root_pop` so the
/// GC edge stays one named contract.
#[inline]
pub fn with_rooted_payload<R>(payload: *mut u8, f: impl FnOnce() -> R) -> R {
    let mut root_slot = payload;
    unsafe { lumia_root_push(&mut root_slot as *mut *mut u8) };
    let out = f();
    lumia_root_pop();
    out
}
