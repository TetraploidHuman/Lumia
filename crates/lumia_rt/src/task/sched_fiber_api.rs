//! Narrow fiber lifecycle API over [`SchedCore`] (spawn / join / scope mutations).
//!
//! Keeps `fiber.rs` as FFI + control-flow orchestration without constructing
//! [`TaskState`] / [`FiberSlot`] / [`ScopeFrame`] or poking sched maps directly.

use super::scan_ptrs::{TaskHandlePtr, YielderAddr};
use super::sched_core::{
    with_sched, FiberId, FiberSlot, PendingSpawn, SchedulerKind, ScopeFrame, ScopeId, TaskId,
    TaskState, Waiter,
};
use super::scheduler::{push_waiter_unique, try_reap_task};
use crate::heap::with_heap;
use std::cell::Cell;
use std::collections::VecDeque;

/// Outcome of atomically registering a join waiter with the done check.
pub(super) enum JoinReg {
    Done { cancelled: bool },
    Waiting,
    Missing,
}

/// Allocate task+fiber ids, install pending spawn, inherit scope stack.
pub(super) fn register_spawn(
    pending: PendingSpawn,
    scope_stack: Vec<ScopeId>,
    kind: SchedulerKind,
    env: i64,
) -> (TaskId, FiberId) {
    with_sched(|s| {
        let task = s.next_id;
        let fiber = task + 1;
        s.next_id = fiber + 1;
        s.tasks.insert(
            task,
            TaskState {
                fiber: Some(fiber),
                result: None,
                result_gc_pin: false,
                done: false,
                cancelled: false,
                join_waiters: VecDeque::new(),
                handle: TaskHandlePtr::null(),
                env,
                kind,
            },
        );
        s.fibers.insert(
            fiber,
            FiberSlot {
                task,
                kind,
                pending: Some(pending),
                has_coro: false,
                yielder: Cell::new(YielderAddr::null()),
                home: None,
                running: false,
                wake_pending: false,
                on_ready: false,
                reclaim_home: false,
            },
        );
        if let Some(&sid) = scope_stack.last() {
            if let Some(frame) = s.scopes.get_mut(&sid) {
                frame.children.push(task);
            }
        }
        // Inherit enclosing scopes so OS-pool fibers see spawn/cancelScope.
        s.parked_scope_stacks.insert(fiber, scope_stack);
        (task, fiber)
    })
}

pub(super) fn set_task_handle(task: TaskId, handle: *mut u8) {
    with_sched(|s| {
        if let Some(st) = s.tasks.get_mut(&task) {
            st.handle = TaskHandlePtr::from_raw(handle);
        }
    });
}

pub(super) fn fiber_owner_task(fid: FiberId) -> Option<TaskId> {
    with_sched(|s| s.fibers.get(&fid).map(|slot| slot.task))
}

pub(super) fn try_register_join(task: TaskId, waiter: Waiter) -> JoinReg {
    with_sched(|s| {
        let Some(st) = s.tasks.get_mut(&task) else {
            return JoinReg::Missing;
        };
        if st.done {
            return JoinReg::Done {
                cancelled: st.cancelled,
            };
        }
        push_waiter_unique(&mut st.join_waiters, waiter);
        JoinReg::Waiting
    })
}

pub(super) fn task_is_cancelled(task: TaskId) -> bool {
    with_sched(|s| s.tasks.get(&task).is_some_and(|st| st.cancelled))
}

pub(super) fn task_is_done(task: TaskId) -> bool {
    with_sched(|s| s.tasks.get(&task).map(|st| st.done).unwrap_or(true))
}

/// Publish join result onto ABI handoff (heap → sched) and unpin / reap.
pub(super) fn publish_join_result(task: TaskId) -> i64 {
    let tid = std::thread::current().id();
    let v = with_heap(|h| {
        with_sched(|s| {
            let st = s.tasks.get_mut(&task);
            let v = st.as_ref().and_then(|t| t.result).unwrap_or(0);
            if h.full_marking {
                crate::gc::mark_value(v);
            }
            if let Some(st) = st {
                st.result_gc_pin = false;
            }
            s.abi_handoff.insert(tid, v);
            v
        })
    });
    try_reap_task(task);
    v
}

pub(super) fn clear_thread_abi_handoff() {
    let tid = std::thread::current().id();
    with_sched(|s| {
        s.abi_handoff.remove(&tid);
    });
}

pub(super) fn scope_alloc(kind: SchedulerKind) -> ScopeId {
    with_sched(|s| {
        let id = s.next_id;
        s.next_id = id + 1;
        s.scopes.insert(
            id,
            ScopeFrame {
                children: vec![],
                kind,
            },
        );
        id
    })
}

pub(super) fn scope_take_children(sid: ScopeId) -> Vec<TaskId> {
    with_sched(|s| {
        s.scopes
            .remove(&sid)
            .map(|f| f.children)
            .unwrap_or_default()
    })
}

/// Tasks among `candidates` that are still unfinished (not done and not cancelled).
pub(super) fn filter_unfinished(candidates: Vec<TaskId>) -> Vec<TaskId> {
    with_sched(|s| {
        candidates
            .into_iter()
            .filter(|&task| {
                s.tasks
                    .get(&task)
                    .map(|st| !st.done && !st.cancelled)
                    .unwrap_or(false)
            })
            .collect()
    })
}
