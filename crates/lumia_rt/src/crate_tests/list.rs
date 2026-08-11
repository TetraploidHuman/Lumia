use super::*;

#[test]
fn list_append_cow_unique_grows_and_alias_copies() {
    use crate::list::{
        lumia_list_append, lumia_list_empty, lumia_list_get, lumia_list_len, lumia_list_retain,
    };
    let mut xs = lumia_list_empty();
    let mut ys: *mut u8 = ptr::null_mut();
    lumia_root_push(&mut xs as *mut *mut u8);
    lumia_root_push(&mut ys as *mut *mut u8);
    xs = lumia_list_append(xs, 1);
    for i in 2..=64 {
        xs = lumia_list_append(xs, i);
    }
    assert_eq!(lumia_list_len(xs), 64);
    assert_eq!(lumia_list_get(xs, 0), 1);
    assert_eq!(lumia_list_get(xs, 63), 64);
    // Alias then append — old snapshot must stay length 64.
    lumia_list_retain(xs);
    ys = xs;
    xs = lumia_list_append(xs, 65);
    assert_eq!(lumia_list_len(ys), 64);
    assert_eq!(lumia_list_len(xs), 65);
    assert_eq!(lumia_list_get(xs, 64), 65);
    lumia_root_pop();
    lumia_root_pop();
}

#[test]
fn list_set_preserves_old_binding() {
    use crate::list::{
        lumia_list_append, lumia_list_empty, lumia_list_get, lumia_list_len, lumia_list_set,
    };
    // Unique list (RC=1) must still allocate on set — live aliases observe old value.
    let mut xs = lumia_list_empty();
    let mut ys = ptr::null_mut();
    lumia_root_push(&mut xs as *mut *mut u8);
    lumia_root_push(&mut ys as *mut *mut u8);
    xs = lumia_list_append(xs, 1);
    xs = lumia_list_append(xs, 2);
    xs = lumia_list_append(xs, 3);
    ys = lumia_list_set(xs, 1, 99);
    assert_eq!(lumia_list_len(xs), 3);
    assert_eq!(lumia_list_get(xs, 1), 2, "xs must keep old elem after set");
    assert_eq!(lumia_list_get(ys, 1), 99);
    assert_ne!(xs, ys, "set must return a distinct list");
    lumia_root_pop();
    lumia_root_pop();
}

#[test]
fn range_is_iota_not_materialized() {
    let r = lumia_range(0, 1_000_000);
    assert!(!r.is_null());
    unsafe {
        assert_eq!((*header_from_payload(r)).type_id, TYPE_LIST_IOTA);
        assert_eq!((*header_from_payload(r)).size, 16);
    }
    assert_eq!(lumia_list_len(r), 1_000_000);
    assert_eq!(lumia_list_get(r, 0), 0);
    assert_eq!(lumia_list_get(r, 999_999), 999_999);
    // Content-equal to a small heap list of the same prefix.
    let h = lumia_range(10, 13);
    let forced = force_heap_list(h);
    unsafe {
        assert_eq!((*header_from_payload(forced)).type_id, TYPE_LIST);
    }
    assert_eq!(lumia_eq(h as i64, forced as i64), 1);
    assert_eq!(lumia_list_len(lumia_list_take(r, 3)), 3);
    assert_eq!(lumia_list_get(lumia_list_slice(r, 5), 0), 5);
}

#[test]
#[should_panic(expected = "list too large")]
fn force_huge_iota_traps_without_alloc() {
    // Length that cannot fit in ObjectHeader.size (u32) when stored as bytes.
    let n = (u32::MAX as i64 / 8) + 8;
    let r = lumia_range(0, n);
    let _ = force_heap_list(r);
}

#[test]
#[should_panic(expected = "list too large")]
fn list_payload_bytes_rejects_overflow() {
    let _ = list_payload_bytes(i64::MAX);
}

#[test]
#[should_panic(expected = "parallel map worker")]
fn par_worker_alloc_is_forbidden() {
    PAR_WORKER.with(|c| c.set(true));
    // Call Rust path (not `extern "C"`) so the panic can unwind for should_panic.
    let _ = MarkSweep.alloc(8, TYPE_LIST);
}
