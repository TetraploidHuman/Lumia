// Extracted from gc/pressure.rs (Todo: RT 测例半迁).
use super::*;
use crate::heap::Heap;
use std::sync::atomic::Ordering;

fn restore(pressure: bool, full: bool) {
    ALLOC_PRESSURE_FAST.store(pressure, Ordering::Release);
    FULL_MARKING_FAST.store(full, Ordering::Release);
}

#[test]
fn refresh_sets_pressure_when_young_over_limit() {
    let prev_p = ALLOC_PRESSURE_FAST.load(Ordering::Acquire);
    let prev_f = FULL_MARKING_FAST.load(Ordering::Acquire);
    let mut h = Heap::new();
    h.bytes_young = h.young_limit;
    refresh_from_heap(&h);
    assert!(alloc_pressure_fast());
    h.bytes_young = 0;
    h.bytes_old = 0;
    h.full_marking = false;
    refresh_from_heap(&h);
    assert!(!alloc_pressure_fast());
    restore(prev_p, prev_f);
}

#[test]
fn set_full_marking_forces_pressure() {
    let prev_p = ALLOC_PRESSURE_FAST.load(Ordering::Acquire);
    let prev_f = FULL_MARKING_FAST.load(Ordering::Acquire);
    let h = Heap::new();
    refresh_from_heap(&h);
    set_full_marking_fast(true);
    assert!(full_marking_fast());
    assert!(alloc_pressure_fast());
    set_full_marking_fast(false);
    refresh_from_heap(&h);
    assert!(!full_marking_fast());
    restore(prev_p, prev_f);
}
