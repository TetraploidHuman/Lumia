//! Type-id classifiers and Float-key/value ensure helpers.
//!
//! # Safety (FFI)
//! Ensure helpers retag empty Map/Set float layouts.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use crate::common::{float_key_eq, float_key_hash, header_from_payload, TYPE_MAP, TYPE_SET};
use crate::ensure::ensure_empty_float_retag;
use crate::eq::lumia_eq;
use crate::hash_ord::lumia_hash;
use lumia_abi::{
    is_map_tid, is_set_tid, map_type_id as abi_map_type_id, map_type_id_flags,
    set_type_id as abi_set_type_id, set_type_id_flags, tid_b_key, tid_b_val, tid_f_key, tid_f_val,
    tid_with_b_key, tid_with_b_val, tid_with_f_key, tid_with_f_val,
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

pub(crate) fn map_bool_keys(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    lumia_abi::map_key_is_bool(unsafe { (*header_from_payload(map)).type_id })
}

pub(crate) fn map_bool_vals(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    lumia_abi::map_val_is_bool(unsafe { (*header_from_payload(map)).type_id })
}

pub(crate) fn set_bool_elems(set: *mut u8) -> bool {
    if set.is_null() {
        return false;
    }
    lumia_abi::set_elem_is_bool(unsafe { (*header_from_payload(set)).type_id })
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
    ensure_empty_float_retag(
        map,
        abi_map_type_id(true, false, false),
        tid_f_key,
        |tid, map| {
            if !is_map_tid(tid) {
                return Err(format!("lumia: ensure_map_f64 on type_id={tid}"));
            }
            if map_count(map) != 0 {
                return Err("lumia: ensure_map_f64 on non-empty Int-key map".into());
            }
            Ok(tid_with_f_key(tid))
        },
    )
}

/// Ensure a map uses IEEE equality for Float values.
pub(crate) fn ensure_map_vf64(map: *mut u8) -> *mut u8 {
    ensure_empty_float_retag(
        map,
        abi_map_type_id(false, true, false),
        tid_f_val,
        |tid, map| {
            if !is_map_tid(tid) {
                return Err(format!("lumia: ensure_map_vf64 on type_id={tid}"));
            }
            if map_count(map) != 0 {
                return Err("lumia: ensure_map_vf64 on non-empty non-Float-value map".into());
            }
            Ok(tid_with_f_val(tid))
        },
    )
}

pub(crate) fn ensure_set_f64(set: *mut u8) -> *mut u8 {
    ensure_empty_float_retag(set, abi_set_type_id(true, false), tid_f_key, |tid, set| {
        if !is_set_tid(tid) {
            return Err(format!("lumia: ensure_set_f64 on type_id={tid}"));
        }
        if unsafe { *(set as *const i64) } != 0 {
            return Err("lumia: ensure_set_f64 on non-empty Int-elem set".into());
        }
        Ok(tid_with_f_key(tid))
    })
}

/// Ensure a map uses Bool-key Show tagging.
pub(crate) fn ensure_map_bool(map: *mut u8) -> *mut u8 {
    ensure_empty_float_retag(
        map,
        map_type_id_flags(false, false, true, false, false),
        |tid| tid_b_key(tid) && !tid_f_key(tid),
        |tid, map| {
            if !is_map_tid(tid) {
                return Err(format!("lumia: ensure_map_bool on type_id={tid}"));
            }
            if map_count(map) != 0 {
                return Err("lumia: ensure_map_bool on non-empty non-Bool-key map".into());
            }
            Ok(tid_with_b_key(tid))
        },
    )
}

/// Ensure a map uses Bool-value Show tagging.
pub(crate) fn ensure_map_vbool(map: *mut u8) -> *mut u8 {
    ensure_empty_float_retag(
        map,
        map_type_id_flags(false, false, false, true, false),
        |tid| tid_b_val(tid) && !tid_f_val(tid),
        |tid, map| {
            if !is_map_tid(tid) {
                return Err(format!("lumia: ensure_map_vbool on type_id={tid}"));
            }
            if map_count(map) != 0 {
                return Err("lumia: ensure_map_vbool on non-empty non-Bool-value map".into());
            }
            Ok(tid_with_b_val(tid))
        },
    )
}

pub(crate) fn ensure_set_bool(set: *mut u8) -> *mut u8 {
    ensure_empty_float_retag(
        set,
        set_type_id_flags(false, true, false),
        |tid| tid_b_key(tid) && !tid_f_key(tid),
        |tid, set| {
            if !is_set_tid(tid) {
                return Err(format!("lumia: ensure_set_bool on type_id={tid}"));
            }
            if unsafe { *(set as *const i64) } != 0 {
                return Err("lumia: ensure_set_bool on non-empty non-Bool set".into());
            }
            Ok(tid_with_b_key(tid))
        },
    )
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
pub unsafe extern "C" fn lumia_ensure_map_f64(map: *mut u8) -> *mut u8 {
    ensure_map_f64(map)
}

#[no_mangle]
pub unsafe extern "C" fn lumia_ensure_map_vf64(map: *mut u8) -> *mut u8 {
    ensure_map_vf64(map)
}

#[no_mangle]
pub unsafe extern "C" fn lumia_ensure_set_f64(set: *mut u8) -> *mut u8 {
    ensure_set_f64(set)
}

#[no_mangle]
pub unsafe extern "C" fn lumia_ensure_map_bool(map: *mut u8) -> *mut u8 {
    ensure_map_bool(map)
}

#[no_mangle]
pub unsafe extern "C" fn lumia_ensure_map_vbool(map: *mut u8) -> *mut u8 {
    ensure_map_vbool(map)
}

#[no_mangle]
pub unsafe extern "C" fn lumia_ensure_set_bool(set: *mut u8) -> *mut u8 {
    ensure_set_bool(set)
}
