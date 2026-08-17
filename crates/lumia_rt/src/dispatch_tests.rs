use super::*;
use crate::common::header_from_payload;
use crate::list::{lumia_list_append, lumia_list_empty, lumia_list_len};
use crate::map_set::{lumia_map_set, lumia_set_insert};
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
fn get_list_and_map_option() {
    unsafe {
        let xs = lumia_list_append(lumia_list_empty(), 42);
        assert_eq!(lumia_get(xs, 0, 0, 1), 42);
        let mut m = ptr::null_mut();
        m = lumia_map_set(m, 3, 9);
        let opt = lumia_get(m, 3, 0, 1);
        let base = opt as *const i64;
        assert_eq!(*base, 0);
        assert_eq!(*base.add(1), 9);
        let none = lumia_get(m, 99, 0, 1);
        assert_eq!(*(none as *const i64), 1);
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
