//! Ord comparison, content hashing, and ADT tag/field accessors.

use crate::common::{
    float_key_hash, header_from_payload, is_heap_payload, list_elem_is_float, splitmix64, tid_base,
    trap_abort, TYPE_ADT, TYPE_BYTES, TYPE_CHAR, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_IOTA, TYPE_MAP,
    TYPE_SET, TYPE_STRING,
};
use crate::list::{list_get_of, list_len_of};
use crate::map_set::{
    map_count, map_float_keys, map_float_vals, map_pair_at, set_elem_at, set_float_elems,
};

pub(crate) fn lumia_ord_cmp(a: i64, b: i64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a == b {
        return Ordering::Equal;
    }
    let pa = a as *mut u8;
    let pb = b as *mut u8;
    let ha = is_heap_payload(pa);
    let hb = is_heap_payload(pb);
    if !ha && !hb {
        return a.cmp(&b);
    }
    if ha && hb {
        unsafe {
            let ta = (*header_from_payload(pa)).type_id;
            let tb = (*header_from_payload(pb)).type_id;
            if tid_base(ta) != tid_base(tb) {
                trap_abort("lumia: Ord operands have mixed heap types");
            }
            // Packed ADT Show-kind must match (different ADT types share base TYPE_ADT).
            if tid_base(ta) == TYPE_ADT && ta != tb {
                trap_abort("lumia: Ord operands have mixed heap types");
            }
            match tid_base(ta) {
                TYPE_STRING => {
                    let na = (*header_from_payload(pa)).size as usize;
                    let nb = (*header_from_payload(pb)).size as usize;
                    let sa = std::slice::from_raw_parts(pa, na);
                    let sb = std::slice::from_raw_parts(pb, nb);
                    sa.cmp(sb)
                }
                TYPE_CHAR => {
                    let ca = *(pa as *const i64);
                    let cb = *(pb as *const i64);
                    ca.cmp(&cb)
                }
                TYPE_ADT => {
                    // Lexicographic: tag then fields (products use tag 0).
                    let words_a = ((*header_from_payload(pa)).size as usize) / 8;
                    let words_b = ((*header_from_payload(pb)).size as usize) / 8;
                    if words_a != words_b {
                        return words_a.cmp(&words_b);
                    }
                    let ba = pa as *const i64;
                    let bb = pb as *const i64;
                    for i in 0..words_a {
                        match lumia_ord_cmp(*ba.add(i), *bb.add(i)) {
                            Ordering::Equal => continue,
                            other => return other,
                        }
                    }
                    Ordering::Equal
                }
                _ => trap_abort(&format!(
                    "lumia: type_id={ta} is not Ord (use Int/Float/Bool/String/Char or Ord ADT)"
                )),
            }
        }
    } else {
        trap_abort("lumia: cannot compare scalar with heap value under Ord");
    }
}

/// C ABI for `<`/`<=`/`>`/`>=`: returns -1 / 0 / 1.
#[no_mangle]
pub extern "C" fn lumia_cmp(a: i64, b: i64) -> i64 {
    match lumia_ord_cmp(a, b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}
/// Stable content hash for Map/Set keys — must agree with `lumia_eq` (DESIGN §3.5.1).
pub fn lumia_hash(key: i64) -> u64 {
    hash_value(key, 0)
}

pub(crate) fn hash_value(key: i64, depth: u32) -> u64 {
    if depth > 64 {
        return splitmix64(key as u64);
    }
    let p = key as *mut u8;
    if !is_heap_payload(p) {
        return splitmix64(key as u64);
    }
    unsafe {
        let h = header_from_payload(p);
        let tid = (*h).type_id;
        match tid_base(tid) {
            TYPE_STRING => {
                let n = (*h).size as usize;
                let bytes = std::slice::from_raw_parts(p, n);
                let mut acc = 0xcbf29ce484222325u64;
                for &b in bytes {
                    acc ^= b as u64;
                    acc = acc.wrapping_mul(0x100000001b3);
                }
                acc
            }
            TYPE_CHAR => splitmix64(*(p as *const i64) as u64),
            TYPE_LIST | TYPE_LIST_IOTA => {
                let n = list_len_of(p);
                let float_elems = list_elem_is_float(tid);
                let mut acc = splitmix64(0x4c495354u64 ^ (n as u64));
                for i in 0..n {
                    let e = list_get_of(p, i);
                    let he = if float_elems {
                        float_key_hash(e)
                    } else {
                        hash_value(e, depth + 1)
                    };
                    acc = acc.rotate_left(7).wrapping_add(he);
                }
                acc
            }
            TYPE_ADT => {
                let words = ((*h).size as usize) / 8;
                let base = p as *const i64;
                let float_mask = (*h)._pad as u64;
                let mut acc = splitmix64(0x414454u64 ^ (words as u64));
                // Tag (word 0) always hashed as bits; fields honour IEEE layout mask.
                if words > 0 {
                    acc = acc.rotate_left(11).wrapping_add(splitmix64(*base as u64));
                }
                for i in 1..words {
                    let e = *base.add(i);
                    let he = if float_mask & (1u64 << (i - 1)) != 0 {
                        float_key_hash(e)
                    } else {
                        hash_value(e, depth + 1)
                    };
                    acc = acc.rotate_left(11).wrapping_add(he);
                }
                acc
            }
            TYPE_MAP => {
                // Unordered mix so content-equal maps collide regardless of insert order.
                let float_keys = map_float_keys(p);
                let float_vals = map_float_vals(p);
                let n = map_count(p);
                let mut acc = splitmix64(0x4d4150u64 ^ (n as u64));
                for i in 0..n as usize {
                    let (k, v) = map_pair_at(p, i);
                    let hk = if float_keys {
                        float_key_hash(k)
                    } else {
                        hash_value(k, depth + 1)
                    };
                    let hv = if float_vals {
                        float_key_hash(v)
                    } else {
                        hash_value(v, depth + 1)
                    };
                    acc ^= hk.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(hv);
                }
                acc
            }
            TYPE_SET => {
                let float_elems = set_float_elems(p);
                let n = *(p as *const i64);
                let mut acc = splitmix64(0x534554u64 ^ (n as u64));
                for i in 0..n as usize {
                    let e = set_elem_at(p, i);
                    acc ^= if float_elems {
                        float_key_hash(e)
                    } else {
                        hash_value(e, depth + 1)
                    };
                }
                acc
            }
            TYPE_CLOSURE | TYPE_BYTES => splitmix64(key as u64),
            _ => splitmix64(key as u64),
        }
    }
}
#[no_mangle]
pub extern "C" fn lumia_adt_tag(obj: *mut u8) -> i64 {
    if obj.is_null() {
        trap_abort("lumia: adt_tag on null");
    }
    unsafe { *(obj as *const i64) }
}

#[no_mangle]
pub extern "C" fn lumia_adt_field(obj: *mut u8, index: i64) -> i64 {
    if obj.is_null() || index < 0 {
        trap_abort(&format!("lumia: adt_field OOB (null or neg index={index})"));
    }
    unsafe {
        let h = header_from_payload(obj);
        let words = ((*h).size as usize) / 8;
        // Layout: [tag][field0]… → field count = words - 1
        if words == 0 || (index as usize) + 1 >= words {
            let nfields = words.saturating_sub(1);
            let tag = *(obj as *const i64);
            let tid = (*h).type_id;
            let bits = obj as usize;
            trap_abort(&format!(
                "lumia: adt_field OOB index={index} nfields={nfields} tag={tag} bytes={} tid={tid} ptr=0x{bits:x}",
                (*h).size
            ));
        }
        let base = obj as *const i64;
        *base.add(1 + index as usize)
    }
}

/// Shallow-clone ADT payload to a fresh heap object (`rc=1`) and retain nested
/// List/ADT fields (shared with `src`).
///
/// `overwrite_mask` bit `i` ⇒ field `i` will be replaced immediately (e.g. inplace
/// `with`): leave the slot null and skip nested retain so brother float buffers
/// are not falsely shared. [`lumia_adt_set_field`] then installs the new value
/// (null old ⇒ no release).
unsafe fn adt_shallow_clone_heap(src: *mut u8, overwrite_mask: u64) -> *mut u8 {
    use crate::common::{adt_retain_nested_fields, tid_base, GcInhibitGuard, TYPE_ADT};
    use crate::gc::lumia_alloc;
    let h = header_from_payload(src);
    if tid_base((*h).type_id) != TYPE_ADT {
        return src;
    }
    let _gc = GcInhibitGuard::enter();
    let nbytes = (*h).size as u64;
    let dest = lumia_alloc(nbytes, (*h).type_id);
    let nwords = ((*h).size as usize) / 8;
    std::ptr::copy_nonoverlapping(src as *const i64, dest as *mut i64, nwords);
    (*header_from_payload(dest))._pad = (*h)._pad;
    if overwrite_mask != 0 {
        let nfields = nwords.saturating_sub(1);
        let base = dest as *mut i64;
        for i in 0..nfields {
            if overwrite_mask & (1u64 << i) != 0 {
                *base.add(1 + i) = 0;
            }
        }
    }
    adt_retain_nested_fields(dest);
    dest
}

/// COW: shared heap ADT → shallow clone; unique heap ADT unchanged.
/// Stack LitAdt has no RC — always promote to a heap clone before in-place `with`.
#[no_mangle]
pub extern "C" fn lumia_adt_ensure_unique(obj: *mut u8) -> *mut u8 {
    lumia_adt_ensure_unique_mask(obj, 0)
}

/// Like [`lumia_adt_ensure_unique`], but `overwrite_mask` skips nested retain on
/// fields that inplace `with` will rewrite (avoids RC≥2 on untouched siblings
/// when the product itself must clone).
#[no_mangle]
pub extern "C" fn lumia_adt_ensure_unique_mask(obj: *mut u8, overwrite_mask: u64) -> *mut u8 {
    use crate::common::{cow_rc_is_unique, tid_base, TYPE_ADT};
    if obj.is_null() {
        return obj;
    }
    unsafe {
        let h = header_from_payload(obj);
        if tid_base((*h).type_id) != TYPE_ADT {
            return obj;
        }
        if !is_heap_payload(obj) {
            return adt_shallow_clone_heap(obj, overwrite_mask);
        }
        if cow_rc_is_unique(obj, true) {
            return obj;
        }
        adt_shallow_clone_heap(obj, overwrite_mask)
    }
}

/// Drop one alias retain (e.g. with-temp), then [`lumia_adt_ensure_unique`].
/// Prefer plain `ensure_unique` when the with-temp Let was optimized away.
#[no_mangle]
pub extern "C" fn lumia_adt_ensure_unique_consume(obj: *mut u8) -> *mut u8 {
    lumia_adt_ensure_unique_consume_mask(obj, 0)
}

/// Consume with-temp retain, then unique-check with overwrite mask for `with`.
#[no_mangle]
pub extern "C" fn lumia_adt_ensure_unique_consume_mask(
    obj: *mut u8,
    overwrite_mask: u64,
) -> *mut u8 {
    use crate::common::cow_rc_drop_alias;
    cow_rc_drop_alias(obj, /*adt_ok=*/ true);
    lumia_adt_ensure_unique_mask(obj, overwrite_mask)
}

/// Write ADT field `index` (0-based): release old List/ADT, retain new, barrier.
#[no_mangle]
pub extern "C" fn lumia_adt_set_field(obj: *mut u8, index: i64, value: i64) {
    use crate::common::{value_rc_release_bits, value_rc_retain_bits};
    if obj.is_null() || index < 0 {
        trap_abort(&format!("lumia: adt_set_field OOB (null or neg index={index})"));
    }
    unsafe {
        let h = header_from_payload(obj);
        let words = ((*h).size as usize) / 8;
        if words == 0 || (index as usize) + 1 >= words {
            trap_abort(&format!(
                "lumia: adt_set_field OOB index={index} words={words}"
            ));
        }
        let slot = (obj as *mut i64).add(1 + index as usize);
        let float_field = ((*h)._pad & (1u64 << (index as u32))) != 0;
        if !float_field {
            let old = *slot;
            if old != value {
                value_rc_release_bits(old);
                value_rc_retain_bits(value);
            }
        }
        *slot = value;
        // Float-masked slots are unboxed bits, not GC pointers.
        if !float_field {
            crate::gc::lumia_write_barrier(obj, (index + 1) as u32, value as *mut u8);
        }
    }
}
