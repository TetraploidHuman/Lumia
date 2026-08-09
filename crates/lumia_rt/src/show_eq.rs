//! Equality, Show, Ord, and content hashing.

use crate::common::{
    float_key_eq, float_key_hash, header_from_payload, is_heap_payload, splitmix64, trap_abort,
    GcInhibitGuard, TYPE_ADT, TYPE_BYTES, TYPE_CHAR, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_F64,
    TYPE_LIST_IOTA, TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V, TYPE_MAP_ASSOC_VF64, TYPE_MAP_F64,
    TYPE_MAP_F64V, TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC, TYPE_SET_F64, TYPE_STRING,
};
use crate::gc::lumia_alloc;
use crate::list::{is_list_tid, list_float_elems, list_get_of, list_len_of};
use crate::map_set::{
    is_map_tid, is_set_tid, map_count, map_eq, map_float_keys, map_float_vals, map_pair_at,
    set_elem_at, set_eq, set_float_elems,
};
use crate::string_io::{lumia_alloc_string, with_str_bytes};

/// Structural equality for scalars and heap objects (DESIGN: recursive `==`).
#[no_mangle]
pub extern "C" fn lumia_eq(a: i64, b: i64) -> i64 {
    // Same pointer/bits is usually equal, but Float-tagged containers hold
    // IEEE elems/keys: NaN ≠ NaN, so reflexivity fails and we must compare.
    if a == b {
        let p = a as *mut u8;
        if is_heap_payload(p) {
            let tid = unsafe { (*header_from_payload(p)).type_id };
            if !matches!(
                tid,
                TYPE_LIST_F64
                    | TYPE_SET_F64
                    | TYPE_MAP_F64
                    | TYPE_MAP_VF64
                    | TYPE_MAP_F64V
                    | TYPE_MAP_ASSOC_VF64
                    | TYPE_MAP_ASSOC_F64
                    | TYPE_MAP_ASSOC_F64V
            ) {
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
            let float_elems = ta == TYPE_LIST_F64 || tb == TYPE_LIST_F64;
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
        if ta != tb {
            return 0;
        }
        match ta {
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
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => set_eq(pa, pb),
            tid if is_map_tid(tid) => map_eq(pa, pb),
            TYPE_ADT => {
                let mask = ((*ha)._pad as u64) | ((*hb)._pad as u64);
                adt_eq_payload(pa, pb, mask)
            }
            _ => 0,
        }
    }
}
#[no_mangle]
pub extern "C" fn lumia_alloc_char(codepoint: i64) -> *mut u8 {
    let dest = lumia_alloc(8, TYPE_CHAR);
    if dest.is_null() {
        trap_abort("lumia: char OOM");
    }
    unsafe {
        *(dest as *mut i64) = codepoint;
    }
    dest
}

/// Format a value as a heap String (for interpolation).
/// Strings are returned as-is; Chars become one-character strings;
/// List/Map/Set show element contents; ADTs are `#tag(field, …)`;
/// otherwise decimal Int.
#[no_mangle]
pub extern "C" fn lumia_show(x: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let p = x as *mut u8;
    if is_heap_payload(p) {
        unsafe {
            let h = header_from_payload(p);
            let tid = (*h).type_id;
            if tid == TYPE_STRING {
                return p;
            }
            if tid == TYPE_CHAR {
                let cp = *(p as *const i64) as u32;
                let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                return lumia_alloc_string(s.as_ptr(), s.len() as u64);
            }
            if tid == TYPE_ADT {
                return show_adt(p);
            }
            if is_list_tid(tid) {
                return show_list(p);
            }
            if is_map_tid(tid) {
                return show_map(p);
            }
            if is_set_tid(tid) {
                return show_set(p);
            }
        }
    }
    let s = x.to_string();
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

pub(crate) unsafe fn show_adt(payload: *mut u8) -> *mut u8 {
    show_adt_masked(payload, 0)
}

pub(crate) fn show_value_bits(bits: i64, as_float: bool) -> String {
    if as_float {
        let s = lumia_show_float(f64::from_bits(bits as u64));
        return with_str_bytes(s, |b| String::from_utf8_lossy(b).into_owned());
    }
    let s = lumia_show(bits);
    with_str_bytes(s, |b| String::from_utf8_lossy(b).into_owned())
}

pub(crate) unsafe fn show_list(list: *mut u8) -> *mut u8 {
    let n = list_len_of(list);
    let float_elems = list_float_elems(list);
    let mut s = String::from("[");
    for i in 0..n {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&show_value_bits(list_get_of(list, i), float_elems));
    }
    s.push(']');
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

pub(crate) unsafe fn show_map(map: *mut u8) -> *mut u8 {
    let n = map_count(map);
    let float_keys = map_float_keys(map);
    let float_vals = map_float_vals(map);
    let mut s = String::from("{");
    for i in 0..n as usize {
        if i > 0 {
            s.push_str(", ");
        }
        let (k, v) = map_pair_at(map, i);
        s.push_str(&show_value_bits(k, float_keys));
        s.push_str(": ");
        s.push_str(&show_value_bits(v, float_vals));
    }
    s.push('}');
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}
pub(crate) unsafe fn show_set(set: *mut u8) -> *mut u8 {
    let n = if set.is_null() {
        0i64
    } else {
        *(set as *const i64)
    };
    let float_elems = set_float_elems(set);
    let mut s = String::from("#{");
    for i in 0..n as usize {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&show_value_bits(set_elem_at(set, i), float_elems));
    }
    s.push('}');
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

pub(crate) unsafe fn show_adt_masked(payload: *mut u8, float_mask: u64) -> *mut u8 {
    let words = ((*header_from_payload(payload)).size as usize) / 8;
    let base = payload as *const i64;
    let mut s = String::from("#");
    if words == 0 {
        s.push_str("()");
        return lumia_alloc_string(s.as_ptr(), s.len() as u64);
    }
    let tag = *base;
    s.push_str(&tag.to_string());
    s.push('(');
    for i in 1..words {
        if i > 1 {
            s.push_str(", ");
        }
        let bits = *base.add(i);
        let field = if float_mask & (1u64 << (i - 1)) != 0 {
            lumia_show_float(f64::from_bits(bits as u64))
        } else {
            lumia_show(bits)
        };
        with_str_bytes(field, |b| {
            s.push_str(std::str::from_utf8(b).unwrap_or("?"));
        });
    }
    s.push(')');
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
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
        if (*ha).type_id != TYPE_ADT || (*hb).type_id != TYPE_ADT {
            return lumia_eq(a, b);
        }
        // Prefer call-site mask; also honour layout stored in header `_pad` (nested eq).
        let mask = (float_mask as u64) | ((*ha)._pad as u64) | ((*hb)._pad as u64);
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
            let ok = if float_mask & (1u64 << (i - 1)) != 0 {
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

/// Show ADT with IEEE formatting for mask-selected fields.
#[no_mangle]
pub extern "C" fn lumia_show_adt(x: i64, float_mask: i64) -> *mut u8 {
    let p = x as *mut u8;
    if !is_heap_payload(p) {
        return lumia_show(x);
    }
    unsafe {
        let h = header_from_payload(p);
        if (*h).type_id != TYPE_ADT {
            return lumia_show(x);
        }
        let mask = (float_mask as u64) | ((*h)._pad as u64);
        show_adt_masked(p, mask)
    }
}

/// Store per-field Float layout mask in ADT header `_pad` (bit `i` ⇒ field `i` is unboxed Float).
#[no_mangle]
pub extern "C" fn lumia_adt_set_float_mask(obj: *mut u8, float_mask: u32) {
    if obj.is_null() || !is_heap_payload(obj) {
        return;
    }
    unsafe {
        let h = header_from_payload(obj);
        if (*h).type_id == TYPE_ADT {
            (*h)._pad = float_mask;
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_show_float(n: f64) -> *mut u8 {
    let s = n.to_string();
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

#[no_mangle]
pub extern "C" fn lumia_show_bool(b: i8) -> *mut u8 {
    let s = if b != 0 { "true" } else { "false" };
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}
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
            if ta != tb {
                trap_abort("lumia: Ord operands have mixed heap types");
            }
            match ta {
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
        match (*h).type_id {
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
            TYPE_LIST | TYPE_LIST_F64 | TYPE_LIST_IOTA => {
                let n = list_len_of(p);
                let float_elems = (*h).type_id == TYPE_LIST_F64;
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
            tid if is_map_tid(tid) => {
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
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => {
                let float_elems = (*h).type_id == TYPE_SET_F64;
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
        trap_abort("lumia: adt_field OOB");
    }
    unsafe {
        let h = header_from_payload(obj);
        let words = ((*h).size as usize) / 8;
        // Layout: [tag][field0]… → field count = words - 1
        if words == 0 || (index as usize) + 1 >= words {
            trap_abort("lumia: adt_field OOB");
        }
        let base = obj as *const i64;
        *base.add(1 + index as usize)
    }
}
