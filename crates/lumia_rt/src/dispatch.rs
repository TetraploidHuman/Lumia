//! Polymorphic C ABI dispatch across List / Map / Set / String.
//!
//! # Safety (FFI)
//! Non-null `obj` / `a` / `b` must be valid RT heap payloads (or the documented
//! empty-null conventions per entry). Callers from LLVM treat these as C ABI.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use crate::common::{
    header_from_payload, tid_base, trap_abort, GcInhibitGuard, TYPE_LIST, TYPE_LIST_IOTA,
    TYPE_STRING,
};
use crate::gc::{list_payload_bytes, lumia_alloc};
use crate::list::{force_heap_list, list_len_of};
use crate::map_set::{map_count, set_elem_at};
use lumia_abi::{is_list_tid, is_map_tid, is_set_tid};

/// # Safety
/// `obj` is null or a valid RT heap payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_len(obj: *mut u8) -> i64 {
    if obj.is_null() {
        return 0;
    }
    // SAFETY: non-null payload per contract.
    let h = header_from_payload(obj);
    match unsafe { (*h).type_id } {
        TYPE_STRING => unsafe { crate::string_io::lumia_str_len(obj) },
        tid if is_list_tid(tid) => list_len_of(obj),
        tid if is_set_tid(tid) => unsafe { *(obj as *const i64) },
        tid if is_map_tid(tid) => map_count(obj),
        other => trap_abort(&format!("lumia: len on unsupported type {other}")),
    }
}

/// # Safety
/// Non-null args are valid RT heap payloads; null follows empty List/String conventions.
#[no_mangle]
pub unsafe extern "C" fn lumia_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    let ta = if a.is_null() {
        TYPE_LIST
    } else {
        // SAFETY: non-null payload.
        unsafe { (*header_from_payload(a)).type_id }
    };
    let tb = if b.is_null() {
        ta
    } else {
        unsafe { (*header_from_payload(b)).type_id }
    };
    if ta == TYPE_STRING || tb == TYPE_STRING {
        if ta != TYPE_STRING || tb != TYPE_STRING {
            trap_abort("lumia: concat type mismatch");
        }
        return unsafe { crate::string_io::lumia_str_concat(a, b) };
    }
    unsafe { crate::list::lumia_list_concat(a, b) }
}

/// # Safety
/// `obj` is null (empty Map) or a valid List/Map heap payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_set(obj: *mut u8, key_or_index: i64, val: i64) -> *mut u8 {
    if obj.is_null() {
        return unsafe { crate::map_set::lumia_map_set(obj, key_or_index, val) };
    }
    // SAFETY: non-null payload.
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        tid if is_list_tid(tid) => unsafe { crate::list::lumia_list_set(obj, key_or_index, val) },
        tid if is_map_tid(tid) => unsafe { crate::map_set::lumia_map_set(obj, key_or_index, val) },
        _ => trap_abort(&format!("lumia: set on unsupported type_id={tid}")),
    }
}

/// # Safety
/// `obj` is null (empty List) or a valid List/Set/Map/Iota heap payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_elems(obj: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if obj.is_null() {
        let dest = lumia_alloc(8, TYPE_LIST);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    // SAFETY: non-null payload.
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        tid if tid_base(tid) == TYPE_LIST => obj,
        TYPE_LIST_IOTA => force_heap_list(obj),
        tid if is_set_tid(tid) => unsafe {
            let n = *(obj as *const i64);
            let nbytes = list_payload_bytes(n);
            let dest_tid = lumia_abi::list_type_id(lumia_abi::set_elem_is_float(tid));
            let dest = lumia_alloc(nbytes, dest_tid);
            let dst = dest as *mut i64;
            *dst = n;
            for i in 0..n as usize {
                *dst.add(1 + i) = set_elem_at(obj, i);
            }
            dest
        },
        tid if is_map_tid(tid) => unsafe { crate::map_set::lumia_map_keys(obj) },
        other => trap_abort(&format!("lumia: elems unsupported type_id={other}")),
    }
}

/// # Safety
/// `obj` is null (empty Map) or a valid Map/Set heap payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_remove(obj: *mut u8, key_or_elem: i64) -> *mut u8 {
    if obj.is_null() {
        // Ambiguous empty — prefer Map (same historical default as typed `remove`).
        return unsafe { crate::map_set::lumia_map_remove(obj, key_or_elem) };
    }
    // SAFETY: non-null payload.
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        tid if is_map_tid(tid) => unsafe { crate::map_set::lumia_map_remove(obj, key_or_elem) },
        tid if is_set_tid(tid) => unsafe { crate::map_set::lumia_set_remove(obj, key_or_elem) },
        _ => trap_abort(&format!("lumia: remove on unsupported type_id={tid}")),
    }
}

/// Dispatch get: List/Set by index → i64 elem; Map by key → Option ADT ptr as i64.
///
/// # Safety
/// `obj` must be a non-null valid List/Set/Map heap payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_get(obj: *mut u8, key_or_index: i64, some_tag: i64, none_tag: i64) -> i64 {
    if obj.is_null() {
        trap_abort("lumia: get on null");
    }
    // SAFETY: non-null payload; header_from_payload is a typed offset helper.
    let h = header_from_payload(obj);
    match unsafe { (*h).type_id } {
        tid if is_list_tid(tid) => unsafe { crate::list::lumia_list_get(obj, key_or_index) },
        tid if is_set_tid(tid) => {
            let n = unsafe { *(obj as *const i64) };
            if key_or_index < 0 || key_or_index >= n {
                trap_abort("lumia: set get OOB");
            }
            set_elem_at(obj, key_or_index as usize)
        }
        tid if is_map_tid(tid) => {
            let opt =
                unsafe { crate::map_set::lumia_map_get(obj, key_or_index, some_tag, none_tag) };
            opt as i64
        }
        other => trap_abort(&format!("lumia: get unsupported type_id {other}")),
    }
}

/// # Safety
/// `obj` is null (false) or a valid Map/Set/String heap payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_contains(obj: *mut u8, key: i64) -> i64 {
    if obj.is_null() {
        return 0;
    }
    let h = header_from_payload(obj);
    match unsafe { (*h).type_id } {
        tid if is_map_tid(tid) => unsafe { crate::map_set::lumia_map_contains(obj, key) },
        tid if is_set_tid(tid) => unsafe { crate::map_set::lumia_set_contains(obj, key) },
        TYPE_STRING => unsafe { crate::string_io::lumia_str_contains(obj, key as *mut u8) },
        other => trap_abort(&format!("lumia: contains unsupported type_id {other}")),
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
