use super::*;

#[test]
fn map_unique_linear_update_in_place() {
    use crate::map_set::{lumi_map_get, lumi_map_set, map_count};
    let mut m = ptr::null_mut();
    lumi_root_push(&mut m as *mut *mut u8);
    m = lumi_map_set(m, 1, 10);
    m = lumi_map_set(m, 2, 20);
    let before = m;
    m = lumi_map_set(m, 1, 99);
    assert_eq!(m, before, "unique linear update should reuse buffer");
    assert_eq!(map_count(m), 2);
    let opt = lumi_map_get(m, 1, 0, 1);
    unsafe {
        assert_eq!(*(opt as *const i64), 0);
        assert_eq!(*(opt as *const i64).add(1), 99);
    }
    lumi_root_pop();
}

#[test]
fn map_unique_hash_upsert_in_place() {
    use crate::map_set::{lumi_map_set, map_count, map_is_hash, map_is_overlay};
    let mut m = ptr::null_mut();
    lumi_root_push(&mut m as *mut *mut u8);
    for i in 0..12 {
        m = lumi_map_set(m, i, i);
    }
    assert!(map_is_hash(m));
    let before = m;
    m = lumi_map_set(m, 3, 333);
    assert_eq!(m, before, "unique hash update should not allocate overlay");
    assert!(!map_is_overlay(m));
    m = lumi_map_set(m, 100, 1);
    assert_eq!(m, before, "unique hash insert should upsert in place when load allows");
    assert_eq!(map_count(m), 13);
    lumi_root_pop();
}

#[test]
fn map_shared_hash_still_uses_overlay() {
    use crate::common::list_rc_retain;
    use crate::map_set::{lumi_map_set, map_is_hash, map_is_overlay};
    let mut m = ptr::null_mut();
    lumi_root_push(&mut m as *mut *mut u8);
    for i in 0..10 {
        m = lumi_map_set(m, i, i);
    }
    assert!(map_is_hash(m));
    list_rc_retain(m); // simulate alias
    m = lumi_map_set(m, 100, 1);
    assert!(map_is_overlay(m), "shared hash set must use overlay");
    lumi_root_pop();
}

#[test]
fn map_promotes_to_hash_and_looks_up() {
    let mut m: *mut u8 = ptr::null_mut();
    lumi_root_push(&mut m as *mut *mut u8);
    for i in 0..20 {
        m = lumi_map_set(m, i, i * 10);
    }
    assert!(!m.is_null());
    assert!(map_is_hash(m) || map_is_overlay(m));
    assert_eq!(map_count(m), 20);
    for i in 0..20 {
        assert_eq!(lumi_map_contains(m, i), 1);
        let opt = lumi_map_get(m, i, 0, 1);
        // Some(v) tag 0 with field
        unsafe {
            let base = opt as *const i64;
            assert_eq!(*base, 0);
            assert_eq!(*base.add(1), i * 10);
        }
    }
    assert_eq!(lumi_map_contains(m, 99), 0);
    m = lumi_map_remove(m, 5);
    assert_eq!(lumi_map_contains(m, 5), 0);
    assert_eq!(map_count(m), 19);
    // Still insertion-ordered keys without 5
    let keys = lumi_map_keys(m);
    unsafe {
        assert_eq!(*(keys as *const i64), 19);
        assert_eq!(*((keys as *const i64).add(1)), 0);
    }
    lumi_root_pop();
}

#[test]
fn map_overlay_set_avoids_full_clone() {
    use crate::common::list_rc_retain;
    let mut m: *mut u8 = ptr::null_mut();
    lumi_root_push(&mut m as *mut *mut u8);
    for i in 0..9 {
        m = lumi_map_set(m, i, i);
    }
    assert!(
        map_is_hash(m),
        "expected hash after promoting past small max"
    );
    // Overlay is for *shared* hash tables; unique owners upsert in place.
    list_rc_retain(m);
    m = lumi_map_set(m, 100, 42);
    assert!(map_is_overlay(m));
    assert_eq!(map_count(m), 10);
    assert_eq!(lumi_map_contains(m, 100), 1);
    assert_eq!(lumi_map_contains(m, 3), 1);
    // Another set on a unique overlay extends delta in place.
    m = lumi_map_set(m, 101, 7);
    assert!(map_is_overlay(m));
    unsafe {
        assert_eq!(map_overlay_dn(m), 2);
    }
    assert_eq!(map_count(m), 11);
    assert_eq!(lumi_map_contains(m, 101), 1);
    lumi_root_pop();
}

#[test]
fn set_promotes_to_hash_and_contains() {
    let mut s: *mut u8 = ptr::null_mut();
    lumi_root_push(&mut s as *mut *mut u8);
    for i in 0..20 {
        s = lumi_set_insert(s, i);
    }
    assert!(!s.is_null());
    assert!(set_is_hash(s));
    assert_eq!(unsafe { *(s as *const i64) }, 20);
    for i in 0..20 {
        assert_eq!(lumi_set_contains(s, i), 1);
        assert_eq!(unsafe { set_elem_at(s, i as usize) }, i);
    }
    assert_eq!(lumi_set_contains(s, 99), 0);
    s = lumi_set_remove(s, 5);
    assert_eq!(lumi_set_contains(s, 5), 0);
    assert_eq!(unsafe { *(s as *const i64) }, 19);
    assert_eq!(unsafe { set_elem_at(s, 0) }, 0);
    assert_eq!(unsafe { set_elem_at(s, 5) }, 6);
    // Shrink far enough to demote to linear
    for i in 0..12 {
        s = lumi_set_remove(s, i);
    }
    assert!(!set_is_hash(s));
    assert_eq!(unsafe { *(s as *const i64) }, 8);
    lumi_root_pop();
}

#[test]
fn show_list_formats_elems() {
    let p = lumi_alloc(list_payload_bytes(2), TYPE_LIST);
    unsafe {
        *(p as *mut i64) = 2;
        *((p as *mut i64).add(1)) = 1;
        *((p as *mut i64).add(2)) = 2;
    }
    let s = lumi_show(p as i64);
    let text = with_str_bytes(s, |b| String::from_utf8_lossy(b).into_owned());
    assert_eq!(text, "[1, 2]");
}

#[test]
fn ensure_map_vf64_accepts_empty_assoc() {
    let m = lumi_alloc(8, TYPE_MAP_ASSOC);
    unsafe {
        *(m as *mut i64) = 0;
    }
    // Rust path (not extern C) so trap_abort can unwind for should_panic tests below.
    let m2 = crate::map_set::ensure_map_vf64(m);
    assert!(!m2.is_null());
    unsafe {
        assert_eq!((*header_from_payload(m2)).type_id, TYPE_MAP_ASSOC_VF64);
    }
    // Still assoc (no hash promotion).
    assert!(map_is_assoc(m2));
}

#[test]
fn ensure_map_f64_null_empty_identity() {
    let from_null = crate::map_set::ensure_map_f64(ptr::null_mut());
    assert!(!from_null.is_null());
    unsafe {
        assert_eq!((*header_from_payload(from_null)).type_id, TYPE_MAP_F64);
        assert_eq!(*(from_null as *const i64), 0);
    }

    let empty = lumi_alloc(8, TYPE_MAP);
    unsafe {
        *(empty as *mut i64) = 0;
    }
    let retagged = crate::map_set::ensure_map_f64(empty);
    unsafe {
        assert_eq!((*header_from_payload(retagged)).type_id, TYPE_MAP_F64);
    }

    let already = lumi_alloc(8, TYPE_MAP_F64);
    unsafe {
        *(already as *mut i64) = 0;
    }
    assert_eq!(crate::map_set::ensure_map_f64(already), already);
}

#[test]
fn ensure_map_vf64_null_and_identity() {
    let from_null = crate::map_set::ensure_map_vf64(ptr::null_mut());
    unsafe {
        assert_eq!((*header_from_payload(from_null)).type_id, TYPE_MAP_VF64);
    }
    let already = lumi_alloc(8, TYPE_MAP_VF64);
    unsafe {
        *(already as *mut i64) = 0;
    }
    assert_eq!(crate::map_set::ensure_map_vf64(already), already);
}

#[test]
fn ensure_set_f64_null_empty_identity() {
    let from_null = crate::map_set::ensure_set_f64(ptr::null_mut());
    unsafe {
        assert_eq!((*header_from_payload(from_null)).type_id, TYPE_SET_F64);
        assert_eq!(*(from_null as *const i64), 0);
    }
    let empty = lumi_alloc(8, TYPE_SET);
    unsafe {
        *(empty as *mut i64) = 0;
    }
    let retagged = crate::map_set::ensure_set_f64(empty);
    unsafe {
        assert_eq!((*header_from_payload(retagged)).type_id, TYPE_SET_F64);
    }
    let already = lumi_alloc(8, TYPE_SET_F64);
    unsafe {
        *(already as *mut i64) = 0;
    }
    assert_eq!(crate::map_set::ensure_set_f64(already), already);
}

#[test]
#[should_panic(expected = "ensure_map_f64 on non-empty Int-key map")]
fn ensure_map_f64_nonempty_traps() {
    let mut m = ptr::null_mut();
    lumi_root_push(&mut m as *mut *mut u8);
    m = lumi_map_set(m, 1, 2);
    let _ = crate::map_set::ensure_map_f64(m);
}

#[test]
#[should_panic(expected = "ensure_map_vf64 on non-empty non-Float-value map")]
fn ensure_map_vf64_nonempty_traps() {
    let mut m = ptr::null_mut();
    lumi_root_push(&mut m as *mut *mut u8);
    m = lumi_map_set(m, 1, 2);
    let _ = crate::map_set::ensure_map_vf64(m);
}

#[test]
#[should_panic(expected = "ensure_set_f64 on non-empty Int-elem set")]
fn ensure_set_f64_nonempty_traps() {
    let mut s = ptr::null_mut();
    lumi_root_push(&mut s as *mut *mut u8);
    s = lumi_set_insert(s, 1);
    let _ = crate::map_set::ensure_set_f64(s);
}
