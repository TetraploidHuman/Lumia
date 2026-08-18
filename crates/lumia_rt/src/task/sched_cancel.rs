//! Task/fiber cancellation and coroutine reclaim.

use crate::common::{trap_abort, CALL_STACK};
use crate::heap::with_heap;
use crate::mutator::take_local_roots;
use corosensei::Coroutine;
use rustc_hash::FxHashSet;
use std::collections::VecDeque;

use super::home_coro;
use super::sched_core::{sched_notify, with_sched, FiberId, SchedCore, TaskId, Waiter};
use super::sched_queue::{enqueue, wake_many};
use super::sched_roots::discard_parked_scope_stack;
use super::scheduler::{recycle_scope_stack, CURRENT_FIBER, SCOPE_KIND_CACHE, SCOPE_STACK};

/// Trap hook: cancel every not-yet-finished task in the process (all OS threads).
pub fn cancel_all_scopes() {
    let tasks: Vec<TaskId> = with_sched(|s| {
        s.abi_handoff.clear();
        s.tasks
            .iter()
            .filter(|(_, st)| !st.done)
            .map(|(&id, _)| id)
            .collect()
    });
    cancel_tasks(&tasks);
}

pub fn cancel_scope_children() {
    let self_task = CURRENT_FIBER.with(|c| {
        c.get()
            .and_then(|fid| with_sched(|s| s.fibers.get(&fid).map(|slot| slot.task)))
    });
    let sid = SCOPE_STACK.with(|s| s.borrow().last().copied());
    let children = sid
        .and_then(|id| with_sched(|s| s.scopes.get(&id).map(|f| f.children.clone())))
        .unwrap_or_default();
    let targets: Vec<TaskId> = match self_task {
        Some(me) => children.into_iter().filter(|&t| t != me).collect(),
        None => children,
    };
    cancel_tasks(&targets);
}

fn cancel_tasks(tasks: &[TaskId]) {
    let (fibers, waiters) = with_sched(|s| {
        let mut fibers = Vec::new();
        let mut waiters = VecDeque::new();
        for &task in tasks {
            let Some(st) = s.tasks.get_mut(&task) else {
                continue;
            };
            if st.done && !st.cancelled {
                continue;
            }
            st.cancelled = true;
            st.done = true;
            // Keep `env` until the fiber slot (and any `PendingSpawn`) is removed
            // so snapshot_sched_gc_roots still sees the spawn argument.
            if let Some(fid) = st.fiber.take() {
                fibers.push(fid);
            }
            waiters.append(&mut st.join_waiters);
        }
        let set: FxHashSet<FiberId> = fibers.iter().copied().collect();
        for ch in s.channels.values_mut() {
            ch.send_waiters
                .retain(|w| !matches!(w, Waiter::Fiber(fid) if set.contains(fid)));
            ch.recv_waiters
                .retain(|w| !matches!(w, Waiter::Fiber(fid) if set.contains(fid)));
        }
        (fibers, waiters)
    });
    wake_many(waiters);
    for fid in fibers {
        abandon_cancelled_fiber(fid);
    }
}

fn abandon_cancelled_fiber(fid: FiberId) {
    let tid = std::thread::current().id();
    enum Abandon {
        Dispose(Coroutine<(), (), i64>),
        RequeueHome,
        None,
    }
    let action = with_sched(|s| {
        s.retain_ready(|x| x != fid);
        let Some(slot) = s.fibers.get_mut(&fid) else {
            s.parked_roots.remove(&fid);
            s.parked_call_stacks.remove(&fid);
            discard_parked_scope_stack(s, fid);
            return Abandon::None;
        };
        if slot.running {
            // Runner still owns the coro; reclaim on their Yield/Return/cancel exit.
            slot.on_ready = false;
            slot.wake_pending = false;
            slot.reclaim_home = true;
            return Abandon::None;
        }
        // Coroutine stacks are !Send — only dispose on the home OS thread.
        if let Some(home) = slot.home {
            if home != tid {
                slot.reclaim_home = true;
                slot.on_ready = false;
                slot.wake_pending = false;
                return Abandon::RequeueHome;
            }
        }
        let mut slot = s.fibers.remove(&fid).expect("fiber");
        let had_coro = slot.has_coro;
        if let Some(st) = s.tasks.get_mut(&slot.task) {
            st.env = 0;
            st.fiber = None;
        }
        let _ = slot.pending.take();
        s.parked_roots.remove(&fid);
        s.parked_call_stacks.remove(&fid);
        discard_parked_scope_stack(s, fid);
        // Home-thread only: take parked stack from TLS after clearing the flag.
        match had_coro.then(|| home_coro::take(fid)).flatten() {
            Some(c) => Abandon::Dispose(c),
            None => Abandon::None,
        }
    });
    match action {
        Abandon::Dispose(coro) => dispose_cancelled_coroutine(coro),
        Abandon::RequeueHome => {
            // Home thread resume sees cancelled / reclaim_home and disposes.
            enqueue(fid);
        }
        Abandon::None => {}
    }
    sched_notify();
}

/// Drop TLS / parked roots that point into `fid`'s stack before freeing the coro.
pub(super) fn scrub_roots_before_coro_drop(fid: FiberId) {
    with_heap(|_| {
        let _ = take_local_roots();
        CALL_STACK.with(|s| s.borrow_mut().clear());
        SCOPE_STACK.with(|s| {
            let old = std::mem::take(&mut *s.borrow_mut());
            recycle_scope_stack(old);
        });
        SCOPE_KIND_CACHE.with(|c| c.set(0));
        with_sched(|s| {
            s.parked_roots.remove(&fid);
            s.parked_call_stacks.remove(&fid);
            discard_parked_scope_stack(s, fid);
        });
    });
}

/// Reclaim a cancelled fiber coroutine.
///
/// Contract: coro was taken from home-thread TLS ([`super::home_coro`]) on the
/// **home** OS thread (or never started / `home` unset). The stack is either
/// never-started, already done, or suspended at [`super::sched_resume::suspend_current`]
/// (RT-only yield — no Rust `Drop` locals across that point). Prefer `force_reset`
/// over `force_unwind` (would cross `extern "C"` TaskFn). Stack returns to the
/// TLS freelist via [`super::scheduler::recycle_coroutine_stack`].
pub(super) fn dispose_cancelled_coroutine(coro: Coroutine<(), (), i64>) {
    super::scheduler::recycle_coroutine_stack(coro);
}

pub(super) fn check_current_not_cancelled() {
    let Some(fid) = CURRENT_FIBER.with(|c| c.get()) else {
        return;
    };
    let cancelled = with_sched(|s| {
        s.fibers
            .get(&fid)
            .and_then(|slot| s.tasks.get(&slot.task).map(|t| t.cancelled))
    });
    if cancelled == Some(true) {
        trap_abort("lumia: task cancelled");
    }
}

/// True if the current fiber's task is already cancelled (caller holds sched lock).
pub(super) fn current_fiber_cancelled_locked(s: &SchedCore) -> bool {
    CURRENT_FIBER.with(|c| {
        c.get()
            .and_then(|fid| s.fibers.get(&fid))
            .and_then(|slot| s.tasks.get(&slot.task))
            .is_some_and(|t| t.cancelled)
    })
}
