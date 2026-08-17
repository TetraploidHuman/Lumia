//! Structural equality for scalars and heap objects.
//!
//! # Safety (FFI)
//! Heap bits that pass [`is_heap_payload`] must be valid RT payloads for the
//! duration of the call. Immediates / FunRef-tagged bits never enter payload
//! paths.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use crate::common::{
    float_key_eq, header_from_payload, is_heap_payload, is_heap_payload_bits, list_elem_is_float,
    may_be_heap_payload_bits, tid_base, tid_f_key, tid_f_val, TYPE_ADT, TYPE_CHAR, TYPE_MAP,
    TYPE_SET, TYPE_STRING,
};
use crate::list::{list_get_of, list_len_of};
use crate::map_set::{map_eq, set_eq};
use lumia_abi::is_list_tid;

/// Structural equality for scalars and heap objects (DESIGN: recursive `==`).
#[no_mangle]
pub extern "C" fn lumia_eq(a: i64, b: i64) -> i64 {
    // Same pointer/bits is usually equal, but Float-tagged containers hold
    // IEEE elems/keys: NaN ≠ NaN, so reflexivity fails and we must compare.
    if a == b {
        // Immediates / FunRef-tagged bits cannot be heap payloads — skip Mutex.
        if !may_be_heap_payload_bits(a) {
            return 1;
        }
        let p = a as *mut u8;
        if is_heap_payload(p) {
            // SAFETY: `p` is a live heap payload.
            let tid = unsafe { (*header_from_payload(p)).type_id };
            // Float-tagged containers: NaN ≠ NaN — must content-compare.
            if !(tid_f_key(tid) || tid_f_val(tid)) {
                return 1;
            }
            // Fall through to content compare (same object still ok for ±0).
        } else {
            return 1;
        }
    } else if !may_be_heap_payload_bits(a) || !may_be_heap_payload_bits(b) {
        // Unequal and at least one side cannot be a managed object.
        return 0;
    }
    let pa = a as *mut u8;
    let pb = b as *mut u8;
    if !is_heap_payload(pa) || !is_heap_payload(pb) {
        return 0;
    }
    // SAFETY: both sides are live heap payloads.
    let ta = unsafe { (*header_from_payload(pa)).type_id };
    let tb = unsafe { (*header_from_payload(pb)).type_id };
    // HeapList ↔ Iota ↔ ListF64: same user type `List`, compare by content.
    if is_list_tid(ta) && is_list_tid(tb) {
        let na = list_len_of(pa);
        let nb = list_len_of(pb);
        if na != nb {
            return 0;
        }
        // Either side tagged Float elems ⇒ IEEE (covers ±0 / NaN).
        let float_elems = list_elem_is_float(ta) || list_elem_is_float(tb);
        for i in 0..na {
            let ea = list_get_of(pa, i);
            let eb = list_get_of(pb, i);
            let ok = if float_elems {
                float_key_eq(ea, eb)
            } else {
                lumia_eq(ea, eb) != 0
            };
            if !ok {
                return 0;
            }
        }
        return 1;
    }
    // Map/Set: compare by base + content (flags may differ on empty).
    if tid_base(ta) != tid_base(tb) {
        return 0;
    }
    match tid_base(ta) {
        TYPE_STRING => {
            // SAFETY: string payloads; size is UTF-8 byte length.
            let na = unsafe { (*header_from_payload(pa)).size as usize };
            let nb = unsafe { (*header_from_payload(pb)).size as usize };
            if na != nb {
                return 0;
            }
            let sa = unsafe { std::slice::from_raw_parts(pa, na) };
            let sb = unsafe { std::slice::from_raw_parts(pb, nb) };
            if sa == sb {
                1
            } else {
                0
            }
        }
        TYPE_CHAR => {
            // SAFETY: Char payload is one i64 codepoint.
            let ca = unsafe { *(pa as *const i64) };
            let cb = unsafe { *(pb as *const i64) };
            if ca == cb {
                1
            } else {
                0
            }
        }
        TYPE_SET => set_eq(pa, pb),
        TYPE_MAP => map_eq(pa, pb),
        TYPE_ADT => {
            // SAFETY: ADT headers for float-mask bits.
            let mask = unsafe {
                crate::common::adt_float_mask((*header_from_payload(pa))._pad)
                    | crate::common::adt_float_mask((*header_from_payload(pb))._pad)
            };
            // SAFETY: both payloads are ADT with matching word layout checked inside.
            unsafe { adt_eq_payload(pa, pb, mask) }
        }
        _ => 0,
    }
}

/// Structural ADT `==` using the object's real payload size (not type-param arity).
/// `float_mask` bit `i` ⇒ field `i` compared with IEEE (`±0` equal; NaN ≠ NaN).
#[no_mangle]
pub extern "C" fn lumia_adt_eq(a: i64, b: i64, float_mask: i64) -> i64 {
    let pa = a as *mut u8;
    let pb = b as *mut u8;
    if !is_heap_payload_bits(a) || !is_heap_payload_bits(b) {
        return if a == b { 1 } else { 0 };
    }
    // SAFETY: both sides are live heap payloads.
    let ha = header_from_payload(pa);
    let hb = header_from_payload(pb);
    if unsafe { tid_base((*ha).type_id) != TYPE_ADT || tid_base((*hb).type_id) != TYPE_ADT } {
        return lumia_eq(a, b);
    }
    // Prefer call-site mask; for headers require **both** sides to tag a field as
    // Float so eq agrees with per-object `hash_value` (which reads only one `_pad`).
    // Use float half of packed `_pad` (lo32); hi32 is Bool mask.
    let mask = (float_mask as u64)
        | unsafe {
            crate::common::adt_float_mask((*ha)._pad) & crate::common::adt_float_mask((*hb)._pad)
        };
    // SAFETY: ADT payloads verified above.
    unsafe { adt_eq_payload(pa, pb, mask) }
}

/// Compare two ADT payloads field-by-field.
///
/// # Safety
/// `pa` / `pb` must be valid ADT heap payloads; sizes are read from headers.
pub(crate) unsafe fn adt_eq_payload(pa: *mut u8, pb: *mut u8, float_mask: u64) -> i64 {
    let ha = header_from_payload(pa);
    let hb = header_from_payload(pb);
    let words_a = ((*ha).size as usize) / 8;
    let words_b = ((*hb).size as usize) / 8;
    if words_a != words_b || words_a == 0 {
        return 0;
    }
    let ba = pa as *const i64;
    let bb = pb as *const i64;
    // Word 0 is the tag (never a Float payload).
    if *ba != *bb {
        return 0;
    }
    for i in 1..words_a {
        let fa = *ba.add(i);
        let fb = *bb.add(i);
        let ok = if crate::common::adt_float_slot(float_mask, i - 1) {
            float_key_eq(fa, fb)
        } else {
            lumia_eq(fa, fb) != 0
        };
        if !ok {
            return 0;
        }
    }
    1
}

#[cfg(test)]
#[path = "eq_tests.rs"]
mod tests;
