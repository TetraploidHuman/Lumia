// Extracted from list/mod.rs (Todo: RT 测例半迁).
use super::*;
use crate::common::header_from_payload;
use crate::TYPE_LIST_IOTA;

#[test]
fn range_empty_and_inverted() {
    let empty = lumia_range(5, 5);
    assert_eq!(list_len_of(empty), 0);
    let inv = lumia_range(10, 3);
    assert_eq!(list_len_of(inv), 0);
    let incl = lumia_range_inclusive(2, 4);
    assert_eq!(list_len_of(incl), 3);
    assert_eq!(list_get_of(incl, 2), 4);
}

#[test]
fn force_iota_stamps_list_int_tid() {
    use lumia_abi::list_elem_is_int;
    let r = lumia_range(0, 100);
    let d = force_heap_list(r);
    unsafe {
        assert!(
            list_elem_is_int((*header_from_payload(d)).type_id),
            "forced iota should be List[Int] for shade skip"
        );
    }
    assert_eq!(list_get_of(d, 50), 50);
}

#[test]
fn iota_identity_set_skips_materialize() {
    use lumia_abi::TYPE_LIST_IOTA;
    let r = lumia_range(0, 100);
    let s = unsafe { lumia_list_set(r, 0, 0) };
    assert_eq!(s, r, "identity set must return the same iota");
    unsafe {
        assert_eq!((*header_from_payload(s)).type_id, TYPE_LIST_IOTA);
    }
}

#[test]
fn iota_sparse_set_stays_patch() {
    use lumia_abi::{tid_list_patch, TYPE_LIST_PATCH};
    let r = lumia_range(0, 1_000_000);
    let s = unsafe { lumia_list_set(r, 42, 99) };
    unsafe {
        let tid = (*header_from_payload(s)).type_id;
        assert!(
            tid_list_patch(tid) || tid == TYPE_LIST_PATCH,
            "sparse set should be a patch overlay, tid={tid}"
        );
    }
    assert_eq!(list_len_of(s), 1_000_000);
    assert_eq!(list_get_of(s, 0), 0);
    assert_eq!(list_get_of(s, 42), 99);
    assert_eq!(list_get_of(s, 999_999), 999_999);
    // Same index update stays sparse.
    let s2 = unsafe { lumia_list_set(s, 42, 100) };
    assert_eq!(list_get_of(s2, 42), 100);
    assert_eq!(list_get_of(s2, 41), 41);
    let s3 = unsafe { lumia_list_set(s2, 7, 70) };
    assert_eq!(s2, s3, "unique patch must append a new index in place");
    assert_eq!(list_get_of(s3, 7), 70);
}

#[test]
fn iota_take_preserves_iota_tag() {
    let r = lumia_range(10, 20);
    let t = unsafe { lumia_list_take(r, 3) };
    unsafe {
        assert_eq!((*header_from_payload(t)).type_id, TYPE_LIST_IOTA);
    }
    assert_eq!(list_len_of(t), 3);
    assert_eq!(list_get_of(t, 0), 10);
    assert_eq!(list_get_of(t, 2), 12);
    // Negative / oversized take clamps.
    assert_eq!(list_len_of(unsafe { lumia_list_take(r, -1) }), 0);
    assert_eq!(list_len_of(unsafe { lumia_list_take(r, 100) }), 10);
}

#[test]
fn reverse_and_sort_heap_list() {
    let mut xs = lumia_list_empty();
    for v in [3, 1, 2] {
        xs = unsafe { lumia_list_append(xs, v) };
    }
    // Shared: reverse/sort must not mutate the source (codegen retains unless `xs = xs.reverse()`).
    unsafe { lumia_list_retain(xs) };
    let rev = unsafe { lumia_list_reverse(xs) };
    assert_ne!(rev, xs);
    assert_eq!(list_get_of(rev, 0), 2);
    assert_eq!(list_get_of(rev, 2), 3);
    unsafe { lumia_list_retain(xs) };
    let sorted = unsafe { lumia_list_sort(xs) };
    assert_ne!(sorted, xs);
    assert_eq!(list_get_of(sorted, 0), 1);
    assert_eq!(list_get_of(sorted, 1), 2);
    assert_eq!(list_get_of(sorted, 2), 3);
}

#[test]
fn unique_reverse_and_sort_in_place() {
    let mut xs = lumia_list_empty();
    for v in [3, 1, 2] {
        xs = unsafe { lumia_list_append(xs, v) };
    }
    let rev = unsafe { lumia_list_reverse(xs) };
    assert_eq!(rev, xs, "unique reverse must be in place");
    assert_eq!(list_get_of(rev, 0), 2);
    assert_eq!(list_get_of(rev, 2), 3);
    let sorted = unsafe { lumia_list_sort(rev) };
    assert_eq!(sorted, rev, "unique sort must be in place");
    assert_eq!(list_get_of(sorted, 0), 1);
    assert_eq!(list_get_of(sorted, 2), 3);
}

#[test]
fn unique_sort_by_keys_in_place() {
    let mut xs = lumia_list_empty();
    for v in [30, 10, 20] {
        xs = unsafe { lumia_list_append(xs, v) };
    }
    let mut keys = lumia_list_empty();
    for k in [3, 1, 2] {
        keys = unsafe { lumia_list_append(keys, k) };
    }
    let out = unsafe { lumia_list_sort_by_keys(xs, keys) };
    assert_eq!(out, xs, "unique sortBy must permute in place");
    assert_eq!(list_get_of(out, 0), 10);
    assert_eq!(list_get_of(out, 1), 20);
    assert_eq!(list_get_of(out, 2), 30);
}

#[test]
fn shared_sort_by_keys_does_not_mutate_source() {
    let mut xs = lumia_list_empty();
    for v in [30, 10, 20] {
        xs = unsafe { lumia_list_append(xs, v) };
    }
    let mut keys = lumia_list_empty();
    for k in [3, 1, 2] {
        keys = unsafe { lumia_list_append(keys, k) };
    }
    unsafe { lumia_list_retain(xs) };
    let out = unsafe { lumia_list_sort_by_keys(xs, keys) };
    assert_ne!(out, xs);
    assert_eq!(list_get_of(xs, 0), 30);
    assert_eq!(list_get_of(out, 0), 10);
}

#[test]
fn iota_reverse_is_dense_int_list() {
    use lumia_abi::list_elem_is_int;
    let r = lumia_range(10, 13);
    let rev = unsafe { lumia_list_reverse(r) };
    assert_eq!(list_len_of(rev), 3);
    assert_eq!(list_get_of(rev, 0), 12);
    assert_eq!(list_get_of(rev, 2), 10);
    unsafe {
        assert!(list_elem_is_int((*header_from_payload(rev)).type_id));
    }
}

#[test]
fn iota_concat_adjacent_stays_iota() {
    let a = lumia_range(0, 5);
    let b = lumia_range(5, 10);
    let c = unsafe { lumia_list_concat(a, b) };
    unsafe {
        assert_eq!((*header_from_payload(c)).type_id, TYPE_LIST_IOTA);
    }
    assert_eq!(list_len_of(c), 10);
    assert_eq!(list_get_of(c, 0), 0);
    assert_eq!(list_get_of(c, 9), 9);
}

#[test]
fn iota_concat_reverse_order_materializes() {
    let a = lumia_range(5, 10);
    let b = lumia_range(0, 5);
    let c = unsafe { lumia_list_concat(a, b) };
    unsafe {
        assert_ne!((*header_from_payload(c)).type_id, TYPE_LIST_IOTA);
    }
    assert_eq!(list_len_of(c), 10);
    assert_eq!(list_get_of(c, 0), 5);
    assert_eq!(list_get_of(c, 4), 9);
    assert_eq!(list_get_of(c, 5), 0);
    assert_eq!(list_get_of(c, 9), 4);
}

#[test]
fn unique_concat_appends_in_place() {
    let mut a = lumia_list_empty();
    a = unsafe { lumia_list_append(a, 1) };
    let mut b = lumia_list_empty();
    b = unsafe { lumia_list_append(b, 2) };
    let c = unsafe { lumia_list_concat(a, b) };
    assert_eq!(
        c, a,
        "unique concat with spare capacity must append in place"
    );
    assert_eq!(list_len_of(c), 2);
    assert_eq!(list_get_of(c, 0), 1);
    assert_eq!(list_get_of(c, 1), 2);
}

#[test]
fn concat_both_empty_preserves_bool_tid() {
    use lumia_abi::{list_elem_is_bool, TYPE_LIST_BOOL};
    let a = unsafe { lumia_list_take(crate::list::lumia_list_append(lumia_list_empty(), 1), 0) };
    // Untagged empty++empty stays immortal.
    let plain = unsafe { lumia_list_concat(lumia_list_empty(), lumia_list_empty()) };
    assert_eq!(plain, lumia_list_empty());
    // Bool-tagged empty ++ Bool-tagged empty keeps TID_B_KEY.
    let b = crate::ensure::alloc_empty_container(TYPE_LIST_BOOL);
    let c = crate::ensure::alloc_empty_container(TYPE_LIST_BOOL);
    let out = unsafe { lumia_list_concat(b, c) };
    unsafe {
        assert!(list_elem_is_bool((*header_from_payload(out)).type_id));
        assert_eq!(list_len_of(out), 0);
    }
    let _ = a;
}
