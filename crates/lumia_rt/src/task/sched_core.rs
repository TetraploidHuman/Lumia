//! Process-shared scheduler maps + ready queues (BUILD §7.7 phase D).
//!
//! Fibers are created lazily on first resume so a worker OS thread can own the
//! corosensei stack (`Coroutine` is `!Send`). Scope **frames** live in
//! [`SchedCore`]; TLS only holds [`ScopeId`] stacks (parked with the fiber).

use rustc_hash::FxHashMap;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use crate::reentrant::with_mutex_reentrant;

use super::scan_ptrs::{ParkedCallFrames, ParkedRootSlots, TaskHandlePtr, YielderCell};

pub type FiberId = u64;
pub type TaskId = u64;
pub type ChannelId = u64;
pub type ScopeId = u64;

pub use lumia_abi::{SCHEDULER_IO, SCHEDULER_WORKER};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulerKind {
    Default,
    Worker,
    Io,
}

impl SchedulerKind {
    pub fn from_i64(v: i64) -> Self {
        match v {
            SCHEDULER_WORKER => Self::Worker,
            SCHEDULER_IO => Self::Io,
            _ => Self::Default,
        }
    }
}

/// Deferred spawn body — built into a coroutine on the resuming OS thread.
#[derive(Clone, Copy)]
pub enum PendingSpawn {
    Unary(extern "C" fn(i64) -> i64, i64),
    Nullary(extern "C" fn() -> i64),
}

pub struct ScopeFrame {
    pub children: Vec<TaskId>,
    pub kind: SchedulerKind,
}

pub struct TaskState {
    pub fiber: Option<FiberId>,
    pub result: Option<i64>,
    /// When true, [`Self::result`] is a GC root via `snapshot_sched_gc_roots`.
    /// Cleared after the first successful join (handle word keeps the value live).
    pub result_gc_pin: bool,
    pub done: bool,
    pub cancelled: bool,
    pub join_waiters: VecDeque<Waiter>,
    pub handle: TaskHandlePtr,
    pub env: i64,
    /// Mirrored from spawn scope; runtime affinity uses [`FiberSlot::kind`].
    #[allow(dead_code)]
    pub kind: SchedulerKind,
}

pub struct ChannelState {
    pub cap: usize,
    pub buf: VecDeque<i64>,
    pub closed: bool,
    pub send_waiters: VecDeque<Waiter>,
    pub recv_waiters: VecDeque<Waiter>,
}

#[derive(Clone, Copy)]
pub enum Waiter {
    Fiber(FiberId),
    Main,
}

pub struct FiberSlot {
    pub task: TaskId,
    pub kind: SchedulerKind,
    pub pending: Option<PendingSpawn>,
    /// Parked coroutine lives in home-thread TLS ([`super::home_coro`]), not here.
    /// True iff that TLS map holds a stack for this fiber.
    pub has_coro: bool,
    pub yielder: YielderCell,
    /// OS thread that first resumed this fiber (`None` = not started; stealable).
    pub home: Option<std::thread::ThreadId>,
    /// True while some OS thread holds the coroutine out of TLS (`resume_fiber`).
    pub running: bool,
    /// `enqueue` while `running` — re-queue on yield / cancel exit.
    pub wake_pending: bool,
    /// Already present in a ready queue (avoids O(n) `contains`).
    pub on_ready: bool,
    /// Cancelled; coro must be disposed on [`Self::home`] (not a foreign thread).
    pub reclaim_home: bool,
}

pub struct SchedCore {
    pub next_id: u64,
    /// Stealable work (`home == None`) and default-kind coop queues.
    pub ready_default: VecDeque<FiberId>,
    pub ready_worker: VecDeque<FiberId>,
    pub ready_io: VecDeque<FiberId>,
    /// Home-pinned ready fibers (`WORKERS`/`IO` > 1): O(1) pop for the owning OS thread.
    pub ready_home: FxHashMap<std::thread::ThreadId, VecDeque<FiberId>>,
    pub fibers: FxHashMap<FiberId, FiberSlot>,
    pub tasks: FxHashMap<TaskId, TaskState>,
    pub channels: FxHashMap<ChannelId, ChannelState>,
    /// Structured-concurrency frames (TLS holds [`ScopeId`] stacks only).
    pub scopes: FxHashMap<ScopeId, ScopeFrame>,
    pub parked_roots: FxHashMap<FiberId, ParkedRootSlots>,
    pub parked_call_stacks: FxHashMap<FiberId, ParkedCallFrames>,
    pub parked_scope_stacks: FxHashMap<FiberId, Vec<ScopeId>>,
    /// Host (non-fiber) roots parked while a fiber runs on that OS thread.
    pub host_roots: FxHashMap<std::thread::ThreadId, ParkedRootSlots>,
    pub host_call_stacks: FxHashMap<std::thread::ThreadId, ParkedCallFrames>,
    pub host_scope_stacks: FxHashMap<std::thread::ThreadId, Vec<ScopeId>>,
    /// Last ABI return handoff (e.g. channel recv) — scanned until overwritten/cleared.
    pub abi_handoff: FxHashMap<std::thread::ThreadId, i64>,
    pub pool_runners: u32,
}

impl SchedCore {
    fn new() -> Self {
        Self {
            next_id: 1,
            ready_default: VecDeque::new(),
            ready_worker: VecDeque::new(),
            ready_io: VecDeque::new(),
            ready_home: FxHashMap::default(),
            fibers: FxHashMap::default(),
            tasks: FxHashMap::default(),
            channels: FxHashMap::default(),
            scopes: FxHashMap::default(),
            parked_roots: FxHashMap::default(),
            parked_call_stacks: FxHashMap::default(),
            parked_scope_stacks: FxHashMap::default(),
            host_roots: FxHashMap::default(),
            host_call_stacks: FxHashMap::default(),
            host_scope_stacks: FxHashMap::default(),
            abi_handoff: FxHashMap::default(),
            pool_runners: 0,
        }
    }

    pub fn ready_queue_mut(&mut self, kind: SchedulerKind) -> &mut VecDeque<FiberId> {
        match kind {
            SchedulerKind::Default => &mut self.ready_default,
            SchedulerKind::Worker => &mut self.ready_worker,
            SchedulerKind::Io => &mut self.ready_io,
        }
    }

    pub fn ready_queue(&self, kind: SchedulerKind) -> &VecDeque<FiberId> {
        match kind {
            SchedulerKind::Default => &self.ready_default,
            SchedulerKind::Worker => &self.ready_worker,
            SchedulerKind::Io => &self.ready_io,
        }
    }

    pub fn ready_nonempty(&self) -> bool {
        !self.ready_default.is_empty()
            || !self.ready_worker.is_empty()
            || !self.ready_io.is_empty()
            || self.ready_home.values().any(|q| !q.is_empty())
    }

    /// Push onto the stealable kind queue or the owning home queue.
    pub fn push_ready(&mut self, fid: FiberId, kind: SchedulerKind, home: Option<std::thread::ThreadId>) {
        if let Some(h) = home {
            self.ready_home.entry(h).or_default().push_back(fid);
        } else {
            self.ready_queue_mut(kind).push_back(fid);
        }
    }

    /// Remove `fid` from every ready structure (cancel / abandon).
    pub fn retain_ready(&mut self, mut pred: impl FnMut(FiberId) -> bool) {
        for kind in [
            SchedulerKind::Default,
            SchedulerKind::Worker,
            SchedulerKind::Io,
        ] {
            self.ready_queue_mut(kind).retain(|&x| pred(x));
        }
        for q in self.ready_home.values_mut() {
            q.retain(|&x| pred(x));
        }
        self.ready_home.retain(|_, q| !q.is_empty());
    }

    /// True if some queued fiber may run on `tid` (affinity + not mid-resume).
    pub fn queue_has_runnable_for(&self, kind: SchedulerKind, tid: std::thread::ThreadId) -> bool {
        if let Some(q) = self.ready_home.get(&tid) {
            if q.iter().any(|&fid| {
                self.fibers.get(&fid).is_some_and(|slot| {
                    !slot.running && slot.kind == kind
                })
            }) {
                return true;
            }
        }
        self.ready_queue(kind).iter().any(|&fid| {
            self.fibers.get(&fid).is_some_and(|slot| {
                !slot.running && (slot.home.is_none() || slot.home == Some(tid))
            })
        })
    }

    /// Shared or home-pinned ready work of `kind` (ignores affinity).
    pub fn kind_pending(&self, kind: SchedulerKind) -> bool {
        !self.ready_queue(kind).is_empty()
            || self.ready_home.values().any(|q| {
                q.iter()
                    .any(|&fid| self.fibers.get(&fid).is_some_and(|slot| slot.kind == kind))
            })
    }

    /// Any ready work or in-flight pool runner (for drain / park predicates).
    pub fn sched_busy(&self) -> bool {
        self.pool_runners > 0 || self.ready_nonempty()
    }
}

/// `SchedCore` is naturally `Send`: `!Send` corosensei stacks live in home-thread
/// TLS ([`super::home_coro`]); parked GC root / call-stack words use
/// [`super::scan_ptrs`] newtypes. Fibers are created, resumed, and disposed only
/// on their **home** OS thread; pool workers may move ready/handoff **metadata**
/// (ids), never a live stack. See
/// [`dispose_cancelled_coroutine`](super::sched_cancel::dispose_cancelled_coroutine).

struct SchedBox {
    core: Mutex<SchedCore>,
    cvar: Condvar,
}

static SCHED: OnceLock<SchedBox> = OnceLock::new();

fn sched_box() -> &'static SchedBox {
    SCHED.get_or_init(|| SchedBox {
        core: Mutex::new(SchedCore::new()),
        cvar: Condvar::new(),
    })
}

thread_local! {
    static SCHED_DEPTH: Cell<u32> = const { Cell::new(0) };
    static SCHED_REBORROW: Cell<*mut SchedCore> = const { Cell::new(std::ptr::null_mut()) };
}

pub fn with_sched<R>(f: impl FnOnce(&mut SchedCore) -> R) -> R {
    with_mutex_reentrant(&sched_box().core, &SCHED_DEPTH, &SCHED_REBORROW, f)
}

pub fn sched_notify() {
    sched_box().cvar.notify_all();
}

/// Wait while `pred` is true (under scheduler lock), up to `timeout`.
/// Returns true if woken because `pred` became false.
pub fn sched_wait_while(mut pred: impl FnMut(&SchedCore) -> bool, timeout: Duration) -> bool {
    let box_ = sched_box();
    let guard = box_.core.lock().unwrap_or_else(|p| p.into_inner());
    if !pred(&guard) {
        return true;
    }
    let (_g, result) = box_
        .cvar
        .wait_timeout_while(guard, timeout, |core| pred(core))
        .unwrap_or_else(|e| e.into_inner());
    !result.timed_out()
}

#[cfg(test)]
#[path = "sched_core_tests.rs"]
mod home_coro_split_tests;
