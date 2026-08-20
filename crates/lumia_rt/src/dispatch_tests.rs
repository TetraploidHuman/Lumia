use super::*;
use crate::common::header_from_payload;
use crate::list::{lumia_list_append, lumia_list_empty, lumia_list_len, lumia_list_retain};
use crate::map_set::{
    lumia_map_remove, lumia_map_set, lumia_set_insert, lumia_set_remove, map_is_overlay,
    set_is_overlay,
};
use crate::string_io::lumia_alloc_string;
use std::ptr;

#[test]
fn len_null_and_list_map_set_string() {
    // SAFETY: null / freshly allocated RT payloads.
    unsafe {
        assert_eq!(lumia_len(ptr::null_mut()), 0);
        let xs = lumia_list_append(lumia_list_empty(), 1);
        assert_eq!(lumia_len(xs), 1);
        let mut m = ptr::null_mut();
        m = lumia_map_set(m, 1, 2);
        assert_eq!(lumia_len(m), 1);
        let mut s = ptr::null_mut();
        s = lumia_set_insert(s, 7);
        assert_eq!(lumia_len(s), 1);
        let st = lumia_alloc_string(b"hi".as_ptr(), 2);
        assert_eq!(lumia_len(st), 2);
    }
}

#[test]
fn len_set_overlay_uses_logical_count() {
    unsafe {
        let mut s = ptr::null_mut();
        for i in 0..9 {
            s = lumia_set_insert(s, i);
        }
        // Unique hash inserts in place; Overlay is for a shared parent.
        lumia_list_retain(s);
        s = lumia_set_insert(s, 100);
        assert!(set_is_overlay(s));
        assert_eq!(lumia_len(s), 10);
    }
}

#[test]
fn get_list_and_map_option() {
    unsafe {
        let xs = lumia_list_append(lumia_list_empty(), 42);
        assert_eq!(lumia_get(xs, 0, 0, 1, 0, 0), 42);
        let mut m = ptr::null_mut();
        m = lumia_map_set(m, 3, 9);
        let opt = lumia_get(m, 3, 0, 1, 0, 0);
        let base = opt as *const i64;
        assert_eq!(*base, 0);
        assert_eq!(*base.add(1), 9);
        let none = lumia_get(m, 99, 0, 1, 0, 0);
        assert_eq!(*(none as *const i64), 1);
    }
}

#[test]
fn get_null_empty_map_yields_none() {
    // `[:]` / `mapOf()` is a null shell — Option-tagged get must not trap.
    unsafe {
        let none = lumia_get(ptr::null_mut(), 0, 0, 1, 0, 0);
        assert_eq!(*(none as *const i64), 1);
        let again = lumia_get(ptr::null_mut(), 42, 0, 1, 0, 0);
        assert_eq!(none, again, "immortal None singleton");
    }
}

#[test]
fn map_set_unique_linear_updates_in_place() {
    unsafe {
        let m = lumia_map_set(ptr::null_mut(), 1, 10);
        let m2 = lumia_map_set(m, 1, 99);
        assert_eq!(m, m2, "unique linear map must update value in place");
        let m3 = lumia_map_set(m2, 2, 20);
        assert_eq!(m2, m3, "unique linear map must append with spare capacity");
    }
}

#[test]
fn map_set_unique_overlay_updates_in_place() {
    unsafe {
        let mut m = ptr::null_mut();
        for i in 0..8 {
            m = lumia_map_set(m, i, i);
        }
        let hash = lumia_map_set(m, 8, 8);
        // Shared hash forces Overlay; unique overlay then mutates in place.
        lumia_list_retain(hash);
        let ov1 = lumia_map_set(hash, 99, 1);
        assert!(map_is_overlay(ov1));
        let ov2 = lumia_map_set(ov1, 99, 2);
        assert_eq!(ov1, ov2, "unique overlay must update in place");
        let ov3 = lumia_map_set(ov2, 100, 3);
        assert_eq!(ov2, ov3, "unique overlay must append in place");
    }
}

#[test]
fn set_insert_existing_is_identity() {
    unsafe {
        let s = lumia_set_insert(ptr::null_mut(), 7);
        let s2 = lumia_set_insert(s, 7);
        assert_eq!(s, s2, "inserting an existing element must be identity");
        let s3 = lumia_set_insert(s2, 8);
        assert_eq!(s2, s3, "unique linear set must append with spare capacity");
    }
}

#[test]
fn map_remove_missing_is_identity() {
    unsafe {
        let mut m = lumia_map_set(ptr::null_mut(), 1, 10);
        m = lumia_map_set(m, 2, 20);
        let miss = lumia_map_remove(m, 99);
        assert_eq!(m, miss, "removing a missing key must be identity");
        let m2 = lumia_map_remove(m, 1);
        assert_eq!(m, m2, "unique linear map remove must compact in place");
        assert_eq!(lumia_len(m2), 1);
        assert_eq!(lumia_contains(m2, 2), 1);
        assert_eq!(lumia_contains(m2, 1), 0);
    }
}

#[test]
fn set_remove_missing_is_identity() {
    unsafe {
        let s = lumia_set_insert(ptr::null_mut(), 7);
        let s2 = lumia_set_remove(s, 99);
        assert_eq!(s, s2, "removing a missing elem must be identity");
        let s3 = lumia_set_insert(s2, 8);
        let s4 = lumia_set_remove(s3, 7);
        assert_eq!(s3, s4, "unique linear set remove must compact in place");
        assert_eq!(lumia_len(s4), 1);
        assert_eq!(lumia_contains(s4, 8), 1);
        assert_eq!(lumia_contains(s4, 7), 0);
    }
}

#[test]
fn overlay_remove_missing_is_identity() {
    unsafe {
        let mut m = ptr::null_mut();
        for i in 0..9 {
            m = lumia_map_set(m, i, i);
        }
        lumia_list_retain(m);
        let ov = lumia_map_set(m, 99, 1);
        assert!(map_is_overlay(ov));
        let miss = lumia_map_remove(ov, 12345);
        assert_eq!(ov, miss, "overlay remove miss must not materialize");
        let mut s = ptr::null_mut();
        for i in 0..9 {
            s = lumia_set_insert(s, i);
        }
        lumia_list_retain(s);
        let sov = lumia_set_insert(s, 99);
        assert!(set_is_overlay(sov));
        let smiss = lumia_set_remove(sov, 12345);
        assert_eq!(sov, smiss, "set overlay remove miss must not materialize");
    }
}

#[test]
fn overlay_remove_delta_only_skips_materialize() {
    unsafe {
        let mut m = ptr::null_mut();
        for i in 0..9 {
            m = lumia_map_set(m, i, i);
        }
        lumia_list_retain(m);
        let ov = lumia_map_set(m, 99, 1);
        assert!(map_is_overlay(ov));
        let ov2 = lumia_map_set(ov, 100, 2);
        assert_eq!(ov, ov2, "unique overlay append");
        let kept = lumia_map_remove(ov2, 99);
        assert_eq!(
            kept, ov2,
            "unique overlay-only remove must compact in place"
        );
        assert!(map_is_overlay(kept));
        assert_eq!(lumia_contains(kept, 99), 0);
        assert_eq!(lumia_contains(kept, 100), 1);
        assert_eq!(lumia_contains(kept, 0), 1);
        let parent = lumia_map_remove(kept, 100);
        assert_eq!(parent, m, "last overlay-only remove returns parent");
        assert!(!map_is_overlay(parent));

        let mut s = ptr::null_mut();
        for i in 0..9 {
            s = lumia_set_insert(s, i);
        }
        lumia_list_retain(s);
        let sov = lumia_set_insert(s, 99);
        assert!(set_is_overlay(sov));
        let back = lumia_set_remove(sov, 99);
        assert_eq!(back, s, "last set overlay-only remove returns parent");
        assert!(!set_is_overlay(back));
    }
}

#[test]
fn set_insert_unique_overlay_appends_in_place() {
    unsafe {
        let mut s = ptr::null_mut();
        for i in 0..8 {
            s = lumia_set_insert(s, i);
        }
        let hash = lumia_set_insert(s, 8);
        lumia_list_retain(hash);
        let ov1 = lumia_set_insert(hash, 99);
        assert!(set_is_overlay(ov1));
        let ov2 = lumia_set_insert(ov1, 100);
        assert_eq!(ov1, ov2, "unique set overlay must append in place");
    }
}

#[test]
fn contains_map_set_string() {
    unsafe {
        let mut m = ptr::null_mut();
        m = lumia_map_set(m, 1, 2);
        assert_eq!(lumia_contains(m, 1), 1);
        assert_eq!(lumia_contains(m, 2), 0);
        let mut s = ptr::null_mut();
        s = lumia_set_insert(s, 5);
        assert_eq!(lumia_contains(s, 5), 1);
        let needle = lumia_alloc_string(b"a".as_ptr(), 1);
        let hay = lumia_alloc_string(b"cat".as_ptr(), 3);
        assert_eq!(lumia_contains(hay, needle as i64), 1);
    }
}

#[test]
fn elems_list_identity_and_set_to_list() {
    unsafe {
        let xs = lumia_list_append(lumia_list_empty(), 1);
        assert_eq!(lumia_elems(xs), xs);
        let mut s = ptr::null_mut();
        s = lumia_set_insert(s, 3);
        s = lumia_set_insert(s, 4);
        let el = lumia_elems(s);
        assert_eq!(lumia_list_len(el), 2);
        assert_eq!((*header_from_payload(el)).type_id, crate::TYPE_LIST);
    }
}

#[test]
fn concat_string_vs_list() {
    unsafe {
        let a = lumia_alloc_string(b"a".as_ptr(), 1);
        let b = lumia_alloc_string(b"b".as_ptr(), 1);
        let ab = lumia_concat(a, b);
        assert_eq!(lumia_len(ab), 2);
        let xs = lumia_list_append(lumia_list_empty(), 1);
        let ys = lumia_list_append(lumia_list_empty(), 2);
        let zs = lumia_concat(xs, ys);
        assert_eq!(lumia_list_len(zs), 2);
    }
}
