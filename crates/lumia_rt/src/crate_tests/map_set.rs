use super::*;

#[test]
fn map_promotes_to_hash_and_looks_up() {
    let mut m: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    for i in 0..20 {
        m = unsafe { lumia_map_set(m, i, i * 10) };
    }
    assert!(!m.is_null());
    assert!(map_is_hash(m) || map_is_overlay(m));
    assert_eq!(map_count(m), 20);
    for i in 0..20 {
        assert_eq!(unsafe { lumia_map_contains(m, i) }, 1);
        let opt = unsafe { lumia_map_get(m, i, 0, 1, 0, 0) };
        // Some(v) tag 0 with field
        unsafe {
            let base = opt as *const i64;
            assert_eq!(*base, 0);
            assert_eq!(*base.add(1), i * 10);
        }
    }
    assert_eq!(unsafe { lumia_map_contains(m, 99) }, 0);
    m = unsafe { lumia_map_remove(m, 5) };
    assert_eq!(unsafe { lumia_map_contains(m, 5) }, 0);
    assert_eq!(map_count(m), 19);
    // Still insertion-ordered keys without 5
    let keys = unsafe { lumia_map_keys(m) };
    unsafe {
        assert_eq!(*(keys as *const i64), 19);
        assert_eq!(*((keys as *const i64).add(1)), 0);
    }
    lumia_root_pop();
}

#[test]
fn map_overlay_set_avoids_full_clone() {
    let mut m: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    for i in 0..9 {
        m = unsafe { lumia_map_set(m, i, i) };
    }
    assert!(
        map_is_hash(m),
        "expected hash after promoting past small max"
    );
    // Unique hash with spare load inserts in place; Overlay is for shared parents.
    let hash = m;
    unsafe { lumia_list_retain(hash) };
    m = unsafe { lumia_map_set(hash, 100, 42) };
    assert!(map_is_overlay(m));
    assert_eq!(map_count(m), 10);
    assert_eq!(unsafe { lumia_map_contains(m, 100) }, 1);
    assert_eq!(unsafe { lumia_map_contains(m, 3) }, 1);
    // Another set extends delta (still overlay).
    m = unsafe { lumia_map_set(m, 101, 7) };
    assert!(map_is_overlay(m));
    unsafe {
        assert_eq!(map_overlay_dn(m), 2);
    }
    assert_eq!(map_count(m), 11);
    assert_eq!(unsafe { lumia_map_contains(m, 101) }, 1);
    lumia_root_pop();
}

#[test]
fn map_unique_hash_inserts_in_place() {
    let mut m: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    for i in 0..9 {
        m = unsafe { lumia_map_set(m, i, i) };
    }
    assert!(map_is_hash(m));
    let hash = m;
    m = unsafe { lumia_map_set(hash, 100, 42) };
    assert_eq!(m, hash, "unique hash with spare load must insert in place");
    assert!(map_is_hash(m));
    assert_eq!(map_count(m), 10);
    lumia_root_pop();
}

#[test]
fn map_unique_hash_remove_is_in_place() {
    let mut m: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    // Stay under hash load-factor so unique inserts do not spill to Overlay.
    for i in 0..12 {
        m = unsafe { lumia_map_set(m, i, i * 10) };
    }
    assert!(map_is_hash(m));
    let hash = m;
    m = unsafe { lumia_map_remove(hash, 5) };
    assert_eq!(m, hash, "unique hash remove must tomb in place");
    assert!(map_is_hash(m));
    assert_eq!(map_count(m), 11);
    assert_eq!(unsafe { lumia_map_contains(m, 5) }, 0);
    assert_eq!(unsafe { lumia_map_contains(m, 0) }, 1);
    assert_eq!(unsafe { lumia_map_contains(m, 11) }, 1);
    m = unsafe { lumia_map_set(m, 5, 50) };
    assert_eq!(
        m, hash,
        "unique hash reinsert after tomb must reuse the table"
    );
    assert_eq!(unsafe { lumia_map_contains(m, 5) }, 1);
    lumia_root_pop();
}

#[test]
fn map_unique_hash_demote_is_in_place() {
    let mut m: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    for i in 0..12 {
        m = unsafe { lumia_map_set(m, i, i * 10) };
    }
    assert!(map_is_hash(m));
    let hash = m;
    for i in 0..4 {
        m = unsafe { lumia_map_remove(m, i) };
        assert_eq!(m, hash);
    }
    assert!(
        !map_is_hash(m),
        "unique hash must demote to linear at small n"
    );
    assert!(!map_is_overlay(m));
    assert_eq!(map_count(m), 8);
    assert_eq!(unsafe { lumia_map_contains(m, 0) }, 0);
    assert_eq!(unsafe { lumia_map_contains(m, 4) }, 1);
    lumia_root_pop();
}

#[test]
fn set_unique_hash_grows_without_overlay() {
    let mut s: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut s as *mut *mut u8) };
    for i in 0..64 {
        s = unsafe { lumia_set_insert(s, i) };
        assert!(
            !set_is_overlay(s),
            "unique set builder must grow hash in place, not overlay (i={i})"
        );
    }
    assert!(set_is_hash(s));
    assert_eq!(set_count(s), 64);
    for i in 0..64 {
        assert_eq!(unsafe { lumia_set_contains(s, i) }, 1);
    }
    lumia_root_pop();
}

#[test]
fn map_unique_hash_grows_without_overlay() {
    let mut m: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    for i in 0..64 {
        m = unsafe { lumia_map_set(m, i, i * 3) };
        assert!(
            !map_is_overlay(m),
            "unique map builder must grow hash in place, not overlay (i={i})"
        );
    }
    assert!(map_is_hash(m));
    assert_eq!(map_count(m), 64);
    assert_eq!(unsafe { lumia_map_contains(m, 63) }, 1);
    lumia_root_pop();
}

#[test]
fn set_unique_hash_remove_is_in_place() {
    let mut s: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut s as *mut *mut u8) };
    for i in 0..12 {
        s = unsafe { lumia_set_insert(s, i) };
    }
    assert!(set_is_hash(s));
    let hash = s;
    s = unsafe { lumia_set_remove(hash, 5) };
    assert_eq!(s, hash, "unique hash set remove must tomb in place");
    assert!(set_is_hash(s));
    assert_eq!(set_count(s), 11);
    assert_eq!(unsafe { lumia_set_contains(s, 5) }, 0);
    assert_eq!(unsafe { set_elem_at(s, 5) }, 6);
    lumia_root_pop();
}

#[test]
fn set_promotes_to_hash_and_contains() {
    let mut s: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut s as *mut *mut u8) };
    for i in 0..20 {
        s = unsafe { lumia_set_insert(s, i) };
    }
    assert!(!s.is_null());
    assert!(
        set_is_hash(s) || set_is_overlay(s),
        "expected hash or overlay after promoting past small max"
    );
    assert_eq!(set_count(s), 20);
    for i in 0..20 {
        assert_eq!(unsafe { lumia_set_contains(s, i) }, 1);
        assert_eq!(unsafe { set_elem_at(s, i as usize) }, i);
    }
    assert_eq!(unsafe { lumia_set_contains(s, 99) }, 0);
    s = unsafe { lumia_set_remove(s, 5) };
    assert_eq!(unsafe { lumia_set_contains(s, 5) }, 0);
    assert_eq!(set_count(s), 19);
    assert_eq!(unsafe { set_elem_at(s, 0) }, 0);
    assert_eq!(unsafe { set_elem_at(s, 5) }, 6);
    // Shrink far enough to demote to linear
    for i in 0..12 {
        s = unsafe { lumia_set_remove(s, i) };
    }
    assert!(!set_is_hash(s) && !set_is_overlay(s));
    assert_eq!(set_count(s), 8);
    lumia_root_pop();
}

#[test]
fn set_overlay_insert_avoids_full_clone() {
    let mut s: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut s as *mut *mut u8) };
    for i in 0..9 {
        s = unsafe { lumia_set_insert(s, i) };
    }
    assert!(
        set_is_hash(s),
        "expected hash after promoting past small max"
    );
    let hash = s;
    unsafe { lumia_list_retain(hash) };
    s = unsafe { lumia_set_insert(hash, 100) };
    assert!(set_is_overlay(s));
    assert_eq!(set_count(s), 10);
    assert_eq!(unsafe { lumia_set_contains(s, 100) }, 1);
    assert_eq!(unsafe { lumia_set_contains(s, 3) }, 1);
    s = unsafe { lumia_set_insert(s, 101) };
    assert!(set_is_overlay(s));
    unsafe {
        assert_eq!(set_overlay_dn(s), 2);
    }
    assert_eq!(set_count(s), 11);
    assert_eq!(unsafe { lumia_set_contains(s, 101) }, 1);
    lumia_root_pop();
}

#[test]
fn set_algebra_union_intersect_diff() {
    let mut a: *mut u8 = ptr::null_mut();
    let mut b: *mut u8 = ptr::null_mut();
    let mut u: *mut u8 = ptr::null_mut();
    let mut inter: *mut u8 = ptr::null_mut();
    let mut d: *mut u8 = ptr::null_mut();
    unsafe { lumia_root_push(&mut a as *mut *mut u8) };
    unsafe { lumia_root_push(&mut b as *mut *mut u8) };
    unsafe { lumia_root_push(&mut u as *mut *mut u8) };
    unsafe { lumia_root_push(&mut inter as *mut *mut u8) };
    unsafe { lumia_root_push(&mut d as *mut *mut u8) };
    for i in 0..20 {
        a = unsafe { lumia_set_insert(a, i) };
    }
    for i in 10..30 {
        b = unsafe { lumia_set_insert(b, i) };
    }
    u = unsafe { lumia_set_union(a, b) };
    assert_eq!(set_count(u), 30);
    assert_eq!(unsafe { lumia_set_contains(u, 0) }, 1);
    assert_eq!(unsafe { lumia_set_contains(u, 29) }, 1);
    inter = unsafe { lumia_set_intersect(a, b) };
    assert_eq!(set_count(inter), 10);
    assert_eq!(unsafe { lumia_set_contains(inter, 10) }, 1);
    assert_eq!(unsafe { lumia_set_contains(inter, 9) }, 0);
    d = unsafe { lumia_set_diff(a, b) };
    assert_eq!(set_count(d), 10);
    assert_eq!(unsafe { lumia_set_contains(d, 0) }, 1);
    assert_eq!(unsafe { lumia_set_contains(d, 10) }, 0);
    for _ in 0..5 {
        lumia_root_pop();
    }
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

    let empty = lumia_alloc(8, TYPE_MAP);
    unsafe {
        *(empty as *mut i64) = 0;
    }
    let retagged = crate::map_set::ensure_map_f64(empty);
    unsafe {
        assert_eq!((*header_from_payload(retagged)).type_id, TYPE_MAP_F64);
    }

    let already = lumia_alloc(8, TYPE_MAP_F64);
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
    let already = lumia_alloc(8, TYPE_MAP_VF64);
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
    let empty = lumia_alloc(8, TYPE_SET);
    unsafe {
        *(empty as *mut i64) = 0;
    }
    let retagged = crate::map_set::ensure_set_f64(empty);
    unsafe {
        assert_eq!((*header_from_payload(retagged)).type_id, TYPE_SET_F64);
    }
    let already = lumia_alloc(8, TYPE_SET_F64);
    unsafe {
        *(already as *mut i64) = 0;
    }
    assert_eq!(crate::map_set::ensure_set_f64(already), already);
}

#[test]
#[should_panic(expected = "ensure_map_f64 on non-empty Int-key map")]
fn ensure_map_f64_nonempty_traps() {
    let mut m = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    m = unsafe { lumia_map_set(m, 1, 2) };
    let _ = crate::map_set::ensure_map_f64(m);
}

#[test]
#[should_panic(expected = "ensure_map_vf64 on non-empty non-Float-value map")]
fn ensure_map_vf64_nonempty_traps() {
    let mut m = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    m = unsafe { lumia_map_set(m, 1, 2) };
    let _ = crate::map_set::ensure_map_vf64(m);
}

#[test]
#[should_panic(expected = "ensure_set_f64 on non-empty Int-elem set")]
fn ensure_set_f64_nonempty_traps() {
    let mut s = ptr::null_mut();
    unsafe { lumia_root_push(&mut s as *mut *mut u8) };
    s = unsafe { lumia_set_insert(s, 1) };
    let _ = crate::map_set::ensure_set_f64(s);
}

#[test]
fn map_get_none_is_immortal_singleton() {
    let a = unsafe { lumia_map_get(ptr::null_mut(), 0, 0, 1, 0, 0) };
    let b = unsafe { lumia_map_get(ptr::null_mut(), 99, 0, 1, 0, 0) };
    assert_eq!(a, b, "same none_tag must reuse immortal None");
    unsafe {
        assert_eq!(*(a as *const i64), 1);
    }
    // Survives GC (perm-rooted like empty list).
    lumia_gc_collect();
    let c = unsafe { lumia_map_get(ptr::null_mut(), 1, 0, 1, 0, 0) };
    assert_eq!(c, a);
    // Distinct tags get distinct singletons.
    let d = unsafe { lumia_map_get(ptr::null_mut(), 0, 0, 7, 0, 0) };
    assert_ne!(d, a);
    unsafe {
        assert_eq!(*(d as *const i64), 7);
    }
}

#[test]
fn map_get_float_val_sets_option_float_mask() {
    // VF64 map → Some(float) must carry `_pad` bit0 for show (not IEEE-as-Int).
    let mut m = crate::map_set::ensure_map_vf64(ptr::null_mut());
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    let bits = 2.5f64.to_bits() as i64;
    m = unsafe { lumia_map_set(m, 2, bits) };
    let opt = unsafe { lumia_map_get(m, 2, 0, 1, 0, 0) };
    unsafe {
        assert_eq!(*(opt as *const i64), 0);
        assert_eq!(*(opt as *const i64).add(1), bits);
        assert_eq!((*header_from_payload(opt))._pad, 0b1);
        assert_eq!(
            lumia_abi::adt_show_kind((*header_from_payload(opt)).type_id),
            0
        );
    }
    let shown = lumia_show(opt as i64);
    let text = with_str_bytes(shown, |b| String::from_utf8_lossy(b).into_owned());
    assert!(
        text.contains("2.5"),
        "Option[Float] show should print 2.5, got {text:?}"
    );
    lumia_root_pop();
}

#[test]
fn map_get_bool_mask_and_show_kind() {
    let mut m = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    m = unsafe { lumia_map_set(m, 1, 1) }; // true
                                           // Register a fake Option show-kind so nested show can resolve names.
    let names: [*const u8; 2] = [b"Some\0".as_ptr(), b"None\0".as_ptr()];
    unsafe {
        crate::adt_show::lumia_adt_register_show(9, names.as_ptr(), 2);
    }
    let opt = unsafe { lumia_map_get(m, 1, 0, 1, 9, 0b1) };
    unsafe {
        assert_eq!(*(opt as *const i64), 0);
        assert_eq!(*(opt as *const i64).add(1), 1);
        assert_eq!(
            crate::common::adt_bool_mask((*header_from_payload(opt))._pad),
            0b1
        );
        assert_eq!(
            lumia_abi::adt_show_kind((*header_from_payload(opt)).type_id),
            9
        );
    }
    let shown = lumia_show(opt as i64);
    let text = with_str_bytes(shown, |b| String::from_utf8_lossy(b).into_owned());
    assert!(
        text.contains("Some") && text.contains("true"),
        "Option[Bool] nested show got {text:?}"
    );
    lumia_root_pop();
}

#[test]
fn map_items_float_pairs_set_float_mask() {
    let mut m = lumia_alloc(8, lumia_abi::TYPE_MAP_F64V);
    unsafe {
        *(m as *mut i64) = 0;
    }
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    let k = 1.5f64.to_bits() as i64;
    let v = 2.5f64.to_bits() as i64;
    m = unsafe { lumia_map_set(m, k, v) };
    let items = unsafe { lumia_map_items(m, 0) };
    unsafe {
        assert_eq!(*(items as *const i64), 1);
        let pair = *((items as *const i64).add(1)) as *mut u8;
        assert_eq!((*header_from_payload(pair))._pad, 0b11);
    }
    let shown = lumia_show(items as i64);
    let text = with_str_bytes(shown, |b| String::from_utf8_lossy(b).into_owned());
    assert!(
        text.contains("1.5") && text.contains("2.5"),
        "items() list show should print floats, got {text:?}"
    );
    lumia_root_pop();
}

#[test]
fn map_items_bool_pairs_set_bool_mask() {
    let mut m = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    m = unsafe { lumia_map_set(m, 1, 0) }; // true -> false
    let items = unsafe { lumia_map_items(m, 0b11) };
    unsafe {
        assert_eq!(*(items as *const i64), 1);
        let pair = *((items as *const i64).add(1)) as *mut u8;
        assert_eq!(
            crate::common::adt_bool_mask((*header_from_payload(pair))._pad),
            0b11
        );
    }
    let shown = lumia_show(items as i64);
    let text = with_str_bytes(shown, |b| String::from_utf8_lossy(b).into_owned());
    assert!(
        text.contains("true") && text.contains("false"),
        "items() list show should print bools, got {text:?}"
    );
    lumia_root_pop();
}

#[test]
fn show_set_bool_prints_true_false() {
    let mut s = ptr::null_mut();
    unsafe { lumia_root_push(&mut s as *mut *mut u8) };
    s = unsafe { lumia_set_insert(s, 1) };
    s = unsafe { lumia_set_insert(s, 0) };
    let shown = lumia_show_set_bool(s as i64, 1);
    let text = with_str_bytes(shown, |b| String::from_utf8_lossy(b).into_owned());
    assert!(
        text.contains("true") && text.contains("false"),
        "Set[Bool] show got {text:?}"
    );
    lumia_root_pop();
}

#[test]
fn show_map_bool_val_prints_true() {
    let mut m = ptr::null_mut();
    unsafe { lumia_root_push(&mut m as *mut *mut u8) };
    m = unsafe { lumia_map_set(m, 1, 1) };
    let shown = lumia_show_map_bool(m as i64, 0, 1);
    let text = with_str_bytes(shown, |b| String::from_utf8_lossy(b).into_owned());
    assert!(text.contains("true"), "Map[Int,Bool] show got {text:?}");
    lumia_root_pop();
}

#[test]
fn show_list_adt_bool_field() {
    // Some(true) tag0 field0=1 — typed bool_mask bit0.
    let opt = crate::map_set::alloc_adt_with_meta(0, &[1], 0, 0, 0);
    let list = lumia_alloc(list_payload_bytes(1), TYPE_LIST);
    unsafe {
        *(list as *mut i64) = 1;
        *((list as *mut i64).add(1)) = opt as i64;
    }
    let shown = lumia_show_list_adt(list as i64, 0, 0b1);
    let text = with_str_bytes(shown, |b| String::from_utf8_lossy(b).into_owned());
    assert!(
        text.contains("true"),
        "List[Option[Bool]] show got {text:?}"
    );
}
