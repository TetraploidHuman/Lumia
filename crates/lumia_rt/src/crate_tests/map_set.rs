use super::*;

#[test]
fn map_promotes_to_hash_and_looks_up() {
    let mut m: *mut u8 = ptr::null_mut();
    lumia_root_push(&mut m as *mut *mut u8);
    for i in 0..20 {
        m = lumia_map_set(m, i, i * 10);
    }
    assert!(!m.is_null());
    assert!(map_is_hash(m) || map_is_overlay(m));
    assert_eq!(map_count(m), 20);
    for i in 0..20 {
        assert_eq!(lumia_map_contains(m, i), 1);
        let opt = lumia_map_get(m, i, 0, 1);
        // Some(v) tag 0 with field
        unsafe {
            let base = opt as *const i64;
            assert_eq!(*base, 0);
            assert_eq!(*base.add(1), i * 10);
        }
    }
    assert_eq!(lumia_map_contains(m, 99), 0);
    m = lumia_map_remove(m, 5);
    assert_eq!(lumia_map_contains(m, 5), 0);
    assert_eq!(map_count(m), 19);
    // Still insertion-ordered keys without 5
    let keys = lumia_map_keys(m);
    unsafe {
        assert_eq!(*(keys as *const i64), 19);
        assert_eq!(*((keys as *const i64).add(1)), 0);
    }
    lumia_root_pop();
}

#[test]
fn map_overlay_set_avoids_full_clone() {
    let mut m: *mut u8 = ptr::null_mut();
    lumia_root_push(&mut m as *mut *mut u8);
    for i in 0..9 {
        m = lumia_map_set(m, i, i);
    }
    assert!(
        map_is_hash(m),
        "expected hash after promoting past small max"
    );
    m = lumia_map_set(m, 100, 42);
    assert!(map_is_overlay(m));
    assert_eq!(map_count(m), 10);
    assert_eq!(lumia_map_contains(m, 100), 1);
    assert_eq!(lumia_map_contains(m, 3), 1);
    // Another set extends delta (still overlay).
    m = lumia_map_set(m, 101, 7);
    assert!(map_is_overlay(m));
    unsafe {
        assert_eq!(map_overlay_dn(m), 2);
    }
    assert_eq!(map_count(m), 11);
    assert_eq!(lumia_map_contains(m, 101), 1);
    lumia_root_pop();
}

#[test]
fn set_promotes_to_hash_and_contains() {
    let mut s: *mut u8 = ptr::null_mut();
    lumia_root_push(&mut s as *mut *mut u8);
    for i in 0..20 {
        s = lumia_set_insert(s, i);
    }
    assert!(!s.is_null());
    assert!(set_is_hash(s));
    assert_eq!(unsafe { *(s as *const i64) }, 20);
    for i in 0..20 {
        assert_eq!(lumia_set_contains(s, i), 1);
        assert_eq!(unsafe { set_elem_at(s, i as usize) }, i);
    }
    assert_eq!(lumia_set_contains(s, 99), 0);
    s = lumia_set_remove(s, 5);
    assert_eq!(lumia_set_contains(s, 5), 0);
    assert_eq!(unsafe { *(s as *const i64) }, 19);
    assert_eq!(unsafe { set_elem_at(s, 0) }, 0);
    assert_eq!(unsafe { set_elem_at(s, 5) }, 6);
    // Shrink far enough to demote to linear
    for i in 0..12 {
        s = lumia_set_remove(s, i);
    }
    assert!(!set_is_hash(s));
    assert_eq!(unsafe { *(s as *const i64) }, 8);
    lumia_root_pop();
}

#[test]
fn show_list_formats_elems() {
    let p = lumia_alloc(list_payload_bytes(2), TYPE_LIST);
    unsafe {
        *(p as *mut i64) = 2;
        *((p as *mut i64).add(1)) = 1;
        *((p as *mut i64).add(2)) = 2;
    }
    let s = lumia_show(p as i64);
    let text = with_str_bytes(s, |b| String::from_utf8_lossy(b).into_owned());
    assert_eq!(text, "[1, 2]");
}

#[test]
fn ensure_map_vf64_accepts_empty_assoc() {
    let m = lumia_alloc(8, TYPE_MAP_ASSOC);
    unsafe {
        *(m as *mut i64) = 0;
    }
    let m2 = lumia_ensure_map_vf64(m);
    assert!(!m2.is_null());
    unsafe {
        assert_eq!((*header_from_payload(m2)).type_id, TYPE_MAP_ASSOC_VF64);
    }
    // Still assoc (no hash promotion).
    assert!(map_is_assoc(m2));
}
