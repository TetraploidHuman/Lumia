//! Task spawn / join / scope + fiber entry.

use super::scheduler::{
    assert_task_api_allowed, check_current_not_cancelled, current_scope_kind, current_waiter,
    enqueue, park_until, push_waiter_unique, suspend_current, with_sched, FiberSlot, PendingSpawn,
    SchedulerKind, ScopeId, TaskId, TaskState, Waiter, CURRENT_FIBER, SCOPE_STACK,
};
use crate::common::trap_abort;
use crate::gc::{lumia_alloc, lumia_root_pop, lumia_root_push};
use crate::heap::with_heap;
use lumia_abi::TYPE_TASK;
use std::cell::Cell;
use std::collections::VecDeque;

pub type TaskFn = extern "C" fn(i64) -> i64;
pub type TaskFnNullary = extern "C" fn() -> i64;

fn install_task_handle(task: u64) -> *mut u8 {
    // [task_id, result] — result word keeps the join value live after GC unpin.
    let p = lumia_alloc(16, TYPE_TASK);
    unsafe {
        *(p as *mut i64) = task as i64;
        *((p as *mut i64).add(1)) = 0;
    }
    // Root across sched publish so concurrent GC cannot free the handle early.
    let mut root_slot = p;
    lumia_root_push(&mut root_slot as *mut *mut u8);
    with_sched(|s| {
        if let Some(st) = s.tasks.get_mut(&task) {
            st.handle = p;
        }
    });
    lumia_root_pop();
    p
}

fn spawn_with(pending: PendingSpawn) -> *mut u8 {
    assert_task_api_allowed();
    crate::task::ensure_trap_hook();
    let scope_stack = super::scheduler::snapshot_scope_stack();
    if scope_stack.is_empty() {
        super::scheduler::recycle_scope_stack(scope_stack);
        trap_abort("lumia: spawn outside scope");
    }

    let env = match pending {
        PendingSpawn::Unary(_, e) => e,
        PendingSpawn::Nullary(_) => 0,
    };
    let kind = current_scope_kind();

    let (task, fiber) = with_sched(|s| {
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
                handle: std::ptr::null_mut(),
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
                coro: None,
                yielder: Cell::new(std::ptr::null()),
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
    });

    let handle = install_task_handle(task);
    let mut root_slot = handle;
    lumia_root_push(&mut root_slot as *mut *mut u8);
    enqueue(fiber);
    lumia_root_pop();
    crate::task::scheduler::lumia_abi_handoff_set(handle as i64);
    handle
}

pub(crate) fn task_spawn(func: TaskFn, env: i64) -> *mut u8 {
    spawn_with(PendingSpawn::Unary(func, env))
}

#[no_mangle]
pub extern "C" fn lumia_task_spawn(func: Option<TaskFn>, env: i64) -> *mut u8 {
    let Some(func) = func else {
        trap_abort("lumia: task_spawn null function");
    };
    task_spawn(func, env)
}

#[no_mangle]
pub extern "C" fn lumia_task_spawn_nullary(func: Option<TaskFnNullary>) -> *mut u8 {
    let Some(func) = func else {
        trap_abort("lumia: task_spawn_nullary null function");
    };
    spawn_with(PendingSpawn::Nullary(func))
}

fn task_id_from_handle(handle: *mut u8) -> u64 {
    if handle.is_null() {
        trap_abort("lumia: null task");
    }
    unsafe { *(handle as *const i64) as u64 }
}

pub(crate) fn task_join_id(task: TaskId) -> i64 {
    assert_task_api_allowed();
    check_current_not_cancelled();

    if let Some(fid) = CURRENT_FIBER.with(|c| c.get()) {
        let self_task = with_sched(|s| s.fibers.get(&fid).map(|slot| slot.task));
        if self_task == Some(task) {
            trap_abort("lumia: join self");
        }
    }

    // Register waiter atomically with the done check so a pool completion
    // between those steps cannot miss the wakeup.
    let waiter = current_waiter();
    let outcome = with_sched(|s| {
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
    });
    match outcome {
        JoinReg::Done {
            cancelled: true, ..
        } => trap_abort("lumia: join cancelled task"),
        JoinReg::Done { .. } => return take_join_result(task),
        JoinReg::Missing => {
            clear_abi_handoff();
            return 0;
        }
        JoinReg::Waiting => {}
    }

    match waiter {
        Waiter::Fiber(_) => suspend_current(),
        Waiter::Main => {
            park_until(|| with_sched(|s| s.tasks.get(&task).map(|st| st.done).unwrap_or(true)))
        }
    }

    let cancelled = with_sched(|s| s.tasks.get(&task).is_some_and(|st| st.cancelled));
    if cancelled {
        trap_abort("lumia: join cancelled task");
    }
    take_join_result(task)
}

/// Copy `st.result` onto sched `abi_handoff` so it stays GC-visible across `return`.
/// After the first successful join, drop the immortal `result_gc_pin` (handle word owns it).
fn take_join_result(task: TaskId) -> i64 {
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
    crate::task::scheduler::try_reap_task(task);
    v
}

enum JoinReg {
    Done {
        cancelled: bool,
    },
    Waiting,
    Missing,
}

pub(crate) fn task_join(handle: *mut u8) -> i64 {
    task_join_id(task_id_from_handle(handle))
}

#[no_mangle]
pub extern "C" fn lumia_task_join(handle: *mut u8) -> i64 {
    task_join(handle)
}

pub(crate) fn task_join_opt(handle: *mut u8, out_ok: *mut i64) -> i64 {
    assert_task_api_allowed();
    check_current_not_cancelled();
    if out_ok.is_null() {
        trap_abort("lumia: join_opt null out_ok");
    }
    let task = task_id_from_handle(handle);

    let waiter = current_waiter();
    let outcome = with_sched(|s| {
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
    });
    match outcome {
        JoinReg::Done {
            cancelled: true, ..
        } => {
            unsafe {
                *out_ok = 0;
            }
            clear_abi_handoff();
            return 0;
        }
        JoinReg::Done { .. } => {
            unsafe {
                *out_ok = 1;
            }
            return take_join_result(task);
        }
        JoinReg::Missing => {
            unsafe {
                *out_ok = 0;
            }
            clear_abi_handoff();
            return 0;
        }
        JoinReg::Waiting => {}
    }
    match waiter {
        Waiter::Fiber(_) => suspend_current(),
        Waiter::Main => {
            park_until(|| with_sched(|s| s.tasks.get(&task).map(|st| st.done).unwrap_or(true)))
        }
    }
    let cancelled = with_sched(|s| s.tasks.get(&task).is_some_and(|t| t.cancelled));
    if cancelled {
        unsafe {
            *out_ok = 0;
        }
        clear_abi_handoff();
        0
    } else {
        unsafe {
            *out_ok = 1;
        }
        take_join_result(task)
    }
}

fn clear_abi_handoff() {
    let tid = std::thread::current().id();
    with_sched(|s| {
        s.abi_handoff.remove(&tid);
    });
}

#[no_mangle]
pub extern "C" fn lumia_task_join_opt(handle: *mut u8, out_ok: *mut i64) -> i64 {
    task_join_opt(handle, out_ok)
}

#[no_mangle]
pub extern "C" fn lumia_scope_enter(kind: i64) {
    assert_task_api_allowed();
    crate::task::ensure_trap_hook();
    let k = SchedulerKind::from_i64(kind);
    let sid: ScopeId = with_sched(|s| {
        let id = s.next_id;
        s.next_id = id + 1;
        s.scopes.insert(
            id,
            super::scheduler::ScopeFrame {
                children: vec![],
                kind: k,
            },
        );
        id
    });
    SCOPE_STACK.with(|s| {
        s.borrow_mut().push(sid);
    });
    crate::task::scheduler::refresh_scope_kind_cache();
}

#[no_mangle]
pub extern "C" fn lumia_scope_leave() {
    assert_task_api_allowed();
    let sid = SCOPE_STACK.with(|s| s.borrow_mut().pop());
    crate::task::scheduler::refresh_scope_kind_cache();
    let children = sid
        .and_then(|id| with_sched(|s| s.scopes.remove(&id).map(|f| f.children)))
        .unwrap_or_default();
    let need_join: Vec<TaskId> = with_sched(|s| {
        children
            .into_iter()
            .filter(|&task| {
                s.tasks
                    .get(&task)
                    .map(|st| !st.done && !st.cancelled)
                    .unwrap_or(false)
            })
            .collect()
    });
    for task in need_join {
        let _ = task_join_id(task);
    }
    crate::task::scheduler::lumia_scheduler_drain();
}

#[no_mangle]
pub extern "C" fn lumia_scope_cancel() {
    assert_task_api_allowed();
    crate::task::scheduler::cancel_scope_children();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::PAR_WORKER;
    use crate::gc::lumia_alloc;
    use crate::task::channel::{lumia_channel_new, lumia_channel_recv, lumia_channel_send};
    use crate::task::scheduler::{
        cancel_scope_children, lumia_scheduler_drain, snapshot_sched_gc_roots, SCHEDULER_WORKER,
    };
    use lumia_abi::TYPE_TASK;

    extern "C" fn add_one(env: i64) -> i64 {
        env + 1
    }
    extern "C" fn nullary_seven() -> i64 {
        7
    }
    extern "C" fn send_then_done(env: i64) -> i64 {
        lumia_channel_send(env as *mut u8, 42);
        0
    }
    extern "C" fn block_on_recv(env: i64) -> i64 {
        lumia_channel_recv(env as *mut u8)
    }

    #[test]
    fn spawn_join_on_main() {
        lumia_scope_enter(0);
        let t = task_spawn(add_one, 41);
        let v = task_join(t);
        lumia_scope_leave();
        assert_eq!(v, 42);
    }

    #[test]
    fn spawn_nullary_join() {
        lumia_scope_enter(0);
        let t = spawn_with(PendingSpawn::Nullary(nullary_seven));
        let v = task_join(t);
        lumia_scope_leave();
        assert_eq!(v, 7);
    }

    #[test]
    fn spawn_inherits_scheduler_kind() {
        lumia_scope_enter(SCHEDULER_WORKER);
        let t = task_spawn(add_one, 1);
        let task = unsafe { *(t as *const i64) as u64 };
        let kind = with_sched(|s| s.tasks.get(&task).map(|st| st.kind));
        assert_eq!(kind, Some(SchedulerKind::Worker));
        let _ = task_join(t);
        lumia_scope_leave();
    }

    #[test]
    fn channel_between_tasks() {
        lumia_scope_enter(0);
        let ch = lumia_channel_new(1);
        let _ = task_spawn(send_then_done, ch as i64);
        let blocked = task_spawn(block_on_recv, ch as i64);
        let v = task_join(blocked);
        lumia_scope_leave();
        assert_eq!(v, 42);
    }

    #[test]
    fn leave_joins_fire_and_forget() {
        lumia_scope_enter(0);
        let _ = task_spawn(add_one, 99);
        lumia_scope_leave();
    }

    #[test]
    fn cancel_never_started_then_leave() {
        lumia_scope_enter(0);
        let _ = task_spawn(add_one, 1);
        cancel_scope_children();
        lumia_scheduler_drain();
        lumia_scope_leave();
    }

    #[test]
    fn cancel_from_fiber_spares_self() {
        lumia_scope_enter(0);
        let ch = lumia_channel_new(1);
        let blocked = task_spawn(block_on_recv, ch as i64);
        extern "C" fn killer(_env: i64) -> i64 {
            crate::task::scheduler::cancel_scope_children();
            7
        }
        let k = task_spawn(killer, 0);
        assert_eq!(task_join(k), 7);
        let mut ok = 0i64;
        let _ = task_join_opt(blocked, &mut ok);
        assert_eq!(ok, 0);
        lumia_scope_leave();
    }

    #[test]
    fn leave_after_cancel_skips_cancelled() {
        lumia_scope_enter(0);
        let ch = lumia_channel_new(1);
        let _ = task_spawn(block_on_recv, ch as i64);
        lumia_scheduler_drain();
        cancel_scope_children();
        lumia_scheduler_drain();
        lumia_scope_leave();
    }

    #[test]
    fn pending_env_marked_before_run() {
        lumia_scope_enter(0);
        let heap = lumia_alloc(8, TYPE_TASK);
        unsafe {
            *(heap as *mut i64) = 123;
        }
        let env = heap as i64;
        let _t = task_spawn(add_one, env);
        let found = with_sched(|s| s.tasks.values().any(|st| st.env == env));
        assert!(found);
        let (_, vals) = snapshot_sched_gc_roots();
        assert!(vals.contains(&env));
        lumia_scope_leave();
    }

    #[test]
    #[should_panic(expected = "spawn outside scope")]
    fn spawn_outside_scope_traps() {
        let _ = task_spawn(add_one, 1);
    }

    #[test]
    #[should_panic(expected = "task/channel API on parallel map worker")]
    fn task_api_on_par_worker_traps() {
        PAR_WORKER.with(|c| c.set(true));
        let _ = task_spawn(add_one, 1);
    }

    #[test]
    #[should_panic(expected = "join self")]
    fn join_self_traps() {
        lumia_scope_enter(0);
        let t = task_spawn(add_one, 1);
        let task = unsafe { *(t as *const i64) as u64 };
        let fid = with_sched(|s| s.tasks.get(&task).and_then(|st| st.fiber)).expect("fiber");
        CURRENT_FIBER.with(|c| c.set(Some(fid)));
        let _ = task_join_id(task);
    }

    #[test]
    fn cancel_started_reclaims_without_abandon_leak() {
        lumia_scope_enter(0);
        let ch = lumia_channel_new(1);
        let _ = task_spawn(block_on_recv, ch as i64);
        lumia_scheduler_drain();
        cancel_scope_children();
        // Started fiber was force_reset + Drop (no forget / abandon leak).
        with_sched(|s| assert_eq!(s.fibers.len(), 0));
        lumia_scope_leave();
    }

    #[test]
    #[should_panic(expected = "join cancelled task")]
    fn cancel_then_join_traps() {
        lumia_scope_enter(0);
        let ch = lumia_channel_new(1);
        let t = task_spawn(block_on_recv, ch as i64);
        lumia_scheduler_drain();
        cancel_scope_children();
        let _ = task_join(t);
    }

    #[test]
    fn join_unpins_result_from_sched_snapshot() {
        lumia_scope_enter(0);
        let t = task_spawn(add_one, 41);
        let task = unsafe { *(t as *const i64) as u64 };
        let _ = task_join(t);
        let still_pinned = with_sched(|s| {
            s.tasks
                .get(&task)
                .is_some_and(|st| st.result_gc_pin)
        });
        assert!(!still_pinned, "first join should clear result_gc_pin");
        let task_pins = with_sched(|s| {
            s.tasks
                .values()
                .filter(|st| st.result_gc_pin)
                .filter_map(|st| st.result)
                .collect::<Vec<_>>()
        });
        assert!(
            !task_pins.contains(&42),
            "task table must not GC-pin result after join"
        );
        let mirrored = unsafe { *((t as *const i64).add(1)) };
        assert_eq!(mirrored, 42);
        lumia_scope_leave();
    }
}
