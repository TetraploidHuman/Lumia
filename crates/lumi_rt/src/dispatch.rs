//! Polymorphic C ABI dispatch across List / Map / Set / String.

use crate::common::{
    header_from_payload, tid_base, trap_abort, GcInhibitGuard, TYPE_LIST, TYPE_LIST_IOTA,
    TYPE_STRING,
};
use crate::gc::{list_payload_bytes, lumi_alloc};
use crate::list::{
    force_heap_list, list_len_of, lumi_list_concat, lumi_list_get, lumi_list_set,
};
use crate::map_set::{
    lumi_map_contains, lumi_map_get, lumi_map_keys, lumi_map_remove, lumi_map_set,
    lumi_set_contains, lumi_set_remove, map_count, set_elem_at,
};
use crate::string_io::{lumi_str_concat, lumi_str_contains};
use lumi_abi::{is_list_tid, is_map_tid, is_set_tid};

#[no_mangle]
pub extern "C" fn lumi_len(obj: *mut u8) -> i64 {
    if obj.is_null() {
        return 0;
    }
    unsafe {
        let h = header_from_payload(obj);
        match (*h).type_id {
            TYPE_STRING => (*h).size as i64,
            tid if is_list_tid(tid) => list_len_of(obj),
            tid if is_set_tid(tid) => *(obj as *const i64),
            tid if is_map_tid(tid) => map_count(obj),
            _ => trap_abort(&format!("lumi: len on unsupported type {}", (*h).type_id)),
        }
    }
}
#[no_mangle]
pub extern "C" fn lumi_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    let ta = if a.is_null() {
        TYPE_LIST
    } else {
        unsafe { (*header_from_payload(a)).type_id }
    };
    let tb = if b.is_null() {
        ta
    } else {
        unsafe { (*header_from_payload(b)).type_id }
    };
    if ta == TYPE_STRING || tb == TYPE_STRING {
        if ta != TYPE_STRING || tb != TYPE_STRING {
            trap_abort("lumi: concat type mismatch");
        }
        return lumi_str_concat(a, b);
    }
    lumi_list_concat(a, b)
}
#[no_mangle]
pub extern "C" fn lumi_set(obj: *mut u8, key_or_index: i64, val: i64) -> *mut u8 {
    if obj.is_null() {
        return lumi_map_set(obj, key_or_index, val);
    }
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        tid if is_list_tid(tid) => lumi_list_set(obj, key_or_index, val),
        tid if is_map_tid(tid) => lumi_map_set(obj, key_or_index, val),
        _ => trap_abort(&format!("lumi: set on unsupported type_id={tid}")),
    }
}
#[no_mangle]
pub extern "C" fn lumi_elems(obj: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if obj.is_null() {
        let dest = lumi_alloc(8, TYPE_LIST);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        tid if tid_base(tid) == TYPE_LIST => obj,
        TYPE_LIST_IOTA => force_heap_list(obj),
        tid if is_set_tid(tid) => unsafe {
            let n = *(obj as *const i64);
            let nbytes = list_payload_bytes(n);
            let dest_tid = lumi_abi::list_type_id(lumi_abi::set_elem_is_float(tid));
            let dest = lumi_alloc(nbytes, dest_tid);
            let dst = dest as *mut i64;
            *dst = n;
            for i in 0..n as usize {
                *dst.add(1 + i) = set_elem_at(obj, i);
            }
            dest
        },
        tid if is_map_tid(tid) => lumi_map_keys(obj),
        other => trap_abort(&format!("lumi: elems unsupported type_id={other}")),
    }
}
#[no_mangle]
pub extern "C" fn lumi_remove(obj: *mut u8, key_or_elem: i64) -> *mut u8 {
    if obj.is_null() {
        // Ambiguous empty — prefer Map (same historical default as typed `remove`).
        return lumi_map_remove(obj, key_or_elem);
    }
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        tid if is_map_tid(tid) => lumi_map_remove(obj, key_or_elem),
        tid if is_set_tid(tid) => lumi_set_remove(obj, key_or_elem),
        _ => trap_abort(&format!("lumi: remove on unsupported type_id={tid}")),
    }
}

/// Dispatch get: List/Set by index → i64 elem; Map by key → Option ADT ptr as i64.
#[no_mangle]
pub extern "C" fn lumi_get(obj: *mut u8, key_or_index: i64, some_tag: i64, none_tag: i64) -> i64 {
    if obj.is_null() {
        trap_abort("lumi: get on null");
    }
    let h = header_from_payload(obj);
    unsafe {
        match (*h).type_id {
            tid if is_list_tid(tid) => lumi_list_get(obj, key_or_index),
            tid if is_set_tid(tid) => {
                let n = *(obj as *const i64);
                if key_or_index < 0 || key_or_index >= n {
                    trap_abort("lumi: set get OOB");
                }
                set_elem_at(obj, key_or_index as usize)
            }
            tid if is_map_tid(tid) => {
                let opt = lumi_map_get(obj, key_or_index, some_tag, none_tag);
                opt as i64
            }
            other => trap_abort(&format!("lumi: get unsupported type_id {other}")),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumi_contains(obj: *mut u8, key: i64) -> i64 {
    if obj.is_null() {
        return 0;
    }
    let h = header_from_payload(obj);
    unsafe {
        match (*h).type_id {
            tid if is_map_tid(tid) => lumi_map_contains(obj, key),
            tid if is_set_tid(tid) => lumi_set_contains(obj, key),
            TYPE_STRING => lumi_str_contains(obj, key as *mut u8),
            other => trap_abort(&format!("lumi: contains unsupported type_id {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::header_from_payload;
    use crate::list::{lumi_list_append, lumi_list_empty, lumi_list_len};
    use crate::map_set::{lumi_map_set, lumi_set_insert};
    use crate::string_io::lumi_alloc_string;
    use std::ptr;

    #[test]
    fn len_null_and_list_map_set_string() {
        assert_eq!(lumi_len(ptr::null_mut()), 0);
        let xs = lumi_list_append(lumi_list_empty(), 1);
        assert_eq!(lumi_len(xs), 1);
        let mut m = ptr::null_mut();
        m = lumi_map_set(m, 1, 2);
        assert_eq!(lumi_len(m), 1);
        let mut s = ptr::null_mut();
        s = lumi_set_insert(s, 7);
        assert_eq!(lumi_len(s), 1);
        let st = lumi_alloc_string(b"hi".as_ptr(), 2);
        assert_eq!(lumi_len(st), 2);
    }

    #[test]
    fn get_list_and_map_option() {
        let xs = lumi_list_append(lumi_list_empty(), 42);
        assert_eq!(lumi_get(xs, 0, 0, 1), 42);
        let mut m = ptr::null_mut();
        m = lumi_map_set(m, 3, 9);
        let opt = lumi_get(m, 3, 0, 1);
        unsafe {
            let base = opt as *const i64;
            assert_eq!(*base, 0);
            assert_eq!(*base.add(1), 9);
        }
        let none = lumi_get(m, 99, 0, 1);
        unsafe {
            assert_eq!(*(none as *const i64), 1);
        }
    }

    #[test]
    fn contains_map_set_string() {
        let mut m = ptr::null_mut();
        m = lumi_map_set(m, 1, 2);
        assert_eq!(lumi_contains(m, 1), 1);
        assert_eq!(lumi_contains(m, 2), 0);
        let mut s = ptr::null_mut();
        s = lumi_set_insert(s, 5);
        assert_eq!(lumi_contains(s, 5), 1);
        let needle = lumi_alloc_string(b"a".as_ptr(), 1);
        let hay = lumi_alloc_string(b"cat".as_ptr(), 3);
        assert_eq!(lumi_contains(hay, needle as i64), 1);
    }

    #[test]
    fn elems_list_identity_and_set_to_list() {
        let xs = lumi_list_append(lumi_list_empty(), 1);
        assert_eq!(lumi_elems(xs), xs);
        let mut s = ptr::null_mut();
        s = lumi_set_insert(s, 3);
        s = lumi_set_insert(s, 4);
        let el = lumi_elems(s);
        assert_eq!(lumi_list_len(el), 2);
        unsafe {
            assert_eq!((*header_from_payload(el)).type_id, crate::TYPE_LIST);
        }
    }

    #[test]
    fn concat_string_vs_list() {
        let a = lumi_alloc_string(b"a".as_ptr(), 1);
        let b = lumi_alloc_string(b"b".as_ptr(), 1);
        let ab = lumi_concat(a, b);
        assert_eq!(lumi_len(ab), 2);
        let xs = lumi_list_append(lumi_list_empty(), 1);
        let ys = lumi_list_append(lumi_list_empty(), 2);
        let zs = lumi_concat(xs, ys);
        assert_eq!(lumi_list_len(zs), 2);
    }
}
