use super::*;

#[test]
fn list_append_cow_unique_grows_and_alias_copies() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_retain,
    };
    let mut xs = lumi_list_empty();
    let mut ys: *mut u8 = ptr::null_mut();
    lumi_root_push(&mut xs as *mut *mut u8);
    lumi_root_push(&mut ys as *mut *mut u8);
    xs = lumi_list_append(xs, 1);
    for i in 2..=64 {
        xs = lumi_list_append(xs, i);
    }
    assert_eq!(lumi_list_len(xs), 64);
    assert_eq!(lumi_list_get(xs, 0), 1);
    assert_eq!(lumi_list_get(xs, 63), 64);
    // Alias then append — old snapshot must stay length 64.
    lumi_list_retain(xs);
    ys = xs;
    xs = lumi_list_append(xs, 65);
    assert_eq!(lumi_list_len(ys), 64);
    assert_eq!(lumi_list_len(xs), 65);
    assert_eq!(lumi_list_get(xs, 64), 65);
    lumi_root_pop();
    lumi_root_pop();
}

#[test]
fn list_set_preserves_old_binding() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_retain,
        lumi_list_set,
    };
    // Shared RC (retain) → set must copy so the old binding keeps its value.
    let mut xs = lumi_list_empty();
    let mut ys = ptr::null_mut();
    lumi_root_push(&mut xs as *mut *mut u8);
    lumi_root_push(&mut ys as *mut *mut u8);
    xs = lumi_list_append(xs, 1);
    xs = lumi_list_append(xs, 2);
    xs = lumi_list_append(xs, 3);
    lumi_list_retain(xs);
    ys = lumi_list_set(xs, 1, 99);
    assert_eq!(lumi_list_len(xs), 3);
    assert_eq!(lumi_list_get(xs, 1), 2, "xs must keep old elem after set");
    assert_eq!(lumi_list_get(ys, 1), 99);
    assert_ne!(xs, ys, "shared set must return a distinct list");
    lumi_root_pop();
    lumi_root_pop();
}

#[test]
fn list_set_unique_writes_in_place() {
    use crate::list::{lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_set};
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    xs = lumi_list_append(xs, 1);
    xs = lumi_list_append(xs, 2);
    xs = lumi_list_append(xs, 3);
    let before = xs;
    xs = lumi_list_set(xs, 1, 99);
    assert_eq!(xs, before, "unique set should reuse the buffer");
    assert_eq!(lumi_list_get(xs, 1), 99);
    lumi_root_pop();
}

#[test]
fn list_set_cow_stress_alternating_alias() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_retain,
        lumi_list_set,
    };
    let mut xs = lumi_list_empty();
    let mut snap = ptr::null_mut();
    lumi_root_push(&mut xs as *mut *mut u8);
    lumi_root_push(&mut snap as *mut *mut u8);
    for i in 0..256 {
        xs = lumi_list_append(xs, i);
    }
    // Unique in-place updates.
    for i in 0..256 {
        let before = xs;
        xs = lumi_list_set(xs, i, i * 3 + 1);
        assert_eq!(xs, before);
        assert_eq!(lumi_list_get(xs, i), i * 3 + 1);
    }
    // Shared: snapshot must freeze, then unique path resumes.
    lumi_list_retain(xs);
    snap = xs;
    xs = lumi_list_set(xs, 0, -1);
    assert_ne!(xs, snap);
    assert_eq!(lumi_list_get(snap, 0), 1);
    assert_eq!(lumi_list_get(xs, 0), -1);
    assert_eq!(lumi_list_len(snap), 256);
    assert_eq!(lumi_list_len(xs), 256);
    let before = xs;
    xs = lumi_list_set(xs, 128, 42);
    assert_eq!(xs, before);
    assert_eq!(lumi_list_get(xs, 128), 42);
    assert_eq!(lumi_list_get(snap, 128), 128 * 3 + 1);
    lumi_root_pop();
    lumi_root_pop();
}

#[test]
fn list_set_retain_release_rc_recovers_for_next_unique_update() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_release, lumi_list_retain,
        lumi_list_set,
    };
    // Simulate codegen for `val ys = xs.set(…)`: temporary retain must be released
    // after set so a later `xs = xs.set(…)` can write in place when unique.
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    xs = lumi_list_append(xs, 1);
    xs = lumi_list_append(xs, 2);
    xs = lumi_list_append(xs, 3);
    lumi_list_retain(xs);
    unsafe {
        assert_eq!((*header_from_payload(xs)).rc, 2);
    }
    let ys = lumi_list_set(xs, 1, 99);
    lumi_list_release(xs);
    unsafe {
        assert_eq!((*header_from_payload(xs)).rc, 1);
    }
    assert_eq!(lumi_list_get(xs, 1), 2);
    assert_eq!(lumi_list_get(ys, 1), 99);
    let before = xs;
    xs = lumi_list_set(xs, 1, 88);
    assert_eq!(xs, before);
    assert_eq!(lumi_list_get(xs, 1), 88);
    lumi_root_pop();
}

#[test]
fn list_concat_unique_grows_amortized() {
    use crate::list::{
        lumi_list_append, lumi_list_concat, lumi_list_empty, lumi_list_get, lumi_list_len,
    };
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    xs = lumi_list_append(xs, 0);
    let mut piece = lumi_list_empty();
    lumi_root_push(&mut piece as *mut *mut u8);
    for i in 1..=4 {
        piece = lumi_list_append(piece, i);
    }
    let before = xs;
    xs = lumi_list_concat(xs, piece);
    // First concat may reallocate (exact capacity from appends).
    assert_eq!(lumi_list_len(xs), 5);
    assert_eq!(lumi_list_get(xs, 0), 0);
    assert_eq!(lumi_list_get(xs, 4), 4);
    // Second concat on unique with spare capacity → often in-place.
    let mid = xs;
    xs = lumi_list_concat(xs, piece);
    assert_eq!(lumi_list_len(xs), 9);
    assert_eq!(lumi_list_get(xs, 8), 4);
    let _ = (before, mid);
    lumi_root_pop();
    lumi_root_pop();
}

#[test]
fn list_reverse_unique_in_place() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_retain,
        lumi_list_reverse, lumi_list_reverse_consume,
    };
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    for i in 0..5 {
        xs = lumi_list_append(xs, i);
    }
    let before = xs;
    xs = lumi_list_reverse_consume(xs);
    assert_eq!(xs, before, "unique reverse_consume should swap in place");
    assert_eq!(lumi_list_len(xs), 5);
    assert_eq!(lumi_list_get(xs, 0), 4);
    assert_eq!(lumi_list_get(xs, 4), 0);
    // Non-consume must not mutate a live alias.
    lumi_list_retain(xs);
    let mut ys = lumi_list_reverse(xs);
    lumi_root_push(&mut ys as *mut *mut u8);
    assert_ne!(ys, xs);
    assert_eq!(lumi_list_get(xs, 0), 4, "xs must stay reversed after shared reverse");
    assert_eq!(lumi_list_get(ys, 0), 0);
    lumi_root_pop();
    lumi_root_pop();
}

#[test]
fn list_append_from_slice_one_shot() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_take,
    };
    use crate::{tid_base, TYPE_LIST, TYPE_LIST_SLICE};
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    for i in 0..32 {
        xs = lumi_list_append(xs, i);
    }
    let mut snap = lumi_list_take(xs, 20);
    lumi_root_push(&mut snap as *mut *mut u8);
    unsafe {
        assert_eq!(
            tid_base((*header_from_payload(snap)).type_id),
            TYPE_LIST_SLICE
        );
    }
    let mut ys = lumi_list_append(snap, 99);
    lumi_root_push(&mut ys as *mut *mut u8);
    unsafe {
        assert_eq!(tid_base((*header_from_payload(ys)).type_id), TYPE_LIST);
    }
    assert_eq!(lumi_list_len(ys), 21);
    assert_eq!(lumi_list_get(ys, 0), 0);
    assert_eq!(lumi_list_get(ys, 19), 19);
    assert_eq!(lumi_list_get(ys, 20), 99);
    assert_eq!(lumi_list_len(snap), 20);
    assert_eq!(lumi_list_get(xs, 0), 0);
    lumi_root_pop();
    lumi_root_pop();
    lumi_root_pop();
}

#[test]
fn list_slice_consume_unique_dense_memmoves_in_place() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_slice_consume,
    };
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    for i in 0..8 {
        xs = lumi_list_append(xs, i);
    }
    let before = xs;
    xs = lumi_list_slice_consume(xs, 3);
    assert_eq!(xs, before, "small remainder should memmove in place");
    assert_eq!(lumi_list_len(xs), 5);
    assert_eq!(lumi_list_get(xs, 0), 3);
    assert_eq!(lumi_list_get(xs, 4), 7);
    lumi_root_pop();
}

#[test]
fn list_slice_consume_unique_dense_large_becomes_slice() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_slice_consume,
    };
    use crate::{tid_base, TYPE_LIST_SLICE};
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    for i in 0..128 {
        xs = lumi_list_append(xs, i);
    }
    let parent = xs;
    xs = lumi_list_slice_consume(xs, 10);
    unsafe {
        assert_eq!(
            tid_base((*header_from_payload(xs)).type_id),
            TYPE_LIST_SLICE,
            "large remainder should become a Slice view"
        );
    }
    assert_ne!(xs, parent);
    assert_eq!(lumi_list_len(xs), 118);
    assert_eq!(lumi_list_get(xs, 0), 10);
    lumi_root_pop();
}

#[test]
fn list_slice_consume_unique_slice_bumps_offset() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len,
        lumi_list_slice_consume, lumi_list_take,
    };
    use crate::{tid_base, TYPE_LIST_SLICE};
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    for i in 0..10 {
        xs = lumi_list_append(xs, i);
    }
    // Shared take → slice view; consume further drop on the unique slice.
    let mut ys = lumi_list_take(xs, 8);
    lumi_root_push(&mut ys as *mut *mut u8);
    unsafe {
        assert_eq!(
            tid_base((*header_from_payload(ys)).type_id),
            TYPE_LIST_SLICE
        );
    }
    let before = ys;
    ys = lumi_list_slice_consume(ys, 2);
    assert_eq!(ys, before, "unique slice_consume should reuse slice header");
    assert_eq!(lumi_list_len(ys), 6);
    assert_eq!(lumi_list_get(ys, 0), 2);
    assert_eq!(lumi_list_get(ys, 5), 7);
    // Parent still intact.
    assert_eq!(lumi_list_get(xs, 0), 0);
    lumi_root_pop();
    lumi_root_pop();
}

#[test]
fn list_take_slice_shares_and_protects_parent() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_set, lumi_list_take,
    };
    use crate::{tid_base, TYPE_LIST_SLICE};
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    for i in 1..=8 {
        xs = lumi_list_append(xs, i);
    }
    let mut ys = lumi_list_take(xs, 3);
    lumi_root_push(&mut ys as *mut *mut u8);
    unsafe {
        assert_eq!(
            tid_base((*header_from_payload(ys)).type_id),
            TYPE_LIST_SLICE
        );
    }
    assert_eq!(lumi_list_len(ys), 3);
    assert_eq!(lumi_list_get(ys, 0), 1);
    assert_eq!(lumi_list_get(ys, 2), 3);
    // Parent shared → set must copy; slice stays [1,2,3].
    xs = lumi_list_set(xs, 0, 99);
    assert_eq!(lumi_list_get(ys, 0), 1);
    assert_eq!(lumi_list_get(xs, 0), 99);
    lumi_root_pop();
    lumi_root_pop();
}

#[test]
fn unique_slice_append_releases_parent_early() {
    use crate::common::{header_from_payload, list_rc_is_unique};
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_take,
    };
    use crate::{tid_base, TYPE_LIST, TYPE_LIST_SLICE};
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    for i in 0..8 {
        xs = lumi_list_append(xs, i);
    }
    // Unique take_consume shrinks; use shared take → Slice, then drop parent alias.
    let mut ys = lumi_list_take(xs, 4);
    lumi_root_push(&mut ys as *mut *mut u8);
    unsafe {
        assert_eq!(
            tid_base((*header_from_payload(ys)).type_id),
            TYPE_LIST_SLICE
        );
    }
    // Release xs so the slice is the unique owner of the parent retain.
    crate::list::lumi_list_release(xs);
    xs = std::ptr::null_mut();
    assert!(list_rc_is_unique(ys));
    ys = lumi_list_append(ys, 99);
    unsafe {
        assert_eq!(tid_base((*header_from_payload(ys)).type_id), TYPE_LIST);
        // Parent slot cleared on the abandoned slice object (may still be reachable
        // only via ys which is now dense).
    }
    assert_eq!(lumi_list_len(ys), 5);
    assert_eq!(lumi_list_get(ys, 4), 99);
    lumi_root_pop();
    lumi_root_pop();
}

#[test]
fn list_sort_consume_unique_in_place() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_sort_consume,
    };
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    for i in [3i64, 1, 4, 1, 5] {
        xs = lumi_list_append(xs, i);
    }
    let before = xs;
    xs = lumi_list_sort_consume(xs);
    assert_eq!(xs, before, "unique sort_consume should reuse buffer");
    assert_eq!(lumi_list_get(xs, 0), 1);
    assert_eq!(lumi_list_get(xs, 1), 1);
    assert_eq!(lumi_list_get(xs, 2), 3);
    assert_eq!(lumi_list_get(xs, 3), 4);
    assert_eq!(lumi_list_get(xs, 4), 5);
    lumi_root_pop();
}

#[test]
fn range_is_iota_not_materialized() {
    let r = lumi_range(0, 1_000_000);
    assert!(!r.is_null());
    unsafe {
        assert_eq!((*header_from_payload(r)).type_id, TYPE_LIST_IOTA);
        assert_eq!((*header_from_payload(r)).size, 16);
    }
    assert_eq!(lumi_list_len(r), 1_000_000);
    assert_eq!(lumi_list_get(r, 0), 0);
    assert_eq!(lumi_list_get(r, 999_999), 999_999);
    // Content-equal to a small heap list of the same prefix.
    let h = lumi_range(10, 13);
    let forced = force_heap_list(h);
    unsafe {
        assert_eq!((*header_from_payload(forced)).type_id, TYPE_LIST);
    }
    assert_eq!(lumi_eq(h as i64, forced as i64), 1);
    assert_eq!(lumi_list_len(lumi_list_take(r, 3)), 3);
    assert_eq!(lumi_list_get(lumi_list_slice(r, 5), 0), 5);
}

#[test]
#[should_panic(expected = "list too large")]
fn force_huge_iota_traps_without_alloc() {
    // Length that cannot fit in ObjectHeader.size (u32) when stored as bytes.
    let n = (u32::MAX as i64 / 8) + 8;
    let r = lumi_range(0, n);
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

#[test]
fn ensure_list_f64_null_and_empty_and_identity() {
    let from_null = crate::list::ensure_list_f64(ptr::null_mut());
    assert!(!from_null.is_null());
    unsafe {
        assert_eq!((*header_from_payload(from_null)).type_id, TYPE_LIST_F64);
        assert_eq!(*(from_null as *const i64), 0);
    }

    let empty = lumi_alloc(8, TYPE_LIST);
    unsafe {
        *(empty as *mut i64) = 0;
    }
    let retagged = crate::list::ensure_list_f64(empty);
    assert!(!retagged.is_null());
    unsafe {
        assert_eq!((*header_from_payload(retagged)).type_id, TYPE_LIST_F64);
        assert_eq!(*(retagged as *const i64), 0);
    }

    let already = lumi_alloc(8, TYPE_LIST_F64);
    unsafe {
        *(already as *mut i64) = 0;
    }
    assert_eq!(crate::list::ensure_list_f64(already), already);
}

#[test]
#[should_panic(expected = "ensure_list_f64 on non-empty Int-elem list")]
fn ensure_list_f64_nonempty_int_traps() {
    let p = lumi_alloc(list_payload_bytes(1), TYPE_LIST);
    unsafe {
        *(p as *mut i64) = 1;
        *((p as *mut i64).add(1)) = 42;
    }
    let _ = crate::list::ensure_list_f64(p);
}

#[test]
#[should_panic(expected = "ensure_list_f64 on Iota")]
fn ensure_list_f64_iota_traps() {
    let r = lumi_range(0, 3);
    let _ = crate::list::ensure_list_f64(r);
}
