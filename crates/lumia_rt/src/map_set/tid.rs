//! Type-id classifiers and Float-key/value ensure helpers.

use crate::common::{
    float_key_eq, float_key_hash, header_from_payload, trap_abort, TYPE_MAP, TYPE_SET,
};
use crate::eq::lumia_eq;
use crate::gc::lumia_alloc;
use crate::hash_ord::lumia_hash;
use lumia_abi::{
    is_map_tid, is_set_tid, map_type_id as abi_map_type_id, set_type_id as abi_set_type_id,
    tid_f_key, tid_f_val, tid_with_f_key, tid_with_f_val,
};

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
        let dest = lumia_alloc(8, abi_map_type_id(true, false, false));
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(map);
        let tid = (*h).type_id;
        if !is_map_tid(tid) {
            trap_abort(&format!("lumia: ensure_map_f64 on type_id={tid}"));
        }
        if tid_f_key(tid) {
            return map;
        }
        if map_count(map) != 0 {
            trap_abort("lumia: ensure_map_f64 on non-empty Int-key map");
        }
        let dest = lumia_alloc(8, tid_with_f_key(tid));
        *(dest as *mut i64) = 0;
        dest
    }
}

/// Ensure a map uses IEEE equality for Float values.
pub(crate) fn ensure_map_vf64(map: *mut u8) -> *mut u8 {
    if map.is_null() {
        let dest = lumia_alloc(8, abi_map_type_id(false, true, false));
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(map);
        let tid = (*h).type_id;
        if !is_map_tid(tid) {
            trap_abort(&format!("lumia: ensure_map_vf64 on type_id={tid}"));
        }
        if tid_f_val(tid) {
            return map;
        }
        if map_count(map) != 0 {
            trap_abort("lumia: ensure_map_vf64 on non-empty non-Float-value map");
        }
        let dest = lumia_alloc(8, tid_with_f_val(tid));
        *(dest as *mut i64) = 0;
        dest
    }
}

pub(crate) fn ensure_set_f64(set: *mut u8) -> *mut u8 {
    if set.is_null() {
        let dest = lumia_alloc(8, abi_set_type_id(true, false));
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(set);
        let tid = (*h).type_id;
        if !is_set_tid(tid) {
            trap_abort(&format!("lumia: ensure_set_f64 on type_id={tid}"));
        }
        if tid_f_key(tid) {
            return set;
        }
        if *(set as *const i64) != 0 {
            trap_abort("lumia: ensure_set_f64 on non-empty Int-elem set");
        }
        let dest = lumia_alloc(8, tid_with_f_key(tid));
        *(dest as *mut i64) = 0;
        dest
    }
}

/// Read packed `type_id` from a map object pointer (null → [`TYPE_MAP`]).
#[inline]
pub(crate) fn map_tid(map: *mut u8) -> u32 {
    if map.is_null() {
        TYPE_MAP
    } else {
        unsafe { (*header_from_payload(map)).type_id }
    }
}

/// Read packed `type_id` from a set object pointer (null → [`TYPE_SET`]).
#[inline]
pub(crate) fn set_tid(set: *mut u8) -> u32 {
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
