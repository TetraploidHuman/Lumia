//! Type-id classifiers and Float-key/value ensure helpers.

use crate::common::{
    float_key_eq, float_key_hash, header_from_payload, trap_abort, TYPE_MAP, TYPE_MAP_ASSOC,
    TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V, TYPE_MAP_ASSOC_VF64, TYPE_MAP_F64, TYPE_MAP_F64V,
    TYPE_MAP_VF64, TYPE_SET, TYPE_SET_F64,
};
use crate::gc::lumia_alloc;
use crate::show_eq::{lumia_eq, lumia_hash};

use super::map_core::map_count;

pub(crate) fn map_is_assoc(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    lumia_abi::map_tid_is_assoc(unsafe { (*header_from_payload(map)).type_id })
}

pub(crate) fn set_is_assoc(set: *mut u8) -> bool {
    if set.is_null() {
        return false;
    }
    lumia_abi::set_tid_is_assoc(unsafe { (*header_from_payload(set)).type_id })
}

#[inline]
pub(crate) fn is_map_tid(tid: u32) -> bool {
    lumia_abi::is_map_tid(tid)
}

pub(crate) fn map_float_keys(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    lumia_abi::map_key_is_float(unsafe { (*header_from_payload(map)).type_id })
}

pub(crate) fn map_float_vals(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    lumia_abi::map_val_is_float(unsafe { (*header_from_payload(map)).type_id })
}

pub(crate) fn map_tid_with_flags(float_keys: bool, float_vals: bool) -> u32 {
    lumia_abi::map_type_id(float_keys, float_vals, false)
}

pub(crate) fn map_assoc_tid_with_flags(float_keys: bool, float_vals: bool) -> u32 {
    lumia_abi::map_type_id(float_keys, float_vals, true)
}

pub(crate) fn set_float_elems(set: *mut u8) -> bool {
    if set.is_null() {
        return false;
    }
    lumia_abi::set_elem_is_float(unsafe { (*header_from_payload(set)).type_id })
}

pub(crate) fn key_eq(a: i64, b: i64, float_keys: bool) -> bool {
    if float_keys {
        float_key_eq(a, b)
    } else {
        lumia_eq(a, b) != 0
    }
}

pub(crate) fn key_hash(key: i64, float_keys: bool) -> u64 {
    if float_keys {
        float_key_hash(key)
    } else {
        lumia_hash(key)
    }
}

/// Ensure a map uses Float-key IEEE eq/hash.
/// Empty maps may be retagged (fresh alloc); non-empty wrong key sort traps.
pub(crate) fn ensure_map_f64(map: *mut u8) -> *mut u8 {
    if map.is_null() {
        let dest = lumia_alloc(8, TYPE_MAP_F64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(map);
        match (*h).type_id {
            TYPE_MAP_F64 | TYPE_MAP_F64V | TYPE_MAP_ASSOC_F64 | TYPE_MAP_ASSOC_F64V => map,
            TYPE_MAP | TYPE_MAP_VF64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_f64 on non-empty Int-key map");
                }
                let tid = map_tid_with_flags(true, (*h).type_id == TYPE_MAP_VF64);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            TYPE_MAP_ASSOC | TYPE_MAP_ASSOC_VF64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_f64 on non-empty Int-key assoc map");
                }
                let tid = map_assoc_tid_with_flags(true, (*h).type_id == TYPE_MAP_ASSOC_VF64);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            other => trap_abort(&format!("lumia: ensure_map_f64 on type_id={other}")),
        }
    }
}

/// Ensure a map uses IEEE equality for Float values.
pub(crate) fn ensure_map_vf64(map: *mut u8) -> *mut u8 {
    if map.is_null() {
        let dest = lumia_alloc(8, TYPE_MAP_VF64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(map);
        match (*h).type_id {
            TYPE_MAP_VF64 | TYPE_MAP_F64V | TYPE_MAP_ASSOC_VF64 | TYPE_MAP_ASSOC_F64V => map,
            TYPE_MAP | TYPE_MAP_F64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_vf64 on non-empty non-Float-value map");
                }
                let tid = map_tid_with_flags((*h).type_id == TYPE_MAP_F64, true);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            TYPE_MAP_ASSOC | TYPE_MAP_ASSOC_F64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_vf64 on non-empty non-Float-value assoc map");
                }
                let tid = map_assoc_tid_with_flags((*h).type_id == TYPE_MAP_ASSOC_F64, true);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            other => trap_abort(&format!("lumia: ensure_map_vf64 on type_id={other}")),
        }
    }
}

pub(crate) fn ensure_set_f64(set: *mut u8) -> *mut u8 {
    if set.is_null() {
        let dest = lumia_alloc(8, TYPE_SET_F64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(set);
        match (*h).type_id {
            TYPE_SET_F64 => set,
            TYPE_SET => {
                if *(set as *const i64) != 0 {
                    trap_abort("lumia: ensure_set_f64 on non-empty Int-elem set");
                }
                let dest = lumia_alloc(8, TYPE_SET_F64);
                *(dest as *mut i64) = 0;
                dest
            }
            other => trap_abort(&format!("lumia: ensure_set_f64 on type_id={other}")),
        }
    }
}

pub(crate) fn map_type_id(map: *mut u8) -> u32 {
    if map.is_null() {
        TYPE_MAP
    } else {
        unsafe { (*header_from_payload(map)).type_id }
    }
}

pub(crate) fn set_type_id(set: *mut u8) -> u32 {
    if set.is_null() {
        TYPE_SET
    } else {
        unsafe { (*header_from_payload(set)).type_id }
    }
}

#[no_mangle]
pub extern "C" fn lumia_ensure_map_f64(map: *mut u8) -> *mut u8 {
    ensure_map_f64(map)
}

#[no_mangle]
pub extern "C" fn lumia_ensure_map_vf64(map: *mut u8) -> *mut u8 {
    ensure_map_vf64(map)
}

#[no_mangle]
pub extern "C" fn lumia_ensure_set_f64(set: *mut u8) -> *mut u8 {
    ensure_set_f64(set)
}
pub(crate) fn is_set_tid(tid: u32) -> bool {
    lumia_abi::is_set_tid(tid)
}
