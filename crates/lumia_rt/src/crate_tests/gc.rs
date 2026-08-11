use super::*;

#[test]
#[should_panic(expected = "stack trace:")]
fn trap_prints_call_stack() {
    let a = b"alpha\0";
    let b = b"beta\0";
    lumia_frame_push(a.as_ptr());
    lumia_frame_push(b.as_ptr());
    trap_abort("lumia: test trap");
}

#[test]
fn alloc_and_collect_unrooted() {
    let p = lumia_alloc(16, TYPE_BYTES);
    assert!(!p.is_null());
    // Not rooted → collect should free
    lumia_gc_collect();
    // Heap should be empty or not contain live unmarked — allocate again
    let q = lumia_alloc(8, TYPE_BYTES);
    assert!(!q.is_null());
}

#[test]
fn rooted_survives_collect() {
    let mut slot: *mut u8 = lumia_alloc(32, TYPE_STRING);
    lumia_root_push(&mut slot as *mut *mut u8);
    lumia_gc_collect();
    assert!(!slot.is_null());
    // header still valid
    let h = header_from_payload(slot);
    unsafe {
        assert_eq!((*h).type_id, TYPE_STRING);
        assert_eq!((*h).size, 32);
    }
    lumia_root_pop();
}

#[test]
fn write_barrier_records_old_to_young() {
    let p = lumia_alloc(8, TYPE_BYTES);
    // Young→young is a no-op for the remembered set.
    lumia_write_barrier(p, 0, ptr::null_mut());
    assert_eq!(crate::common::gc_remembered_len_for_test(), 0);
}

#[test]
fn println_int_smoke() {
    lumia_println_int(7);
}

#[test]
fn rooted_survives_soft_threshold() {
    // Lower limit temporarily via many small allocs with a rooted object.
    let mut slot: *mut u8 = lumia_alloc(64, TYPE_STRING);
    lumia_root_push(&mut slot as *mut *mut u8);
    for _ in 0..5000 {
        let _ = lumia_alloc(64, TYPE_BYTES);
    }
    assert!(!slot.is_null());
    let h = header_from_payload(slot);
    unsafe {
        assert_eq!((*h).type_id, TYPE_STRING);
        assert_eq!((*h).size, 64);
    }
    lumia_root_pop();
}

#[test]
fn minor_promotes_rooted_and_frees_garbage() {
    let _limits = GcLimitGuard::set(256, 16 * 1024 * 1024);
    let mut slot: *mut u8 = lumia_alloc(64, TYPE_STRING);
    lumia_root_push(&mut slot as *mut *mut u8);
    // Fill nursery with garbage past young limit → minor STW.
    for _ in 0..32 {
        let _ = lumia_alloc(64, TYPE_BYTES);
    }
    let (young_n, old_n) = gc_heap_lens_for_test();
    assert!(
        old_n >= 1,
        "rooted survivor should promote to old, young={young_n} old={old_n}"
    );
    assert!(!slot.is_null());
    unsafe {
        assert_eq!((*header_from_payload(slot)).type_id, TYPE_STRING);
    }
    lumia_root_pop();
}

#[test]
fn minor_keeps_young_reachable_from_old() {
    use crate::list::{lumia_list_append, lumia_list_empty, lumia_list_get, lumia_list_len};
    let _limits = GcLimitGuard::set(512, 16 * 1024 * 1024);
    // Build a unique list, promote it, then store a fresh young heap pointer in it.
    let mut xs = lumia_list_empty();
    let mut child = lumia_alloc(16, TYPE_BYTES);
    xs = lumia_list_append(xs, child as i64);
    lumia_root_push(&mut xs as *mut *mut u8);
    for _ in 0..64 {
        let _ = lumia_alloc(64, TYPE_BYTES);
    }
    let (_y0, old0) = gc_heap_lens_for_test();
    assert!(old0 >= 1, "list should have tenured");
    // New young child stored into (possibly tenured) list via COW/in-place append.
    child = lumia_alloc(16, TYPE_BYTES);
    xs = lumia_list_append(xs, child as i64);
    for _ in 0..64 {
        let _ = lumia_alloc(64, TYPE_BYTES);
    }
    assert_eq!(lumia_list_len(xs), 2);
    let kept = lumia_list_get(xs, 1) as *mut u8;
    assert!(
        crate::common::is_heap_payload(kept),
        "young-from-old child must survive minor"
    );
    lumia_root_pop();
    let (_y, _o) = gc_live_bytes_for_test();
}

#[test]
fn write_barrier_remembers_old_to_young() {
    use crate::list::{lumia_list_append, lumia_list_empty, lumia_list_len};
    let _limits = GcLimitGuard::set(256, 16 * 1024 * 1024);
    // Drop leftovers from earlier tests so nursery pressure is predictable.
    lumia_gc_collect();
    let mut xs = lumia_list_empty();
    let child0 = lumia_alloc(16, TYPE_BYTES);
    xs = lumia_list_append(xs, child0 as i64);
    lumia_root_push(&mut xs as *mut *mut u8);
    let old_before = gc_heap_lens_for_test().1;
    for _ in 0..128 {
        let _ = lumia_alloc(64, TYPE_BYTES);
        if gc_heap_lens_for_test().1 > old_before {
            break;
        }
    }
    assert!(
        gc_heap_lens_for_test().1 > old_before,
        "rooted list should tenure under nursery pressure"
    );
    let before = crate::common::gc_remembered_len_for_test();
    let child1 = lumia_alloc(16, TYPE_BYTES);
    xs = lumia_list_append(xs, child1 as i64);
    let after = crate::common::gc_remembered_len_for_test();
    assert!(
        after > before,
        "in-place append into old list must dirty remembered set ({before} -> {after})"
    );
    assert_eq!(lumia_list_len(xs), 2);
    lumia_root_pop();
}

#[test]
fn empty_list_singleton_survives_gc() {
    let a = lumia_list_empty();
    let b = lumia_list_empty();
    assert_eq!(a, b);
    assert_eq!(lumia_list_len(a), 0);
    // Force a collection; permanent root must keep the singleton alive.
    lumia_gc_collect();
    assert_eq!(lumia_list_empty(), a);
    // Identity concat on a heap list (Iota would be forced first).
    let xs = force_heap_list(lumia_range(1, 4));
    let id = lumia_list_concat(lumia_list_empty(), xs);
    assert_eq!(id, xs);
    assert_eq!(lumia_list_len(id), 3);
    assert_eq!(lumia_list_concat(xs, lumia_list_empty()), xs);
}
