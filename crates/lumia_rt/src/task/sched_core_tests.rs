// Extracted from production module (Todo: RT 测例半迁).

/// Lock-in: fiber slots no longer embed `Coroutine` (home TLS owns stacks).
#[test]
fn fiber_slot_has_coro_flag_not_coroutine_field() {
    let slot = super::FiberSlot {
        task: 0,
        kind: super::SchedulerKind::Default,
        pending: None,
        has_coro: false,
        yielder: std::cell::Cell::new(super::super::scan_ptrs::YielderAddr::null()),
        home: None,
        running: false,
        wake_pending: false,
        on_ready: false,
        reclaim_home: false,
    };
    assert!(!slot.has_coro);
    assert!(slot.pending.is_none());
}

#[test]
fn sched_core_is_send_after_scan_ptr_newtypes() {
    fn assert_send<T: Send>() {}
    assert_send::<super::SchedCore>();
}
