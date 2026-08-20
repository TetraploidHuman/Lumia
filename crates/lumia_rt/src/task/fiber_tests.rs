use super::*;
use crate::common::PAR_WORKER;
use crate::gc::lumia_alloc;
use crate::task::channel::{lumia_channel_new, lumia_channel_recv, lumia_channel_send};
use crate::task::scheduler::{
    cancel_scope_children, lumia_scheduler_drain, snapshot_sched_gc_roots, with_sched,
    SCHEDULER_WORKER,
};
use lumia_abi::TYPE_TASK;

extern "C" fn add_one(env: i64) -> i64 {
    env + 1
}
extern "C" fn nullary_seven() -> i64 {
    7
}
extern "C" fn send_then_done(env: i64) -> i64 {
    unsafe { lumia_channel_send(env as *mut u8, 42) };
    0
}
extern "C" fn block_on_recv(env: i64) -> i64 {
    unsafe { lumia_channel_recv(env as *mut u8) }
}

#[test]
fn spawn_join_on_main() {
    let _g = crate::task::scheduler::sched_test_guard();
    lumia_scope_enter(0);
    let t = task_spawn(add_one, 41);
    let v = task_join(t);
    lumia_scope_leave();
    assert_eq!(v, 42);
}

#[test]
fn spawn_nullary_join() {
    let _g = crate::task::scheduler::sched_test_guard();
    lumia_scope_enter(0);
    let t = spawn_with(PendingSpawn::Nullary(nullary_seven));
    let v = task_join(t);
    lumia_scope_leave();
    assert_eq!(v, 7);
}

#[test]
fn spawn_inherits_scheduler_kind() {
    let _g = crate::task::scheduler::sched_test_guard();
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
    let _g = crate::task::scheduler::sched_test_guard();
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
    let _g = crate::task::scheduler::sched_test_guard();
    lumia_scope_enter(0);
    let _ = task_spawn(add_one, 99);
    lumia_scope_leave();
}

#[test]
fn cancel_never_started_then_leave() {
    let _g = crate::task::scheduler::sched_test_guard();
    lumia_scope_enter(0);
    let _ = task_spawn(add_one, 1);
    cancel_scope_children();
    lumia_scheduler_drain();
    lumia_scope_leave();
}

#[test]
fn cancel_from_fiber_spares_self() {
    let _g = crate::task::scheduler::sched_test_guard();
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
    let _g = crate::task::scheduler::sched_test_guard();
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
    let _g = crate::task::scheduler::sched_test_guard();
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
    let _g = crate::task::scheduler::sched_test_guard();
    let _ = task_spawn(add_one, 1);
}

#[test]
#[should_panic(expected = "task/channel API on parallel map worker")]
fn task_api_on_par_worker_traps() {
    let _g = crate::task::scheduler::sched_test_guard();
    struct ResetParWorker;
    impl Drop for ResetParWorker {
        fn drop(&mut self) {
            PAR_WORKER.with(|c| c.set(false));
        }
    }
    let _reset = ResetParWorker;
    PAR_WORKER.with(|c| c.set(true));
    let _ = task_spawn(add_one, 1);
}

#[test]
#[should_panic(expected = "join self")]
fn join_self_traps() {
    let _g = crate::task::scheduler::sched_test_guard();
    // should_panic still runs Drop — clear TLS so parallel test threads are not poisoned.
    struct ResetTaskTls;
    impl Drop for ResetTaskTls {
        fn drop(&mut self) {
            CURRENT_FIBER.with(|c| c.set(None));
            SCOPE_STACK.with(|s| s.borrow_mut().clear());
            crate::task::scheduler::refresh_scope_kind_cache();
        }
    }
    let _reset = ResetTaskTls;
    lumia_scope_enter(0);
    let t = task_spawn(add_one, 1);
    let task = unsafe { *(t as *const i64) as u64 };
    let fid = with_sched(|s| s.tasks.get(&task).and_then(|st| st.fiber)).expect("fiber");
    CURRENT_FIBER.with(|c| c.set(Some(fid)));
    let _ = task_join_id(task);
}

#[test]
fn cancel_started_reclaims_without_abandon_leak() {
    let _g = crate::task::scheduler::sched_test_guard();
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
    let _g = crate::task::scheduler::sched_test_guard();
    struct ResetScope;
    impl Drop for ResetScope {
        fn drop(&mut self) {
            CURRENT_FIBER.with(|c| c.set(None));
            SCOPE_STACK.with(|s| s.borrow_mut().clear());
            crate::task::scheduler::refresh_scope_kind_cache();
        }
    }
    let _reset = ResetScope;
    lumia_scope_enter(0);
    let ch = lumia_channel_new(1);
    let t = task_spawn(block_on_recv, ch as i64);
    lumia_scheduler_drain();
    cancel_scope_children();
    let _ = task_join(t);
}

#[test]
fn join_unpins_result_from_sched_snapshot() {
    let _g = crate::task::scheduler::sched_test_guard();
    lumia_scope_enter(0);
    let t = task_spawn(add_one, 41);
    let task = unsafe { *(t as *const i64) as u64 };
    let _ = task_join(t);
    let still_pinned = with_sched(|s| s.tasks.get(&task).is_some_and(|st| st.result_gc_pin));
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
