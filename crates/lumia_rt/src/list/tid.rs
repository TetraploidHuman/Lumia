//! List type_id helpers and Float-elem ensure.

use crate::common::{
    header_from_payload, list_elem_is_float, tid_base, trap_abort, TYPE_LIST, TYPE_LIST_F64,
    TYPE_LIST_IOTA,
};
use crate::gc::lumia_alloc;

#[inline]
pub(crate) fn is_list_tid(tid: u32) -> bool {
    lumia_abi::is_list_tid(tid)
}

#[inline]
pub(crate) fn list_tid(list: *mut u8) -> u32 {
    if list.is_null() {
        TYPE_LIST
    } else {
        unsafe { (*header_from_payload(list)).type_id }
    }
}

/// Preserve Float-elem tagging when allocating a derived HeapList.
#[inline]
pub(crate) fn heap_list_tid(list: *mut u8) -> u32 {
    if list_elem_is_float(list_tid(list)) {
        TYPE_LIST_F64
    } else {
        TYPE_LIST
    }
}

pub(crate) fn list_float_elems(list: *mut u8) -> bool {
    list_elem_is_float(list_tid(list))
}

/// Ensure a list uses IEEE elem eq/hash (`TYPE_LIST_F64`).
/// Empty ordinary lists become a fresh empty F64 list (no in-place retag).
pub(crate) fn ensure_list_f64(list: *mut u8) -> *mut u8 {
    if list.is_null() {
        let dest = lumia_alloc(8, TYPE_LIST_F64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(list);
        let tid = (*h).type_id;
        if list_elem_is_float(tid) {
            return list;
        }
        if tid_base(tid) == TYPE_LIST {
            if *(list as *const i64) != 0 {
                trap_abort("lumia: ensure_list_f64 on non-empty Int-elem list");
            }
            let dest = lumia_alloc(8, TYPE_LIST_F64);
            *(dest as *mut i64) = 0;
            return dest;
        }
        if tid_base(tid) == TYPE_LIST_IOTA {
            trap_abort("lumia: ensure_list_f64 on Iota");
        }
        trap_abort(&format!("lumia: ensure_list_f64 on type_id={tid}"))
    }
}

#[no_mangle]
pub extern "C" fn lumia_ensure_list_f64(list: *mut u8) -> *mut u8 {
    ensure_list_f64(list)
}
