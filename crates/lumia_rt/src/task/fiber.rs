//! Task spawn / join / scope + fiber entry.
//!
//! SchedCore mutations go through [`super::sched_fiber_api`]; this module owns
//! FFI wrappers, trap checks, and suspend/park control flow.
//!
//! # Safety (FFI)
//! Task `handle` is a valid TYPE_TASK payload; `out_ok` is writable.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use super::sched_core::{PendingSpawn, SchedulerKind, TaskId, Waiter};
use super::sched_fiber_api::{
    clear_thread_abi_handoff, fiber_owner_task, filter_unfinished, publish_join_result,
    register_spawn, scope_alloc, scope_take_children, set_task_handle, task_is_cancelled,
    task_is_done, try_register_join, JoinReg,
};
use super::scheduler::{
    assert_task_api_allowed, check_current_not_cancelled, current_scope_kind, current_waiter,
    enqueue, park_until, suspend_current, CURRENT_FIBER, SCOPE_STACK,
};
use crate::common::trap_abort;
use crate::concurrency_policy::{
    recycle_scope_stack, snapshot_scope_stack, with_rooted_payload,
};
use crate::gc::lumia_alloc;
use lumia_abi::TYPE_TASK;

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
    with_rooted_payload(p, || {
        set_task_handle(task, p);
    });
    p
}

fn spawn_with(pending: PendingSpawn) -> *mut u8 {
    assert_task_api_allowed();
    crate::task::ensure_trap_hook();
    let scope_stack = snapshot_scope_stack();
    if scope_stack.is_empty() {
        recycle_scope_stack(scope_stack);
        trap_abort("lumia: spawn outside scope");
    }

    let env = match pending {
        PendingSpawn::Unary(_, e) => e,
        PendingSpawn::Nullary(_) => 0,
    };
    let kind = current_scope_kind();
    let (task, fiber) = register_spawn(pending, scope_stack, kind, env);

    let handle = install_task_handle(task);
    with_rooted_payload(handle, || {
        enqueue(fiber);
    });
    crate::task::scheduler::lumia_abi_handoff_set(handle as i64);
    handle
}

pub(crate) fn task_spawn(func: TaskFn, env: i64) -> *mut u8 {
    spawn_with(PendingSpawn::Unary(func, env))
}

/// Spawn a unary task body `fn(env) -> i64`.
///
/// # Safety
/// `func` must be a valid C ABI entry (null traps); returned handle is a Task payload.
#[no_mangle]
pub extern "C" fn lumia_task_spawn(func: Option<TaskFn>, env: i64) -> *mut u8 {
    let Some(func) = func else {
        trap_abort("lumia: task_spawn null function");
    };
    task_spawn(func, env)
}

/// Spawn a nullary task body `fn() -> i64`.
///
/// # Safety
/// `func` must be a valid C ABI entry (null traps); returned handle is a Task payload.
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
        if fiber_owner_task(fid) == Some(task) {
            trap_abort("lumia: join self");
        }
    }

    // Register waiter atomically with the done check so a pool completion
    // between those steps cannot miss the wakeup.
    let waiter = current_waiter();
    match try_register_join(task, waiter) {
        JoinReg::Done {
            cancelled: true, ..
        } => trap_abort("lumia: join cancelled task"),
        JoinReg::Done { .. } => return publish_join_result(task),
        JoinReg::Missing => {
            clear_thread_abi_handoff();
            return 0;
        }
        JoinReg::Waiting => {}
    }

    match waiter {
        Waiter::Fiber(_) => suspend_current(),
        Waiter::Main => park_until(|| task_is_done(task)),
    }

    if task_is_cancelled(task) {
        trap_abort("lumia: join cancelled task");
    }
    publish_join_result(task)
}

pub(crate) fn task_join(handle: *mut u8) -> i64 {
    task_join_id(task_id_from_handle(handle))
}

/// # Safety
/// `handle` is null or a valid `TYPE_TASK` payload (see module Safety).
#[no_mangle]
pub unsafe extern "C" fn lumia_task_join(handle: *mut u8) -> i64 {
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
    match try_register_join(task, waiter) {
        JoinReg::Done {
            cancelled: true, ..
        } => {
            unsafe {
                *out_ok = 0;
            }
            clear_thread_abi_handoff();
            return 0;
        }
        JoinReg::Done { .. } => {
            unsafe {
                *out_ok = 1;
            }
            return publish_join_result(task);
        }
        JoinReg::Missing => {
            unsafe {
                *out_ok = 0;
            }
            clear_thread_abi_handoff();
            return 0;
        }
        JoinReg::Waiting => {}
    }
    match waiter {
        Waiter::Fiber(_) => suspend_current(),
        Waiter::Main => park_until(|| task_is_done(task)),
    }
    if task_is_cancelled(task) {
        unsafe {
            *out_ok = 0;
        }
        clear_thread_abi_handoff();
        0
    } else {
        unsafe {
            *out_ok = 1;
        }
        publish_join_result(task)
    }
}

/// # Safety
/// `handle` is null or a valid `TYPE_TASK` payload; `out_ok` is a writable `i64`.
#[no_mangle]
pub unsafe extern "C" fn lumia_task_join_opt(handle: *mut u8, out_ok: *mut i64) -> i64 {
    task_join_opt(handle, out_ok)
}

/// Enter a structured-concurrency scope (`kind` selects scheduler policy).
///
/// # Safety
/// Must be paired with [`lumia_scope_leave`] / cancel on the same fiber; `kind` is a
/// documented scheduler enum discriminant.
#[no_mangle]
pub extern "C" fn lumia_scope_enter(kind: i64) {
    assert_task_api_allowed();
    crate::task::ensure_trap_hook();
    let k = SchedulerKind::from_i64(kind);
    let sid = scope_alloc(k);
    SCOPE_STACK.with(|s| {
        s.borrow_mut().push(sid);
    });
    crate::task::scheduler::refresh_scope_kind_cache();
}

/// Leave the current scope, joining unfinished children.
///
/// # Safety
/// Scope stack must be non-empty from a prior [`lumia_scope_enter`] on this fiber.
#[no_mangle]
pub extern "C" fn lumia_scope_leave() {
    assert_task_api_allowed();
    let sid = SCOPE_STACK.with(|s| s.borrow_mut().pop());
    crate::task::scheduler::refresh_scope_kind_cache();
    let children = sid.map(scope_take_children).unwrap_or_default();
    let need_join = filter_unfinished(children);
    for task in need_join {
        let _ = task_join_id(task);
    }
    crate::task::scheduler::lumia_scheduler_drain();
}

/// Cancel children of the current scope.
///
/// # Safety
/// Scope stack must be non-empty from a prior [`lumia_scope_enter`] on this fiber.
#[no_mangle]
pub extern "C" fn lumia_scope_cancel() {
    assert_task_api_allowed();
    crate::task::scheduler::cancel_scope_children();
}

#[cfg(test)]
#[path = "fiber_tests.rs"]
mod tests;
