//! Structural equality for scalars and heap objects.

use crate::common::{
    float_key_eq, header_from_payload, is_heap_payload, list_elem_is_float, tid_base, tid_f_key,
    tid_f_val, TYPE_ADT, TYPE_CHAR, TYPE_MAP, TYPE_SET, TYPE_STRING,
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
        let p = a as *mut u8;
        if is_heap_payload(p) {
            let tid = unsafe { (*header_from_payload(p)).type_id };
            // Float-tagged containers: NaN ≠ NaN — must content-compare.
            if !(tid_f_key(tid) || tid_f_val(tid)) {
                return 1;
            }
            // Fall through to content compare (same object still ok for ±0).
        } else {
            return 1;
        }
    }
    let pa = a as *mut u8;
    let pb = b as *mut u8;
    if !is_heap_payload(pa) || !is_heap_payload(pb) {
        return 0;
    }
    unsafe {
        let ha = header_from_payload(pa);
        let hb = header_from_payload(pb);
        let ta = (*ha).type_id;
        let tb = (*hb).type_id;
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
                let na = (*ha).size as usize;
                let nb = (*hb).size as usize;
                if na != nb {
                    return 0;
                }
                let sa = std::slice::from_raw_parts(pa, na);
                let sb = std::slice::from_raw_parts(pb, nb);
                if sa == sb {
                    1
                } else {
                    0
                }
            }
            TYPE_CHAR => {
                let ca = *(pa as *const i64);
                let cb = *(pb as *const i64);
                if ca == cb {
                    1
                } else {
                    0
                }
            }
            TYPE_SET => set_eq(pa, pb),
            TYPE_MAP => map_eq(pa, pb),
            TYPE_ADT => {
                let mask = (*ha)._pad | (*hb)._pad;
                adt_eq_payload(pa, pb, mask)
            }
            _ => 0,
        }
    }
}

/// Structural ADT `==` using the object's real payload size (not type-param arity).
/// `float_mask` bit `i` ⇒ field `i` compared with IEEE (`±0` equal; NaN ≠ NaN).
#[no_mangle]
pub extern "C" fn lumia_adt_eq(a: i64, b: i64, float_mask: i64) -> i64 {
    let pa = a as *mut u8;
    let pb = b as *mut u8;
    if !is_heap_payload(pa) || !is_heap_payload(pb) {
        return if a == b { 1 } else { 0 };
    }
    unsafe {
        let ha = header_from_payload(pa);
        let hb = header_from_payload(pb);
        if tid_base((*ha).type_id) != TYPE_ADT || tid_base((*hb).type_id) != TYPE_ADT {
            return lumia_eq(a, b);
        }
        // Prefer call-site mask; also honour layout stored in header `_pad` (nested eq).
        let mask = (float_mask as u64) | (*ha)._pad | (*hb)._pad;
        adt_eq_payload(pa, pb, mask)
    }
}

pub(crate) fn adt_eq_payload(pa: *mut u8, pb: *mut u8, float_mask: u64) -> i64 {
    unsafe {
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
}

#[cfg(test)]
mod tests {
    use super::lumia_eq;

    #[test]
    fn scalar_non_heap_eq_is_bit_identity() {
        assert_eq!(lumia_eq(0, 0), 1);
        assert_eq!(lumia_eq(1, 2), 0);
        let pos0 = 0.0f64.to_bits() as i64;
        let neg0 = (-0.0f64).to_bits() as i64;
        assert_ne!(pos0, neg0);
        assert_eq!(lumia_eq(pos0, neg0), 0);
    }
}
