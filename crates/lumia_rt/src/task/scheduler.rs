//! Cooperative scheduler + OS worker/io pools (BUILD §7.7 phase D).
//!
//! Default-kind fibers run on the calling thread. `Scheduler.worker` / `.io`
//! enqueue to process-shared queues drained by dedicated OS threads. Coroutines
//! are created on first resume (thread-local stack; corosensei is `!Send`).

use crate::common::{trap_abort, CALL_STACK, PAR_WORKER};
use crate::heap::with_heap;
use crate::mutator::{ensure_mutator_registered, set_local_roots, take_local_roots};
use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult};
use rustc_hash::FxHashSet;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};
use std::time::Duration;

pub use super::sched_core::{
    sched_notify, sched_wait_while, with_sched, ChannelId, ChannelState, FiberId, FiberSlot,
    PendingSpawn, SchedCore, SchedulerKind, ScopeFrame, ScopeId, TaskId, TaskState, Waiter,
    SCHEDULER_IO, SCHEDULER_WORKER,
};

/// Latched when Task/Channel APIs run; avoids fiber-table scans on every `par_map`
/// in programs that never use the scheduler.
static TASK_RUNTIME_USED: AtomicBool = AtomicBool::new(false);

pub(super) fn note_task_runtime_used() {
    TASK_RUNTIME_USED.store(true, Ordering::Release);
}

thread_local! {
    pub(super) static CURRENT_FIBER: Cell<Option<FiberId>> = const { Cell::new(None) };
    pub(super) static CURRENT_YIELDER: Cell<*const corosensei::Yielder<(), ()>> =
        const { Cell::new(std::ptr::null()) };
    /// Nesting of process-shared [`ScopeId`]s (parked with the fiber across OS threads).
    pub(super) static SCOPE_STACK: RefCell<Vec<ScopeId>> = const { RefCell::new(Vec::new()) };
    /// Cached [`current_scope_kind`] — avoids a sched lock on every `pop_ready`.
    static SCOPE_KIND_CACHE: Cell<u8> = const { Cell::new(0) };
}

/// Fiber coroutine stack size (bytes). Default 64KiB (override with `LUMIA_FIBER_STACK_KB`).
fn fiber_stack_bytes() -> usize {
    static CACHED: Mutex<Option<usize>> = Mutex::new(None);
    let mut g = CACHED.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(n) = *g {
        return n;
    }
    let default = 64 * 1024;
    let n = match std::env::var("LUMIA_FIBER_STACK_KB") {
        Ok(v) => v
            .trim()
            .parse::<usize>()
            .ok()
            .map(|kb| kb.saturating_mul(1024).max(16 * 1024))
            .unwrap_or(default),
        Err(_) => default,
    };
    *g = Some(n);
    n
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
    if !TASK_RUNTIME_USED.load(Ordering::Acquire) {
        return false;
    }
    with_sched(|s| {
        s.ready_nonempty()
            || s.pool_runners > 0
            || s.fibers
                .values()
                .any(|slot| slot.coro.is_some() || slot.pending.is_some())
    })
}

pub(super) fn current_scope_kind() -> SchedulerKind {
    SCOPE_KIND_CACHE.with(|c| scope_kind_from_u8(c.get()))
}

/// Cached `LUMIA_SCHED_WORKERS` / `LUMIA_SCHED_IO` (process lifetime; tests may reload).
static SCHED_ENV: Mutex<Option<(usize, usize)>> = Mutex::new(None);

fn sched_pool_counts() -> (usize, usize) {
    let mut g = SCHED_ENV.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(c) = *g {
        return c;
    }
    let default = default_pool_size();
    let c = (
        parse_env_usize("LUMIA_SCHED_WORKERS", default),
        parse_env_usize("LUMIA_SCHED_IO", default),
    );
    *g = Some(c);
    c
}

fn worker_threads() -> usize {
    sched_pool_counts().0
}

fn io_threads() -> usize {
    sched_pool_counts().1
}

/// Default OS-thread pool size when env is unset: host `available_parallelism`, else 1.
/// Tests may pin with `LUMIA_SCHED_WORKERS=0|1` (0 = cooperative / no dedicated pool).
fn default_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn parse_env_usize(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Ok(v) => v.trim().parse().unwrap_or(default),
        Err(_) => default,
    }
}

#[cfg(test)]
fn reload_sched_env_for_test() {
    *SCHED_ENV.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Pop a ready fiber that may run on this OS thread (`home` unset or matching).
/// Claims `running` under the same lock so an enqueue in the pop→resume gap
/// sets `wake_pending` instead of a stale ready-queue entry.
fn try_pop_affinity(
    s: &mut SchedCore,
    kind: SchedulerKind,
    tid: std::thread::ThreadId,
) -> Option<FiberId> {
    // 1) Own home queue first (no foreign-home rotation).
    if s.ready_home.contains_key(&tid) {
        let n = s.ready_home.get(&tid).map(|q| q.len()).unwrap_or(0);
        for _ in 0..n {
            let Some(fid) = s.ready_home.get_mut(&tid).and_then(|q| q.pop_front()) else {
                break;
            };
            let ok = s.fibers.get(&fid).map(|slot| {
                !slot.running && slot.kind == kind && slot.home == Some(tid)
            });
            match ok {
                Some(true) => {
                    if let Some(slot) = s.fibers.get_mut(&fid) {
                        slot.on_ready = false;
                        slot.running = true;
                    }
                    if s.ready_home.get(&tid).is_some_and(|q| q.is_empty()) {
                        s.ready_home.remove(&tid);
                    }
                    return Some(fid);
                }
                Some(false) => {
                    if let Some(q) = s.ready_home.get_mut(&tid) {
                        q.push_back(fid);
                    }
                }
                None => {}
            }
        }
        if s.ready_home.get(&tid).is_some_and(|q| q.is_empty()) {
            s.ready_home.remove(&tid);
        }
    }

    // 2) Stealable shared queue for this kind (`home == None`, or rare home==tid).
    let n = s.ready_queue(kind).len();
    for _ in 0..n {
        let Some(fid) = s.ready_queue_mut(kind).pop_front() else {
            break;
        };
        let ok = s.fibers.get(&fid).map(|slot| {
            !slot.running && (slot.home.is_none() || slot.home == Some(tid))
        });
        match ok {
            Some(true) => {
                if let Some(slot) = s.fibers.get_mut(&fid) {
                    slot.on_ready = false;
                    slot.running = true;
                }
                return Some(fid);
            }
            Some(false) => s.ready_queue_mut(kind).push_back(fid),
            None => {}
        }
    }
    None
}

/// Pop ready fiber for the **local** mutator (main / default).
/// When a dedicated OS pool owns a kind, never steal that kind's work.
/// Kinds with `LUMIA_SCHED_*=0` stay cooperative and may be drained here.
pub(super) fn pop_ready() -> Option<FiberId> {
    let tid = std::thread::current().id();
    let prefer = current_scope_kind();
    let (workers, ios) = sched_pool_counts();
    if workers > 0 || ios > 0 {
        return with_sched(|s| {
            if let Some(fid) = try_pop_affinity(s, SchedulerKind::Default, tid) {
                return Some(fid);
            }
            // Coop fallback for kinds that have no dedicated pool threads.
            if workers == 0 {
                if let Some(fid) = try_pop_affinity(s, SchedulerKind::Worker, tid) {
                    return Some(fid);
                }
            }
            if ios == 0 {
                if let Some(fid) = try_pop_affinity(s, SchedulerKind::Io, tid) {
                    return Some(fid);
                }
            }
            None
        });
    }
    let order = match prefer {
        SchedulerKind::Io => [
            SchedulerKind::Io,
            SchedulerKind::Default,
            SchedulerKind::Worker,
        ],
        SchedulerKind::Worker => [
            SchedulerKind::Worker,
            SchedulerKind::Default,
            SchedulerKind::Io,
        ],
        SchedulerKind::Default => [
            SchedulerKind::Default,
            SchedulerKind::Worker,
            SchedulerKind::Io,
        ],
    };
    with_sched(|s| {
        for kind in order {
            if let Some(fid) = try_pop_affinity(s, kind, tid) {
                return Some(fid);
            }
        }
        None
    })
}

fn pop_ready_kind(kind: SchedulerKind) -> Option<FiberId> {
    let tid = std::thread::current().id();
    with_sched(|s| try_pop_affinity(s, kind, tid))
}

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

pub(super) fn save_fiber_roots(fid: FiberId) {
    // Hold heap across TLS take + sched publish so GC cannot miss roots in flight.
    with_heap(|_| {
        let roots = take_local_roots();
        let frames = CALL_STACK.with(|s| std::mem::take(&mut *s.borrow_mut()));
        let scopes = SCOPE_STACK.with(|s| std::mem::take(&mut *s.borrow_mut()));
        SCOPE_KIND_CACHE.with(|c| c.set(0));
        with_sched(|s| {
            if roots.is_empty() {
                s.parked_roots.remove(&fid);
            } else {
                s.parked_roots.insert(fid, roots);
            }
            if frames.is_empty() {
                s.parked_call_stacks.remove(&fid);
            } else {
                s.parked_call_stacks.insert(fid, frames);
            }
            if scopes.is_empty() {
                s.parked_scope_stacks.remove(&fid);
            } else {
                s.parked_scope_stacks.insert(fid, scopes);
            }
        });
    });
}

pub(super) fn load_fiber_roots(fid: FiberId) {
    with_heap(|_| {
        let (roots, frames, scopes) = with_sched(|s| {
            (
                s.parked_roots.remove(&fid).unwrap_or_default(),
                s.parked_call_stacks.remove(&fid).unwrap_or_default(),
                s.parked_scope_stacks.remove(&fid).unwrap_or_default(),
            )
        });
        set_local_roots(roots);
        CALL_STACK.with(|s| *s.borrow_mut() = frames);
        SCOPE_STACK.with(|s| *s.borrow_mut() = scopes);
        refresh_scope_kind_cache();
    });
}

/// Queue `fid` if needed. Returns `Some(kind)` only when newly pushed onto a ready queue.
fn enqueue_inner(fid: FiberId) -> Option<SchedulerKind> {
    with_sched(|s| {
        let Some(slot) = s.fibers.get_mut(&fid) else {
            return None;
        };
        if slot.running {
            slot.wake_pending = true;
            return None;
        }
        if slot.on_ready {
            return None;
        }
        let kind = slot.kind;
        let home = slot.home;
        slot.on_ready = true;
        s.push_ready(fid, kind, home);
        Some(kind)
    })
}

pub(super) fn enqueue(fid: FiberId) {
    if let Some(kind) = enqueue_inner(fid) {
        ensure_pool_for_kind(kind);
        sched_notify();
    }
}

pub(super) fn wake(waiter: Waiter) {
    match waiter {
        Waiter::Fiber(fid) => enqueue(fid),
        Waiter::Main => sched_notify(),
    }
}

/// Wake many waiters under one notify (and at most one pool ensure per kind).
pub(super) fn wake_many(waiters: impl IntoIterator<Item = Waiter>) {
    let mut notify = false;
    let mut need_worker = false;
    let mut need_io = false;
    for w in waiters {
        match w {
            Waiter::Main => notify = true,
            Waiter::Fiber(fid) => {
                if let Some(kind) = enqueue_inner(fid) {
                    notify = true;
                    match kind {
                        SchedulerKind::Worker => need_worker = true,
                        SchedulerKind::Io => need_io = true,
                        SchedulerKind::Default => {}
                    }
                }
            }
        }
    }
    if need_worker {
        ensure_pool_for_kind(SchedulerKind::Worker);
    }
    if need_io {
        ensure_pool_for_kind(SchedulerKind::Io);
    }
    if notify {
        sched_notify();
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

pub(super) fn resume_fiber(fid: FiberId) {
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
        let Some(slot) = s.fibers.get_mut(&fid) else {
            return None;
        };
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
        let cancelled =
            s.tasks.get(&task).is_some_and(|t| t.cancelled) || slot.reclaim_home;
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
                s.host_roots.insert(tid, roots);
                s.host_call_stacks.insert(tid, frames);
                s.host_scope_stacks.insert(tid, scopes);
            });
        });
    }
    load_fiber_roots(fid);
    CURRENT_FIBER.with(|c| c.set(Some(fid)));

    if cancelled {
        let coro = with_sched(|s| {
            let coro = s.fibers.remove(&fid).and_then(|mut slot| {
                if let Some(st) = s.tasks.get_mut(&slot.task) {
                    st.env = 0;
                }
                let _ = slot.pending.take();
                slot.coro.take()
            });
            s.parked_roots.remove(&fid);
            s.parked_call_stacks.remove(&fid);
            s.parked_scope_stacks.remove(&fid);
            coro
        });
        CURRENT_FIBER.with(|c| c.set(None));
        CURRENT_YIELDER.with(|c| c.set(std::ptr::null()));
        scrub_roots_before_coro_drop(fid);
        if let Some(c) = coro {
            dispose_cancelled_coroutine(c);
        }
        restore_host_roots();
        return;
    }

    // Allocate fiber stack *outside* the sched lock (mmap is slow).
    let needs_stack = with_sched(|s| {
        s.fibers.get(&fid).is_some_and(|slot| {
            slot.coro.is_none()
                && slot.pending.is_some()
                && !s.tasks.get(&slot.task).is_some_and(|t| t.cancelled)
        })
    });
    let mut pre_stack = if needs_stack {
        Some(DefaultStack::new(fiber_stack_bytes()).expect("lumia: fiber stack"))
    } else {
        None
    };

    // Build (if needed) + take coro + yielder; re-check cancel under the same lock.
    let taken = with_sched(|s| {
        let cancelled_now = s.tasks.get(&task_id).is_some_and(|t| t.cancelled);
        if cancelled_now {
            return Err(());
        }
        let Some(slot) = s.fibers.get_mut(&fid) else {
            return Ok(None);
        };
        if slot.coro.is_none() {
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
            slot.coro = Some(Coroutine::with_stack(stack, move |yielder, ()| {
                let yptr = yielder as *const _;
                CURRENT_YIELDER.with(|c| c.set(yptr));
                CURRENT_FIBER.with(|c| c.set(Some(fid)));
                with_sched(|s| {
                    if let Some(slot) = s.fibers.get(&fid) {
                        slot.yielder.set(yptr);
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
            }));
        } else if slot.home.is_none() {
            slot.home = Some(tid);
        }
        let y = slot.yielder.get();
        Ok(slot.coro.take().map(|c| (c, y)))
    });
    drop(pre_stack);
    let taken = match taken {
        Err(()) => {
            let coro = with_sched(|s| {
                let coro = s.fibers.remove(&fid).and_then(|mut slot| {
                    if let Some(st) = s.tasks.get_mut(&slot.task) {
                        st.env = 0;
                    }
                    let _ = slot.pending.take();
                    slot.coro.take()
                });
                s.parked_roots.remove(&fid);
                s.parked_call_stacks.remove(&fid);
                s.parked_scope_stacks.remove(&fid);
                coro
            });
            CURRENT_FIBER.with(|c| c.set(None));
            CURRENT_YIELDER.with(|c| c.set(std::ptr::null()));
            scrub_roots_before_coro_drop(fid);
            if let Some(c) = coro {
                dispose_cancelled_coroutine(c);
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
                let cancelled = s
                    .fibers
                    .get(&fid)
                    .is_some_and(|slot| {
                        slot.reclaim_home
                            || s.tasks.get(&slot.task).is_some_and(|t| t.cancelled)
                    })
                    || !s.fibers.contains_key(&fid);
                if cancelled {
                    if let Some(mut slot) = s.fibers.remove(&fid) {
                        slot.running = false;
                        let _ = slot.coro.take();
                        if let Some(st) = s.tasks.get_mut(&slot.task) {
                            st.env = 0;
                        }
                    }
                    s.parked_roots.remove(&fid);
                    s.parked_call_stacks.remove(&fid);
                    s.parked_scope_stacks.remove(&fid);
                    YieldOut::Dispose(coro)
                } else if let Some(slot) = s.fibers.get_mut(&fid) {
                    slot.coro = Some(coro);
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
                    s.parked_scope_stacks.remove(&fid);
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
                    s.parked_scope_stacks.remove(&fid);
                    let _ = s.fibers.remove(&fid);
                    if let Some(st) = s.tasks.get_mut(&task_id) {
                        st.fiber = None;
                    }
                    waiters
                });
                let _ = take_local_roots();
                SCOPE_STACK.with(|s| s.borrow_mut().clear());
                let (host_roots, host_frames, host_scopes) = with_sched(|s| {
                    (
                        s.host_roots.remove(&tid),
                        s.host_call_stacks.remove(&tid),
                        s.host_scope_stacks.remove(&tid),
                    )
                });
                if let Some(roots) = host_roots {
                    set_local_roots(roots);
                }
                if let Some(frames) = host_frames {
                    CALL_STACK.with(|s| *s.borrow_mut() = frames);
                }
                if let Some(scopes) = host_scopes {
                    SCOPE_STACK.with(|s| *s.borrow_mut() = scopes);
                }
                refresh_scope_kind_cache();
                waiters
            });
            drop(coro);
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

fn restore_host_roots() {
    let tid = std::thread::current().id();
    with_heap(|_| {
        let (roots, frames, scopes) = with_sched(|s| {
            (
                s.host_roots.remove(&tid),
                s.host_call_stacks.remove(&tid),
                s.host_scope_stacks.remove(&tid),
            )
        });
        if let Some(roots) = roots {
            set_local_roots(roots);
        }
        if let Some(frames) = frames {
            CALL_STACK.with(|s| *s.borrow_mut() = frames);
        }
        if let Some(scopes) = scopes {
            SCOPE_STACK.with(|s| *s.borrow_mut() = scopes);
        } else {
            SCOPE_STACK.with(|s| s.borrow_mut().clear());
        }
        refresh_scope_kind_cache();
    });
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
    if !st.handle.is_null() && crate::common::is_heap_payload(st.handle) {
        let handle = st.handle;
        unsafe {
            *((handle as *mut i64).add(1)) = val;
        }
        let p = val as *mut u8;
        if crate::common::is_heap_payload(p) {
            crate::gc::lumia_write_barrier(handle, 1, p);
        }
    } else {
        st.handle = std::ptr::null_mut();
    }
    std::mem::take(&mut st.join_waiters)
}

fn clear_thread_abi_handoff() {
    let tid = std::thread::current().id();
    with_sched(|s| {
        s.abi_handoff.remove(&tid);
    });
}

/// TYPE_TASK unmarked: clear dangling `TaskState.handle` (heap → sched lock order).
pub fn on_task_handle_swept(task: TaskId) {
    with_sched(|s| {
        let reap = if let Some(st) = s.tasks.get_mut(&task) {
            st.handle = std::ptr::null_mut();
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
            s.parked_scope_stacks.remove(&fid);
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
        let coro = slot.coro.take();
        if let Some(st) = s.tasks.get_mut(&slot.task) {
            st.env = 0;
            st.fiber = None;
        }
        let _ = slot.pending.take();
        s.parked_roots.remove(&fid);
        s.parked_call_stacks.remove(&fid);
        s.parked_scope_stacks.remove(&fid);
        match coro {
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
fn scrub_roots_before_coro_drop(fid: FiberId) {
    with_heap(|_| {
        let _ = take_local_roots();
        CALL_STACK.with(|s| s.borrow_mut().clear());
        SCOPE_STACK.with(|s| s.borrow_mut().clear());
        SCOPE_KIND_CACHE.with(|c| c.set(0));
        with_sched(|s| {
            s.parked_roots.remove(&fid);
            s.parked_call_stacks.remove(&fid);
            s.parked_scope_stacks.remove(&fid);
        });
    });
}

/// Reclaim a cancelled fiber coroutine.
///
/// Contract: coro was taken from the fiber slot under the sched lock on the
/// **home** OS thread (or never started / `home` unset). The stack is either
/// never-started, already done, or suspended at [`suspend_current`] (RT-only
/// yield — no Rust `Drop` locals across that point). Prefer `force_reset` over
/// `force_unwind` (would cross `extern "C"` TaskFn).
fn dispose_cancelled_coroutine(mut coro: Coroutine<(), (), i64>) {
    if !coro.started() || coro.done() {
        drop(coro);
        return;
    }
    debug_assert!(
        coro.started() && !coro.done(),
        "lumia: force_reset only for started+suspended fibers"
    );
    // SAFETY: RT suspend points hold no Rust `Drop` locals (see suspend_current).
    unsafe {
        coro.force_reset();
    }
    drop(coro);
}

pub(super) fn check_current_not_cancelled() {
    let Some(fid) = CURRENT_FIBER.with(|c| c.get()) else {
        return;
    };
    let cancelled = with_sched(|s| {
        s.fibers.get(&fid).and_then(|slot| {
            s.tasks.get(&slot.task).map(|t| t.cancelled)
        })
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

/// Snapshot scheduler-owned GC roots.
///
/// Prefer calling while already holding the heap lock (`with_heap`), nesting
/// `with_sched` underneath (heap → sched). Park/unpark uses the same order.
///
/// Returns `(rooted_payload_bits, task_channel_values)`. Parked/host slots are
/// dereferenced under the sched lock so later mark does not touch fiber stacks.
pub fn snapshot_sched_gc_roots() -> (Vec<i64>, Vec<i64>) {
    with_sched(|s| {
        let mut rooted = Vec::new();
        for roots in s.parked_roots.values().chain(s.host_roots.values()) {
            for &slot in roots {
                // Safety: slot addresses are live while the owning fiber/host is
                // parked in these maps; resume removes entries before mutating TLS.
                let p = unsafe { *slot };
                rooted.push(p as i64);
            }
        }
        let mut vals = Vec::new();
        for ch in s.channels.values() {
            vals.extend(ch.buf.iter().copied());
            // Channel handles live only via mutator roots (not immortal-pinned here).
        }
        for st in s.tasks.values() {
            if st.env != 0 {
                vals.push(st.env);
            }
            // Pin result only until the first successful join; afterwards the
            // TYPE_TASK handle word (scanned from mutator roots) owns it.
            if st.result_gc_pin {
                if let Some(v) = st.result {
                    vals.push(v);
                }
            }
            // Do not immortal-pin `st.handle` — live handles are mutator roots.
        }
        for slot in s.fibers.values() {
            if let Some(PendingSpawn::Unary(_, e)) = slot.pending {
                if e != 0 {
                    vals.push(e);
                }
            }
        }
        vals.extend(s.abi_handoff.values().copied());
        (rooted, vals)
    })
}

fn ensure_pool_for_kind(kind: SchedulerKind) {
    match kind {
        SchedulerKind::Worker if worker_threads() > 0 => start_pool_once(),
        SchedulerKind::Io if io_threads() > 0 => start_pool_once(),
        _ => {}
    }
}

fn start_pool_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for _ in 0..worker_threads() {
            std::thread::Builder::new()
                .name("lumia-sched-worker".into())
                .spawn(|| pool_thread_main(SchedulerKind::Worker))
                .expect("lumia: spawn worker thread");
        }
        for _ in 0..io_threads() {
            std::thread::Builder::new()
                .name("lumia-sched-io".into())
                .spawn(|| pool_thread_main(SchedulerKind::Io))
                .expect("lumia: spawn io thread");
        }
    });
}

fn pool_thread_main(kind: SchedulerKind) {
    ensure_mutator_registered();
    crate::task::ensure_trap_hook();
    let tid = std::thread::current().id();
    loop {
        let fid = loop {
            if let Some(fid) = pop_ready_kind(kind) {
                break fid;
            }
            // Wait only when nothing on this queue is runnable for *this* thread
            // (foreign-home entries must not busy-spin).
            sched_wait_while(
                |s| !s.queue_has_runnable_for(kind, tid),
                Duration::from_millis(50),
            );
        };
        with_sched(|s| s.pool_runners = s.pool_runners.saturating_add(1));
        resume_fiber(fid);
        with_sched(|s| {
            s.pool_runners = s.pool_runners.saturating_sub(1);
        });
        sched_notify();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_kind_roundtrip() {
        assert_eq!(lumia_scheduler_kind(SCHEDULER_WORKER), SCHEDULER_WORKER);
        assert_eq!(lumia_scheduler_kind(SCHEDULER_IO), SCHEDULER_IO);
    }

    #[test]
    fn pop_ready_prefers_current_scope_kind_coop() {
        // Force cooperative mode for this ordering test.
        std::env::set_var("LUMIA_SCHED_WORKERS", "0");
        std::env::set_var("LUMIA_SCHED_IO", "0");
        reload_sched_env_for_test();
        with_sched(|s| {
            s.ready_worker.clear();
            s.ready_io.clear();
            s.ready_default.clear();
            s.ready_worker.push_back(101);
            s.ready_io.push_back(202);
            s.ready_default.push_back(303);
            for (fid, task) in [(101u64, 1u64), (202, 2), (303, 3)] {
                s.fibers.insert(
                    fid,
                    FiberSlot {
                        task,
                        kind: SchedulerKind::Default,
                        pending: None,
                        coro: None,
                        yielder: Cell::new(std::ptr::null()),
                        home: None,
                        running: false,
                        wake_pending: false,
                        on_ready: true,
                    reclaim_home: false,
                    },
                );
            }
        });
        SCOPE_STACK.with(|s| {
            let sid = with_sched(|sched| {
                let id = sched.next_id;
                sched.next_id = id + 1;
                sched.scopes.insert(
                    id,
                    ScopeFrame {
                        children: vec![],
                        kind: SchedulerKind::Io,
                    },
                );
                id
            });
            s.borrow_mut().push(sid);
        });
        refresh_scope_kind_cache();
        assert_eq!(pop_ready(), Some(202));
        assert_eq!(pop_ready(), Some(303));
        assert_eq!(pop_ready(), Some(101));
        // Release pop claims (tests do not resume).
        with_sched(|s| {
            for fid in [101u64, 202, 303] {
                if let Some(slot) = s.fibers.get_mut(&fid) {
                    slot.running = false;
                }
            }
        });
        let sid = SCOPE_STACK.with(|s| s.borrow_mut().pop());
        refresh_scope_kind_cache();
        with_sched(|s| {
            if let Some(id) = sid {
                s.scopes.remove(&id);
            }
            s.fibers.remove(&101);
            s.fibers.remove(&202);
            s.fibers.remove(&303);
        });
        std::env::remove_var("LUMIA_SCHED_WORKERS");
        std::env::remove_var("LUMIA_SCHED_IO");
        reload_sched_env_for_test();
    }

    #[test]
    fn pop_ready_skips_foreign_home() {
        std::env::set_var("LUMIA_SCHED_WORKERS", "0");
        std::env::set_var("LUMIA_SCHED_IO", "0");
        reload_sched_env_for_test();
        let foreign = std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap();
        with_sched(|s| {
            s.ready_default.clear();
            s.ready_default.push_back(404);
            s.ready_default.push_back(405);
            s.fibers.insert(
                404,
                FiberSlot {
                    task: 4,
                    kind: SchedulerKind::Default,
                    pending: None,
                    coro: None,
                    yielder: Cell::new(std::ptr::null()),
                    home: Some(foreign),
                    running: false,
                    wake_pending: false,
                    on_ready: true,
                    reclaim_home: false,
                },
            );
            s.fibers.insert(
                405,
                FiberSlot {
                    task: 5,
                    kind: SchedulerKind::Default,
                    pending: None,
                    coro: None,
                    yielder: Cell::new(std::ptr::null()),
                    home: None,
                    running: false,
                    wake_pending: false,
                    on_ready: true,
                    reclaim_home: false,
                },
            );
        });
        assert_eq!(pop_ready(), Some(405));
        assert_eq!(pop_ready(), None);
        with_sched(|s| {
            assert_eq!(s.ready_default.front().copied(), Some(404));
            if let Some(slot) = s.fibers.get_mut(&405) {
                slot.running = false;
            }
            s.fibers.remove(&404);
            s.fibers.remove(&405);
            s.ready_default.clear();
        });
        std::env::remove_var("LUMIA_SCHED_WORKERS");
        std::env::remove_var("LUMIA_SCHED_IO");
        reload_sched_env_for_test();
    }

    #[test]
    fn sched_busy_sees_ready_home() {
        let tid = std::thread::current().id();
        with_sched(|s| {
            s.fibers.insert(
                701,
                FiberSlot {
                    task: 70,
                    kind: SchedulerKind::Worker,
                    pending: None,
                    coro: None,
                    yielder: Cell::new(std::ptr::null()),
                    home: Some(tid),
                    running: false,
                    wake_pending: false,
                    on_ready: true,
                    reclaim_home: false,
                },
            );
            s.ready_home.entry(tid).or_default().push_back(701);
            assert!(s.kind_pending(SchedulerKind::Worker));
            assert!(s.sched_busy());
            assert!(!s.kind_pending(SchedulerKind::Io));
            s.ready_home.clear();
            s.fibers.remove(&701);
        });
    }

    #[test]
    fn enqueue_with_home_uses_ready_home_queue() {
        let tid = std::thread::current().id();
        with_sched(|s| {
            s.fibers.insert(
                601,
                FiberSlot {
                    task: 60,
                    kind: SchedulerKind::Worker,
                    pending: None,
                    coro: None,
                    yielder: Cell::new(std::ptr::null()),
                    home: Some(tid),
                    running: false,
                    wake_pending: false,
                    on_ready: false,
                    reclaim_home: false,
                },
            );
        });
        enqueue(601);
        with_sched(|s| {
            assert_eq!(
                s.ready_home.get(&tid).and_then(|q| q.front().copied()),
                Some(601)
            );
            assert!(!s.ready_worker.contains(&601));
            s.ready_home.clear();
            s.fibers.remove(&601);
        });
    }

    #[test]
    fn enqueue_while_running_sets_wake_pending() {
        with_sched(|s| {
            s.fibers.insert(
                501,
                FiberSlot {
                    task: 50,
                    kind: SchedulerKind::Default,
                    pending: None,
                    coro: None,
                    yielder: Cell::new(std::ptr::null()),
                    home: Some(std::thread::current().id()),
                    running: true,
                    wake_pending: false,
                    on_ready: false,
                    reclaim_home: false,
                },
            );
            s.tasks.insert(
                50,
                TaskState {
                    fiber: Some(501),
                    result: None,
                    result_gc_pin: false,
                    done: false,
                    cancelled: false,
                    join_waiters: VecDeque::new(),
                    handle: std::ptr::null_mut(),
                    env: 0,
                    kind: SchedulerKind::Default,
                },
            );
        });
        enqueue(501);
        with_sched(|s| {
            let slot = s.fibers.get(&501).unwrap();
            assert!(slot.wake_pending);
            assert!(!s.ready_default.contains(&501));
            s.fibers.remove(&501);
            s.tasks.remove(&50);
        });
    }

    #[test]
    fn queue_has_runnable_ignores_foreign_home() {
        let foreign = std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap();
        let tid = std::thread::current().id();
        with_sched(|s| {
            s.ready_worker.clear();
            s.ready_worker.push_back(601);
            s.fibers.insert(
                601,
                FiberSlot {
                    task: 60,
                    kind: SchedulerKind::Default,
                    pending: None,
                    coro: None,
                    yielder: Cell::new(std::ptr::null()),
                    home: Some(foreign),
                    running: false,
                    wake_pending: false,
                    on_ready: true,
                    reclaim_home: false,
                },
            );
            assert!(!s.queue_has_runnable_for(SchedulerKind::Worker, tid));
            s.fibers.get_mut(&601).unwrap().home = Some(tid);
            assert!(s.queue_has_runnable_for(SchedulerKind::Worker, tid));
            s.fibers.remove(&601);
            s.ready_worker.clear();
        });
    }

    #[test]
    fn worker_pool_runs_spawned_task() {
        use crate::task::fiber::{
            lumia_scope_enter, lumia_scope_leave, lumia_task_spawn_nullary, task_join,
        };
        lumia_scope_enter(SCHEDULER_WORKER);
        extern "C" fn forty_two() -> i64 {
            42
        }
        let t = lumia_task_spawn_nullary(Some(forty_two));
        let v = task_join(t);
        lumia_scope_leave();
        assert_eq!(v, 42);
    }

    #[test]
    fn cancel_all_scopes_cancels_process_tasks_without_local_scope() {
        // Empty TLS SCOPE_STACK — trap hook must still cancel process tasks.
        assert!(SCOPE_STACK.with(|s| s.borrow().is_empty()));
        let task = alloc_id();
        with_sched(|s| {
            s.tasks.insert(
                task,
                TaskState {
                    fiber: None,
                    result: None,
                    result_gc_pin: false,
                    done: false,
                    cancelled: false,
                    join_waiters: VecDeque::new(),
                    handle: std::ptr::null_mut(),
                    env: 0,
                    kind: SchedulerKind::Default,
                },
            );
        });
        cancel_all_scopes();
        with_sched(|s| {
            let st = s.tasks.get(&task).expect("task");
            assert!(st.cancelled);
            assert!(st.done);
            s.tasks.remove(&task);
        });
    }
}
