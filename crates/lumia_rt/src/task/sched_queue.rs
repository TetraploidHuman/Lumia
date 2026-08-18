//! Ready-queue pop / enqueue / wake (affinity + kind pools).

use super::sched_core::{sched_notify, with_sched, FiberId, SchedCore, SchedulerKind, Waiter};
use super::sched_env::sched_pool_counts;
use super::sched_pool::ensure_pool_for_kind;
use super::scheduler::current_scope_kind;

/// Pop a ready fiber that may run on this OS thread (`home` unset or matching).
/// Claims `running` under the same lock so an enqueue in the pop→resume gap
/// sets `wake_pending` instead of a stale ready-queue entry.
pub(super) fn try_pop_affinity(
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
            let ok = s
                .fibers
                .get(&fid)
                .map(|slot| !slot.running && slot.kind == kind && slot.home == Some(tid));
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
        let ok = s
            .fibers
            .get(&fid)
            .map(|slot| !slot.running && (slot.home.is_none() || slot.home == Some(tid)));
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

pub(crate) fn pop_ready_kind(kind: SchedulerKind) -> Option<FiberId> {
    let tid = std::thread::current().id();
    with_sched(|s| try_pop_affinity(s, kind, tid))
}

/// Queue `fid` if needed. Returns `Some(kind)` only when newly pushed onto a ready queue.
fn enqueue_inner(fid: FiberId) -> Option<SchedulerKind> {
    with_sched(|s| {
        let slot = s.fibers.get_mut(&fid)?;
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
