//! Fiber suspend / resume / park / drain.

use crate::common::{trap_abort, CALL_STACK};
use crate::heap::with_heap;
use crate::mutator::{set_local_roots, take_local_roots};
use corosensei::{Coroutine, CoroutineResult};
use std::collections::VecDeque;
use std::time::Duration;

use super::home_coro;
use super::scan_ptrs::YielderAddr;
use super::sched_cancel::{dispose_cancelled_coroutine, scrub_roots_before_coro_drop};
use super::sched_core::{
    sched_wait_while, with_sched, FiberId, PendingSpawn, SchedCore, SchedulerKind, TaskId, Waiter,
};
use super::sched_env::sched_pool_counts;
use super::sched_queue::{enqueue, pop_ready, wake_many};
use super::sched_roots::{
    discard_parked_scope_stack, load_fiber_roots, restore_host_roots, save_fiber_roots,
};
use super::scheduler::{
    recycle_coroutine_stack, recycle_fiber_stack, recycle_scope_stack, refresh_scope_kind_cache,
    take_fiber_stack, CURRENT_FIBER, CURRENT_YIELDER, SCOPE_KIND_CACHE, SCOPE_STACK,
};

pub(super) fn suspend_current() {
    let y = CURRENT_YIELDER.with(|c| c.get());
    if y.is_null() {
        trap_abort("lumia: suspend on main stack");
    }
    // v1 contract: the only yield points are RT `suspend_current` call sites.
    // Cancel reclaim uses `force_reset`, which requires no Rust `Drop` locals
    // across this yield. Do not introduce Drop guards / RAII across this call
    // (a future C-unwind TaskFn ABI would be needed to support that).
    if let Some(fid) = CURRENT_FIBER.with(|c| c.get()) {
        save_fiber_roots(fid);
    }
    unsafe {
        (*y).suspend(());
    }
}

pub(super) fn current_waiter() -> Waiter {
    match CURRENT_FIBER.with(|c| c.get()) {
        Some(fid) => Waiter::Fiber(fid),
        None => Waiter::Main,
    }
}

pub(super) fn park_until(mut pred: impl FnMut() -> bool) {
    let mut idle = 0u32;
    while !pred() {
        if let Some(fid) = pop_ready() {
            idle = 0;
            resume_fiber(fid);
            continue;
        }
        if pred() {
            break;
        }
        let (workers, ios) = sched_pool_counts();
        let waiting_on_pool = (workers > 0 || ios > 0)
            && with_sched(|s| {
                s.pool_runners > 0
                    || (workers > 0 && s.kind_pending(SchedulerKind::Worker))
                    || (ios > 0 && s.kind_pending(SchedulerKind::Io))
            });
        if waiting_on_pool {
            let tid = std::thread::current().id();
            // Wait while no local coop work is runnable for us AND a pool-owned queue is busy.
            sched_wait_while(
                |s| {
                    let local_idle = !s.queue_has_runnable_for(SchedulerKind::Default, tid)
                        && (workers > 0 || !s.queue_has_runnable_for(SchedulerKind::Worker, tid))
                        && (ios > 0 || !s.queue_has_runnable_for(SchedulerKind::Io, tid));
                    local_idle
                        && (s.pool_runners > 0
                            || (workers > 0 && s.kind_pending(SchedulerKind::Worker))
                            || (ios > 0 && s.kind_pending(SchedulerKind::Io)))
                },
                Duration::from_millis(10),
            );
            idle = 0;
            continue;
        }
        idle = idle.saturating_add(1);
        if idle > 3 {
            trap_abort("lumia: task deadlock (empty ready queue)");
        }
    }
}

pub(crate) fn resume_fiber(fid: FiberId) {
    // Nested resume while a fiber is live on this stack corrupts parked roots.
    if let Some(cur) = CURRENT_FIBER.with(|c| c.get()) {
        if cur != fid {
            enqueue(fid);
            return;
        }
    }

    let tid = std::thread::current().id();
    // Pop already claims `running`; finish affinity + cancel check (do not clear wake_pending).
    // `home` is pinned only once a coroutine stack exists (unstarted work stays stealable).
    let claim = with_sched(|s| {
        let slot = s.fibers.get_mut(&fid)?;
        if !slot.running {
            slot.running = true;
        }
        if let Some(home) = slot.home {
            if home != tid {
                slot.running = false;
                return None;
            }
        }
        let task = slot.task;
        let cancelled = s.tasks.get(&task).is_some_and(|t| t.cancelled) || slot.reclaim_home;
        Some((task, cancelled))
    });
    let Some((task_id, cancelled)) = claim else {
        // Affinity reject / missing slot: put back on the correct ready queue.
        enqueue(fid);
        return;
    };

    // Stale ABI handoff from a prior return must not outlive this sched entry.
    clear_thread_abi_handoff();

    if CURRENT_FIBER.with(|c| c.get()).is_none() {
        with_heap(|_| {
            let roots = take_local_roots();
            let frames = CALL_STACK.with(|s| std::mem::take(&mut *s.borrow_mut()));
            let scopes = SCOPE_STACK.with(|s| std::mem::take(&mut *s.borrow_mut()));
            SCOPE_KIND_CACHE.with(|c| c.set(0));
            with_sched(|s| {
                s.host_roots.insert(tid, roots.into());
                s.host_call_stacks.insert(tid, frames.into());
                s.host_scope_stacks.insert(tid, scopes);
            });
        });
    }
    load_fiber_roots(fid);
    CURRENT_FIBER.with(|c| c.set(Some(fid)));

    if cancelled {
        let had_coro = with_sched(|s| {
            let had = s.fibers.remove(&fid).map(|mut slot| {
                if let Some(st) = s.tasks.get_mut(&slot.task) {
                    st.env = 0;
                }
                let _ = slot.pending.take();
                slot.has_coro
            });
            s.parked_roots.remove(&fid);
            s.parked_call_stacks.remove(&fid);
            discard_parked_scope_stack(s, fid);
            had.unwrap_or(false)
        });
        CURRENT_FIBER.with(|c| c.set(None));
        CURRENT_YIELDER.with(|c| c.set(std::ptr::null()));
        scrub_roots_before_coro_drop(fid);
        if had_coro {
            if let Some(c) = home_coro::take(fid) {
                dispose_cancelled_coroutine(c);
            }
        }
        restore_host_roots();
        return;
    }

    // Allocate fiber stack *outside* the sched lock (mmap is slow).
    let needs_stack = with_sched(|s| {
        s.fibers.get(&fid).is_some_and(|slot| {
            !slot.has_coro
                && slot.pending.is_some()
                && !s.tasks.get(&slot.task).is_some_and(|t| t.cancelled)
        })
    });
    let mut pre_stack = if needs_stack {
        Some(take_fiber_stack())
    } else {
        None
    };

    // Build (if needed) + take coro + yielder; re-check cancel under the same lock.
    // Parked stacks live in home-thread TLS (`home_coro`), not in `SchedCore`.
    let taken = with_sched(|s| {
        let cancelled_now = s.tasks.get(&task_id).is_some_and(|t| t.cancelled);
        if cancelled_now {
            return Err(());
        }
        let Some(slot) = s.fibers.get_mut(&fid) else {
            return Ok(None);
        };
        if !slot.has_coro {
            let Some(pending) = slot.pending.take() else {
                return Ok(None);
            };
            let Some(stack) = pre_stack.take() else {
                slot.pending = Some(pending);
                return Ok(None);
            };
            let body = pending;
            let task = slot.task;
            // First stack creation pins affinity to this OS thread.
            slot.home = Some(tid);
            let coro = Coroutine::with_stack(stack, move |yielder, ()| {
                let yptr = yielder as *const _;
                CURRENT_YIELDER.with(|c| c.set(yptr));
                CURRENT_FIBER.with(|c| c.set(Some(fid)));
                with_sched(|s| {
                    if let Some(slot) = s.fibers.get(&fid) {
                        slot.yielder.set(YielderAddr::from_raw(yptr));
                    }
                });
                let out = match body {
                    PendingSpawn::Unary(func, e) => func(e),
                    PendingSpawn::Nullary(func) => func(),
                };
                CURRENT_YIELDER.with(|c| c.set(std::ptr::null()));
                let waiters = with_heap(|h| {
                    if h.full_marking {
                        crate::gc::mark_value(out);
                    }
                    with_sched(|s| publish_task_result(s, task, out))
                });
                wake_many(waiters);
                out
            });
            let y = slot.yielder.get().as_raw();
            Ok(Some((coro, y)))
        } else {
            if slot.home.is_none() {
                slot.home = Some(tid);
            }
            slot.has_coro = false;
            let y = slot.yielder.get().as_raw();
            let coro = home_coro::take(fid).expect("has_coro without home TLS entry");
            Ok(Some((coro, y)))
        }
    });
    if let Some(stack) = pre_stack.take() {
        recycle_fiber_stack(stack);
    }
    let taken = match taken {
        Err(()) => {
            let had_coro = with_sched(|s| {
                let had = s.fibers.remove(&fid).map(|mut slot| {
                    if let Some(st) = s.tasks.get_mut(&slot.task) {
                        st.env = 0;
                    }
                    let _ = slot.pending.take();
                    slot.has_coro
                });
                s.parked_roots.remove(&fid);
                s.parked_call_stacks.remove(&fid);
                discard_parked_scope_stack(s, fid);
                had.unwrap_or(false)
            });
            CURRENT_FIBER.with(|c| c.set(None));
            CURRENT_YIELDER.with(|c| c.set(std::ptr::null()));
            scrub_roots_before_coro_drop(fid);
            if had_coro {
                if let Some(c) = home_coro::take(fid) {
                    dispose_cancelled_coroutine(c);
                }
            }
            restore_host_roots();
            return;
        }
        Ok(v) => v,
    };
    let Some((mut coro, y)) = taken else {
        clear_running(fid);
        CURRENT_FIBER.with(|c| c.set(None));
        restore_host_roots();
        return;
    };
    if !y.is_null() {
        CURRENT_YIELDER.with(|c| c.set(y));
    }

    let result = coro.resume(());

    match result {
        CoroutineResult::Yield(()) => {
            enum YieldOut {
                Requeue,
                Dispose(Coroutine<(), (), i64>),
                Parked,
            }
            let out = with_sched(|s| {
                let cancelled = s.fibers.get(&fid).is_some_and(|slot| {
                    slot.reclaim_home || s.tasks.get(&slot.task).is_some_and(|t| t.cancelled)
                }) || !s.fibers.contains_key(&fid);
                if cancelled {
                    if let Some(mut slot) = s.fibers.remove(&fid) {
                        slot.running = false;
                        // Runner owns `coro` on the stack; drop any stale TLS park.
                        if slot.has_coro {
                            let _ = home_coro::take(fid);
                        }
                        if let Some(st) = s.tasks.get_mut(&slot.task) {
                            st.env = 0;
                        }
                    }
                    s.parked_roots.remove(&fid);
                    s.parked_call_stacks.remove(&fid);
                    discard_parked_scope_stack(s, fid);
                    YieldOut::Dispose(coro)
                } else if let Some(slot) = s.fibers.get_mut(&fid) {
                    slot.has_coro = true;
                    home_coro::park(fid, coro);
                    slot.running = false;
                    let pending = slot.wake_pending;
                    slot.wake_pending = false;
                    if pending {
                        YieldOut::Requeue
                    } else {
                        YieldOut::Parked
                    }
                } else {
                    if let Some(st) = s.tasks.get_mut(&task_id) {
                        st.env = 0;
                    }
                    s.parked_roots.remove(&fid);
                    s.parked_call_stacks.remove(&fid);
                    discard_parked_scope_stack(s, fid);
                    YieldOut::Dispose(coro)
                }
            });
            CURRENT_FIBER.with(|c| c.set(None));
            CURRENT_YIELDER.with(|c| c.set(std::ptr::null()));
            match out {
                YieldOut::Dispose(c) => {
                    scrub_roots_before_coro_drop(fid);
                    dispose_cancelled_coroutine(c);
                    restore_host_roots();
                }
                YieldOut::Requeue => {
                    restore_host_roots();
                    enqueue(fid);
                }
                YieldOut::Parked => {
                    restore_host_roots();
                }
            }
        }
        CoroutineResult::Return(val) => {
            CURRENT_FIBER.with(|c| c.set(None));
            CURRENT_YIELDER.with(|c| c.set(std::ptr::null()));
            // Coro wrapper usually already published; keep idempotent publish as
            // a safety net, then scrub fiber roots / drop the stack.
            let tid = std::thread::current().id();
            let waiters = with_heap(|_| {
                let waiters = with_sched(|s| {
                    let waiters = publish_task_result(s, task_id, val);
                    s.parked_roots.remove(&fid);
                    s.parked_call_stacks.remove(&fid);
                    discard_parked_scope_stack(s, fid);
                    let _ = s.fibers.remove(&fid);
                    if let Some(st) = s.tasks.get_mut(&task_id) {
                        st.fiber = None;
                    }
                    waiters
                });
                let _ = take_local_roots();
                SCOPE_STACK.with(|s| {
                    let old = std::mem::take(&mut *s.borrow_mut());
                    recycle_scope_stack(old);
                });
                let (host_roots, host_frames, host_scopes) = with_sched(|s| {
                    (
                        s.host_roots.remove(&tid),
                        s.host_call_stacks.remove(&tid),
                        s.host_scope_stacks.remove(&tid),
                    )
                });
                if let Some(roots) = host_roots {
                    set_local_roots(roots.into_vec());
                }
                if let Some(frames) = host_frames {
                    CALL_STACK.with(|s| *s.borrow_mut() = frames.into_vec());
                }
                if let Some(scopes) = host_scopes {
                    SCOPE_STACK.with(|s| {
                        let old = std::mem::replace(&mut *s.borrow_mut(), scopes);
                        recycle_scope_stack(old);
                    });
                }
                refresh_scope_kind_cache();
                waiters
            });
            recycle_coroutine_stack(coro);
            wake_many(waiters);
        }
    }
}

fn clear_running(fid: FiberId) {
    let requeue = with_sched(|s| {
        if let Some(slot) = s.fibers.get_mut(&fid) {
            slot.running = false;
            let pending = slot.wake_pending;
            slot.wake_pending = false;
            pending
        } else {
            false
        }
    });
    if requeue {
        enqueue(fid);
    }
}

/// Publish join result under the sched lock. Caller must keep `val` GC-reachable
/// until this returns (fiber TLS roots or an already-published field).
/// Idempotent if `st.done` already (coro publishes before `CoroutineResult::Return`).
/// Does **not** clear `st.fiber` — the runner clears it when removing the slot.
fn publish_task_result(s: &mut SchedCore, task: TaskId, val: i64) -> VecDeque<Waiter> {
    let Some(st) = s.tasks.get_mut(&task) else {
        return VecDeque::new();
    };
    if st.cancelled {
        st.env = 0;
        return std::mem::take(&mut st.join_waiters);
    }
    if st.done {
        // Already published from the coro wrapper.
        return VecDeque::new();
    }
    st.result = Some(val);
    st.result_gc_pin = true;
    st.done = true;
    st.env = 0;
    // Mirror into the Task handle so mutator roots keep the value after unpin.
    // Handle may already have been swept — never touch a dangling pointer.
    if !st.handle.is_null() && crate::common::is_heap_payload(st.handle.as_raw()) {
        let handle = st.handle.as_raw();
        unsafe {
            *((handle as *mut i64).add(1)) = val;
        }
        let p = val as *mut u8;
        if crate::common::is_heap_payload(p) {
            unsafe { crate::gc::lumia_write_barrier(handle, 1, p) };
        }
    } else {
        st.handle = super::scan_ptrs::TaskHandlePtr::null();
    }
    std::mem::take(&mut st.join_waiters)
}

fn clear_thread_abi_handoff() {
    let tid = std::thread::current().id();
    with_sched(|s| {
        s.abi_handoff.remove(&tid);
    });
}

#[no_mangle]
pub extern "C" fn lumia_scheduler_drain() {
    let (workers, ios) = sched_pool_counts();
    // Nested drain from a running fiber must not resume others on this stack.
    if CURRENT_FIBER.with(|c| c.get()).is_some() {
        let mut idle = 0u32;
        loop {
            let busy = with_sched(|s| {
                s.pool_runners > 0
                    || (workers > 0 && s.kind_pending(SchedulerKind::Worker))
                    || (ios > 0 && s.kind_pending(SchedulerKind::Io))
            });
            if !busy {
                break;
            }
            sched_wait_while(
                |s| {
                    s.pool_runners > 0
                        || (workers > 0 && s.kind_pending(SchedulerKind::Worker))
                        || (ios > 0 && s.kind_pending(SchedulerKind::Io))
                },
                Duration::from_millis(5),
            );
            idle = idle.saturating_add(1);
            if idle > 500 {
                break;
            }
        }
        return;
    }

    // Drain local coop work; wait briefly for pool-owned queues to empty.
    let mut idle = 0u32;
    let tid = std::thread::current().id();
    loop {
        if let Some(fid) = pop_ready() {
            idle = 0;
            resume_fiber(fid);
            continue;
        }
        let busy = with_sched(|s| s.sched_busy());
        if !busy {
            break;
        }
        sched_wait_while(
            |s| {
                let local_idle = !s.queue_has_runnable_for(SchedulerKind::Default, tid)
                    && (workers > 0 || !s.queue_has_runnable_for(SchedulerKind::Worker, tid))
                    && (ios > 0 || !s.queue_has_runnable_for(SchedulerKind::Io, tid));
                local_idle && s.sched_busy()
            },
            Duration::from_millis(5),
        );
        idle = idle.saturating_add(1);
        if idle > 500 {
            break;
        }
    }
}
