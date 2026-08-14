//! Show / format values as heap Strings.

use crate::adt_show::adt_show_name_ptrs;
use crate::common::{
    header_from_payload, is_heap_payload, tid_base, trap_abort, GcInhibitGuard, TYPE_ADT,
    TYPE_CHAR, TYPE_STRING,
};
use crate::gc::lumia_alloc;
use crate::list::{list_float_elems, list_get_of, list_len_of};
use crate::map_set::{
    map_count, map_float_keys, map_float_vals, map_pair_at, set_elem_at, set_float_elems,
};
use crate::string_io::{lumia_alloc_string, with_str_bytes};
use lumia_abi::{adt_show_kind, is_list_tid, is_map_tid, is_set_tid};

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
/// List/Map/Set show element contents; ADTs use `#tag(…)` when untyped,
/// or constructor / type names when the print site knows the ADT layout;
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
            if tid_base(tid) == TYPE_ADT {
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
    let h = header_from_payload(payload);
    let tid = (*h).type_id;
    let mask = (*h)._pad;
    let kind = adt_show_kind(tid);
    let ptrs = adt_show_name_ptrs(kind);
    if !ptrs.is_empty() {
        return show_adt_masked_named(payload, mask, ptrs.as_ptr(), ptrs.len() as i64);
    }
    show_adt_masked(payload, mask)
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
    show_adt_masked_named(payload, float_mask, std::ptr::null(), 0)
}

/// Like [`show_adt_masked`], but `names[tag]` (NUL-terminated) replaces `#tag` when present.
pub(crate) unsafe fn show_adt_masked_named(
    payload: *mut u8,
    float_mask: u64,
    names: *const *const u8,
    n_names: i64,
) -> *mut u8 {
    let words = ((*header_from_payload(payload)).size as usize) / 8;
    let base = payload as *const i64;
    if words == 0 {
        return lumia_alloc_string(b"#()".as_ptr(), 3);
    }
    let tag = *base;
    let mut s = String::new();
    let named = if !names.is_null() && tag >= 0 && tag < n_names {
        let p = *names.add(tag as usize);
        if !p.is_null() {
            let mut len = 0usize;
            while *p.add(len) != 0 {
                len += 1;
            }
            Some(std::str::from_utf8(std::slice::from_raw_parts(p, len)).unwrap_or("?"))
        } else {
            None
        }
    } else {
        None
    };
    match named {
        Some(n) if !n.is_empty() => s.push_str(n),
        _ => {
            s.push('#');
            s.push_str(&tag.to_string());
        }
    }
    if words == 1 {
        // Unit variant / empty product payload: just the name (e.g. `A`, `None`).
        return lumia_alloc_string(s.as_ptr(), s.len() as u64);
    }
    s.push('(');
    for i in 1..words {
        if i > 1 {
            s.push_str(", ");
        }
        let bits = *base.add(i);
        let field = if crate::common::adt_float_slot(float_mask, i - 1) {
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

/// Show ADT with IEEE formatting for mask-selected fields.
#[no_mangle]
pub extern "C" fn lumia_show_adt(x: i64, float_mask: i64) -> *mut u8 {
    let p = x as *mut u8;
    if !is_heap_payload(p) {
        return lumia_show(x);
    }
    unsafe {
        let h = header_from_payload(p);
        if tid_base((*h).type_id) != TYPE_ADT {
            return lumia_show(x);
        }
        let mask = (float_mask as u64) | (*h)._pad;
        show_adt_masked(p, mask)
    }
}

/// Typed Show: `names` is an array of `n_names` NUL-terminated variant labels (by tag).
#[no_mangle]
pub extern "C" fn lumia_show_adt_named(
    x: i64,
    float_mask: i64,
    names: *const *const u8,
    n_names: i64,
) -> *mut u8 {
    let p = x as *mut u8;
    if !is_heap_payload(p) {
        return lumia_show(x);
    }
    unsafe {
        let h = header_from_payload(p);
        if tid_base((*h).type_id) != TYPE_ADT {
            return lumia_show(x);
        }
        let mask = (float_mask as u64) | (*h)._pad;
        show_adt_masked_named(p, mask, names, n_names)
    }
}

/// Store per-field Float layout mask in ADT header `_pad` (bit `i` ⇒ field `i` is unboxed Float).
///
/// Call **after** field slots are written. Any mask bit whose slot currently holds a
/// live heap pointer is cleared — product mono sometimes types List fields as Float,
/// which would otherwise make GC skip those edges (UAF).
#[no_mangle]
pub extern "C" fn lumia_adt_set_float_mask(obj: *mut u8, float_mask: u64) {
    if obj.is_null() {
        return;
    }
    unsafe {
        let h = header_from_payload(obj);
        if tid_base((*h).type_id) != TYPE_ADT {
            return;
        }
        let mut mask = float_mask;
        let words = ((*h).size as usize) / 8;
        let nfields = words.saturating_sub(1).min(64);
        let base = obj as *const i64;
        for i in 0..nfields {
            if (mask >> i) & 1 == 0 {
                continue;
            }
            let v = *base.add(i + 1);
            if is_heap_payload(v as *mut u8) {
                mask &= !(1u64 << i);
            }
        }
        (*h)._pad = mask;
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
