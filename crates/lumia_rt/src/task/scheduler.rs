//! Cooperative scheduler + OS worker/io pools (BUILD §7.7 phase D).
//!
//! Default-kind fibers run on the calling thread. `Scheduler.worker` / `.io`
//! enqueue to process-shared queues drained by dedicated OS threads. Coroutines
//! are created on first resume (thread-local stack; corosensei is `!Send`).

use crate::common::{trap_abort, PAR_WORKER};
use crate::globals::{note_task_runtime_used, task_runtime_used_latched};
use crate::heap::with_heap;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

pub use super::sched_core::{
    with_sched, ChannelId, ChannelState, FiberId, SchedulerKind, ScopeId, TaskId, Waiter,
    SCHEDULER_IO, SCHEDULER_WORKER,
};
#[cfg(test)]
pub use super::sched_core::{FiberSlot, ScopeFrame, TaskState};

thread_local! {
    pub(super) static CURRENT_FIBER: Cell<Option<FiberId>> = const { Cell::new(None) };
    pub(super) static CURRENT_YIELDER: Cell<*const corosensei::Yielder<(), ()>> =
        const { Cell::new(std::ptr::null()) };
    /// Nesting of process-shared [`ScopeId`]s (parked with the fiber across OS threads).
    pub(super) static SCOPE_STACK: RefCell<Vec<ScopeId>> = const { RefCell::new(Vec::new()) };
    /// Recycled scope-stack buffers for spawn snapshots (fine-grained spawn tax).
    static SCOPE_STACK_FREELIST: RefCell<Vec<Vec<ScopeId>>> = const { RefCell::new(Vec::new()) };
    /// Recycled fiber coroutine stacks (`DefaultStack` / mmap). Home-thread TLS —
    /// matches coro `!Send` affinity; avoids cross-thread stack reuse.
    static FIBER_STACK_FREELIST: RefCell<Vec<corosensei::stack::DefaultStack>> =
        const { RefCell::new(Vec::new()) };
    /// Cached [`current_scope_kind`] — avoids a sched lock on every `pop_ready`.
    pub(super) static SCOPE_KIND_CACHE: Cell<u8> = const { Cell::new(0) };
}

const SCOPE_FREELIST_MAX: usize = 64;
const SCOPE_VEC_CAP_MAX: usize = 64;
const FIBER_STACK_FREELIST_MAX: usize = 32;

/// Copy the current TLS scope stack into a recycled buffer (or a fresh `Vec`).
pub(crate) fn snapshot_scope_stack() -> Vec<ScopeId> {
    SCOPE_STACK.with(|s| {
        let src = s.borrow();
        SCOPE_STACK_FREELIST.with(|fl| {
            let mut fl = fl.borrow_mut();
            let mut v = fl
                .pop()
                .unwrap_or_else(|| Vec::with_capacity(src.len().max(4)));
            v.clear();
            v.extend_from_slice(src.as_slice());
            v
        })
    })
}

/// Return a scope-stack buffer to the freelist (cleared).
pub(crate) fn recycle_scope_stack(mut v: Vec<ScopeId>) {
    v.clear();
    if v.capacity() > SCOPE_VEC_CAP_MAX {
        return;
    }
    SCOPE_STACK_FREELIST.with(|fl| {
        let mut fl = fl.borrow_mut();
        if fl.len() < SCOPE_FREELIST_MAX {
            fl.push(v);
        }
    });
}

/// Take a fiber coroutine stack from the TLS freelist, or mmap a fresh one.
pub(crate) fn take_fiber_stack() -> corosensei::stack::DefaultStack {
    if let Some(stack) = FIBER_STACK_FREELIST.with(|fl| fl.borrow_mut().pop()) {
        return stack;
    }
    corosensei::stack::DefaultStack::new(crate::globals::fiber_stack_bytes())
        .expect("lumia: fiber stack")
}

/// Return a finished fiber stack to the TLS freelist (dropped if freelist is full).
pub(crate) fn recycle_fiber_stack(stack: corosensei::stack::DefaultStack) {
    FIBER_STACK_FREELIST.with(|fl| {
        let mut fl = fl.borrow_mut();
        if fl.len() < FIBER_STACK_FREELIST_MAX {
            fl.push(stack);
        }
    });
}

/// Reclaim a coroutine's stack into the freelist.
///
/// If the coro is not yet [`done`](corosensei::Coroutine::done), marks it done via
/// `force_reset` (caller must uphold the no-`Drop`-locals-on-stack contract —
/// never-started or RT `suspend_current` only).
pub(crate) fn recycle_coroutine_stack(mut coro: corosensei::Coroutine<(), (), i64>) {
    if !coro.done() {
        // SAFETY: never-started has no stack objects; suspended fibers only yield
        // at RT `suspend_current` (no Rust `Drop` locals across that point).
        unsafe {
            coro.force_reset();
        }
    }
    recycle_fiber_stack(coro.into_stack());
}

fn scope_kind_to_u8(k: SchedulerKind) -> u8 {
    match k {
        SchedulerKind::Default => 0,
        SchedulerKind::Worker => 1,
        SchedulerKind::Io => 2,
    }
}

fn scope_kind_from_u8(v: u8) -> SchedulerKind {
    match v {
        1 => SchedulerKind::Worker,
        2 => SchedulerKind::Io,
        _ => SchedulerKind::Default,
    }
}

pub(super) fn refresh_scope_kind_cache() {
    let k = SCOPE_STACK
        .with(|s| s.borrow().last().copied())
        .and_then(|id| with_sched(|s| s.scopes.get(&id).map(|f| f.kind)))
        .unwrap_or(SchedulerKind::Default);
    SCOPE_KIND_CACHE.with(|c| c.set(scope_kind_to_u8(k)));
}

/// Serialize tests that touch the process-global scheduler / TLS scope stack.
/// Parallel `cargo test` otherwise interleaves stress/fiber cases on one `SCHED`.
#[cfg(test)]
pub(crate) fn sched_test_guard() -> impl Drop {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    CURRENT_FIBER.with(|c| c.set(None));
    SCOPE_STACK.with(|s| s.borrow_mut().clear());
    refresh_scope_kind_cache();
    crate::common::PAR_WORKER.with(|c| c.set(false));
    g
}

pub(super) fn alloc_id() -> u64 {
    with_sched(|s| {
        let id = s.next_id;
        s.next_id = id + 1;
        id
    })
}

pub(super) fn assert_task_api_allowed() {
    if PAR_WORKER.get() {
        trap_abort("lumia: task/channel API on parallel map worker");
    }
    note_task_runtime_used();
}

pub fn task_runtime_active() -> bool {
    if CURRENT_FIBER.with(|c| c.get()).is_some() {
        return true;
    }
    if SCOPE_STACK.with(|s| !s.borrow().is_empty()) {
        return true;
    }
    if !task_runtime_used_latched() {
        return false;
    }
    with_sched(|s| {
        s.ready_nonempty()
            || s.pool_runners > 0
            || s.fibers
                .values()
                .any(|slot| slot.has_coro || slot.pending.is_some())
    })
}

pub(super) fn current_scope_kind() -> SchedulerKind {
    SCOPE_KIND_CACHE.with(|c| scope_kind_from_u8(c.get()))
}

#[cfg(test)]
pub(super) use super::sched_env::{reload_sched_env_for_test, sched_pool_counts};

/// Spin until no live fibers and pool runners are idle (test isolation).
#[cfg(test)]
pub(crate) fn await_sched_quiescent_for_test() {
    for _ in 0..20_000 {
        let idle = with_sched(|s| s.fibers.is_empty() && s.pool_runners == 0);
        if idle {
            return;
        }
        lumia_scheduler_drain();
        std::thread::yield_now();
    }
    let (n, runners) = with_sched(|s| (s.fibers.len(), s.pool_runners));
    panic!("lumia: sched not quiescent after wait (fibers={n}, pool_runners={runners})");
}

/// Process-global sched tables are shared across libtest threads; serialize
/// unit tests that insert synthetic fibers / clear ready queues.
#[cfg(test)]
static SCHED_UNIT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(super) fn with_sched_unit_test<R>(f: impl FnOnce() -> R) -> R {
    let _guard = SCHED_UNIT_TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    f()
}

pub(super) use super::sched_queue::{enqueue, pop_ready_kind, wake, wake_many};
#[cfg(test)]
pub(super) use super::sched_queue::pop_ready;

pub use super::sched_cancel::{cancel_all_scopes, cancel_scope_children};
pub(super) use super::sched_cancel::{
    check_current_not_cancelled, current_fiber_cancelled_locked,
};
pub use super::sched_roots::snapshot_sched_gc_roots;
pub use super::sched_resume::lumia_scheduler_drain;
pub(crate) use super::sched_resume::resume_fiber;
pub(super) use super::sched_resume::{current_waiter, park_until, suspend_current};

/// TYPE_TASK unmarked: clear dangling `TaskState.handle` (heap → sched lock order).
pub fn on_task_handle_swept(task: TaskId) {
    with_sched(|s| {
        let reap = if let Some(st) = s.tasks.get_mut(&task) {
            st.handle = super::scan_ptrs::TaskHandlePtr::null();
            st.done && st.fiber.is_none() && st.join_waiters.is_empty() && !st.result_gc_pin
        } else {
            false
        };
        if reap {
            s.tasks.remove(&task);
        }
    });
}

/// TYPE_CHANNEL unmarked: drop orphan channel if no waiters remain.
pub fn on_channel_handle_swept(id: ChannelId) {
    with_sched(|s| {
        let reap = s.channels.get(&id).is_some_and(|ch| {
            ch.send_waiters.is_empty() && ch.recv_waiters.is_empty()
        });
        if reap {
            s.channels.remove(&id);
        }
    });
}

/// Drop finished task metadata after join when nothing else references it.
pub(crate) fn try_reap_task(task: TaskId) {
    with_sched(|s| {
        let reap = s.tasks.get(&task).is_some_and(|st| {
            st.done
                && st.fiber.is_none()
                && st.join_waiters.is_empty()
                && !st.result_gc_pin
                && st.handle.is_null()
        });
        if reap {
            s.tasks.remove(&task);
        }
    });
}

/// Pin an ABI return / handoff value for GC until overwritten (codegen epilogue).
#[no_mangle]
pub extern "C" fn lumia_abi_handoff_set(val: i64) {
    let tid = std::thread::current().id();
    with_heap(|h| {
        if h.full_marking {
            crate::gc::mark_value(val);
        }
        with_sched(|s| {
            s.abi_handoff.insert(tid, val);
        });
    });
}

/// Push a channel/join waiter, coalescing duplicate `Main` / same fiber.
pub(super) fn push_waiter_unique(q: &mut VecDeque<Waiter>, w: Waiter) {
    let dup = match w {
        Waiter::Main => q.iter().any(|x| matches!(x, Waiter::Main)),
        Waiter::Fiber(fid) => q
            .iter()
            .any(|x| matches!(x, Waiter::Fiber(f) if *f == fid)),
    };
    if !dup {
        q.push_back(w);
    }
}

#[no_mangle]
pub extern "C" fn lumia_scheduler_kind(kind: i64) -> i64 {
    match SchedulerKind::from_i64(kind) {
        SchedulerKind::Worker => SCHEDULER_WORKER,
        SchedulerKind::Io => SCHEDULER_IO,
        SchedulerKind::Default => 0,
    }
}


#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
