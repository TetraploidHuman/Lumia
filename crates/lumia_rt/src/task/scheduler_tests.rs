use super::*;

#[test]
fn scheduler_kind_roundtrip() {
    assert_eq!(lumia_scheduler_kind(SCHEDULER_WORKER), SCHEDULER_WORKER);
    assert_eq!(lumia_scheduler_kind(SCHEDULER_IO), SCHEDULER_IO);
}

#[test]
fn pop_ready_prefers_current_scope_kind_coop() {
    with_sched_unit_test(|| {
        // Force cooperative mode for this ordering test.
        std::env::set_var("LUMIA_SCHED_WORKERS", "0");
        std::env::set_var("LUMIA_SCHED_IO", "0");
        reload_sched_env_for_test();
        assert_eq!(
            sched_pool_counts(),
            (0, 0),
            "coop ordering test requires LUMIA_SCHED_*=0"
        );
        // Pin to this thread so a leftover OS pool cannot steal synthetic entries
        // (pool Once starts workers for the process lifetime).
        let home = std::thread::current().id();
        // Drop any polluted scope stack from prior tests on this thread.
        SCOPE_STACK.with(|s| s.borrow_mut().clear());
        SCOPE_KIND_CACHE.with(|c| c.set(0));
        with_sched(|s| {
            s.ready_worker.clear();
            s.ready_io.clear();
            s.ready_default.clear();
            s.ready_home.remove(&home);
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
                        has_coro: false,
                        yielder: Cell::new(crate::task::scan_ptrs::YielderAddr::null()),
                        home: Some(home),
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
    });
}

#[test]
fn pop_ready_skips_foreign_home() {
    with_sched_unit_test(|| {
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
                    has_coro: false,
                    yielder: Cell::new(crate::task::scan_ptrs::YielderAddr::null()),
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
                    has_coro: false,
                    yielder: Cell::new(crate::task::scan_ptrs::YielderAddr::null()),
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
    });
}

#[test]
fn sched_busy_sees_ready_home() {
    with_sched_unit_test(|| {
        let tid = std::thread::current().id();
        with_sched(|s| {
            s.fibers.insert(
                701,
                FiberSlot {
                    task: 70,
                    kind: SchedulerKind::Worker,
                    pending: None,
                    has_coro: false,
                    yielder: Cell::new(crate::task::scan_ptrs::YielderAddr::null()),
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
            s.ready_home.remove(&tid);
            s.fibers.remove(&701);
        });
    });
}

#[test]
fn enqueue_with_home_uses_ready_home_queue() {
    with_sched_unit_test(|| {
        let tid = std::thread::current().id();
        with_sched(|s| {
            s.fibers.insert(
                601,
                FiberSlot {
                    task: 60,
                    kind: SchedulerKind::Worker,
                    pending: None,
                    has_coro: false,
                    yielder: Cell::new(crate::task::scan_ptrs::YielderAddr::null()),
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
            s.ready_home.remove(&tid);
            s.fibers.remove(&601);
        });
    });
}

#[test]
fn enqueue_while_running_sets_wake_pending() {
    with_sched_unit_test(|| {
        with_sched(|s| {
            s.fibers.insert(
                501,
                FiberSlot {
                    task: 50,
                    kind: SchedulerKind::Default,
                    pending: None,
                    has_coro: false,
                    yielder: Cell::new(crate::task::scan_ptrs::YielderAddr::null()),
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
                    handle: crate::task::scan_ptrs::TaskHandlePtr::null(),
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
    });
}

#[test]
fn queue_has_runnable_ignores_foreign_home() {
    with_sched_unit_test(|| {
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
                    has_coro: false,
                    yielder: Cell::new(crate::task::scan_ptrs::YielderAddr::null()),
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
    });
}

#[test]
fn worker_pool_runs_spawned_task() {
    with_sched_unit_test(|| {
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
    });
}

#[test]
fn cancel_all_scopes_cancels_process_tasks_without_local_scope() {
    with_sched_unit_test(|| {
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
                    handle: crate::task::scan_ptrs::TaskHandlePtr::null(),
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
    });
}
