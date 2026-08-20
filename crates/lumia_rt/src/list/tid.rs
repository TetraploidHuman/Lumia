//! List type_id helpers and Float-elem ensure.
//!
//! # Safety (FFI)
//! `list` is null or a valid List/Iota payload.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use crate::common::{header_from_payload, list_elem_is_float, tid_base, TYPE_LIST, TYPE_LIST_IOTA};
use crate::ensure::ensure_empty_float_retag;
use lumia_abi::{
    list_elem_is_bool, list_elem_is_int, list_type_id, list_type_id_flags, list_type_id_int,
};

#[inline]
pub(crate) fn list_tid(list: *mut u8) -> u32 {
    if list.is_null() {
        TYPE_LIST
    } else {
        unsafe { (*header_from_payload(list)).type_id }
    }
}

/// Preserve Float/Bool/Int-elem tagging when allocating a derived HeapList.
#[inline]
pub(crate) fn heap_list_tid(list: *mut u8) -> u32 {
    let tid = list_tid(list);
    if list_elem_is_float(tid) || list_elem_is_bool(tid) {
        list_type_id_flags(list_elem_is_float(tid), list_elem_is_bool(tid))
    } else if list_elem_is_int(tid) {
        list_type_id_int()
    } else {
        TYPE_LIST
    }
}

pub(crate) fn list_float_elems(list: *mut u8) -> bool {
    list_elem_is_float(list_tid(list))
}

pub(crate) fn list_bool_elems(list: *mut u8) -> bool {
    list_elem_is_bool(list_tid(list))
}

/// Ensure a list uses IEEE elem eq/hash (`list_type_id(true)`).
/// Empty ordinary lists become a fresh empty F64 list (no in-place retag).
pub(crate) fn ensure_list_f64(list: *mut u8) -> *mut u8 {
    ensure_empty_float_retag(list, list_type_id(true), list_elem_is_float, |tid, list| {
        if tid_base(tid) == TYPE_LIST {
            if unsafe { *(list as *const i64) } != 0 {
                return Err("lumia: ensure_list_f64 on non-empty Int-elem list".into());
            }
            return Ok(list_type_id(true));
        }
        if tid_base(tid) == TYPE_LIST_IOTA {
            return Err("lumia: ensure_list_f64 on Iota".into());
        }
        Err(format!("lumia: ensure_list_f64 on type_id={tid}"))
    })
}

/// Ensure a list uses Bool-elem Show tagging (`TYPE_LIST_BOOL`).
pub(crate) fn ensure_list_bool(list: *mut u8) -> *mut u8 {
    ensure_empty_float_retag(
        list,
        list_type_id_flags(false, true),
        list_elem_is_bool,
        |tid, list| {
            if tid_base(tid) == TYPE_LIST {
                if unsafe { *(list as *const i64) } != 0 {
                    return Err("lumia: ensure_list_bool on non-empty non-Bool list".into());
                }
                return Ok(list_type_id_flags(false, true));
            }
            if tid_base(tid) == TYPE_LIST_IOTA {
                return Err("lumia: ensure_list_bool on Iota".into());
            }
            Err(format!("lumia: ensure_list_bool on type_id={tid}"))
        },
    )
}

/// Ensure a list uses IEEE elem eq/hash (`list_type_id(true)`).
/// Empty ordinary lists become a fresh empty F64 list (no in-place retag).
///
/// # Safety
/// `list` is null or a valid List/Iota payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_ensure_list_f64(list: *mut u8) -> *mut u8 {
    ensure_list_f64(list)
}

/// Ensure a list uses Bool-elem Show tagging.
///
/// # Safety
/// `list` is null or a valid List/Iota payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_ensure_list_bool(list: *mut u8) -> *mut u8 {
    ensure_list_bool(list)
}
