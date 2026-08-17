//! Show / format values as heap Strings.
//!
//! # Safety (FFI)
//! ADT mask setters take a heap ADT payload pointer (null is a no-op).

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use std::fmt::Write as _;

use crate::adt_show::adt_show_name_ptrs;
use crate::common::{
    header_from_payload, is_heap_payload, is_heap_payload_bits, may_be_heap_payload_bits, tid_base,
    trap_abort, GcInhibitGuard, TYPE_ADT, TYPE_CHAR, TYPE_STRING,
};
use crate::gc::lumia_alloc;
use crate::list::{list_float_elems, list_get_of, list_len_of};
use crate::map_set::{
    map_count, map_float_keys, map_float_vals, map_pair_at, set_elem_at, set_float_elems,
};
use crate::string_io::lumia_alloc_string;
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

fn alloc_from_buf(s: &str) -> *mut u8 {
    unsafe { lumia_alloc_string(s.as_ptr(), s.len() as u64) }
}

/// Append a formatted value into `buf` (no intermediate heap String for nesting).
fn append_show_value(buf: &mut String, bits: i64, as_float: bool, as_bool: bool) {
    if as_float {
        let _ = write!(buf, "{}", f64::from_bits(bits as u64));
        return;
    }
    if as_bool {
        buf.push_str(if bits != 0 { "true" } else { "false" });
        return;
    }
    if !may_be_heap_payload_bits(bits) {
        let _ = write!(buf, "{bits}");
        return;
    }
    let p = bits as *mut u8;
    if !is_heap_payload(p) {
        let _ = write!(buf, "{bits}");
        return;
    }
    // SAFETY: `p` is a live heap payload for tid / size / ADT pad reads below.
    let tid = unsafe { (*header_from_payload(p)).type_id };
    if tid == TYPE_STRING {
        let n = unsafe { (*header_from_payload(p)).size as usize };
        let bytes = unsafe { std::slice::from_raw_parts(p, n) };
        buf.push_str(std::str::from_utf8(bytes).unwrap_or("?"));
        return;
    }
    if tid == TYPE_CHAR {
        let cp = unsafe { *(p as *const i64) as u32 };
        let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
        buf.push(ch);
        return;
    }
    if tid_base(tid) == TYPE_ADT {
        let ptrs = adt_show_name_ptrs(adt_show_kind(tid));
        let (names, n_names) = if ptrs.is_empty() {
            (std::ptr::null(), 0i64)
        } else {
            (ptrs.as_ptr(), ptrs.len() as i64)
        };
        let (fmask, bmask) = unsafe {
            (
                crate::common::adt_float_mask((*header_from_payload(p))._pad),
                crate::common::adt_bool_mask((*header_from_payload(p))._pad),
            )
        };
        // SAFETY: ADT payload + optional name table from registry.
        unsafe {
            append_show_adt(buf, p, fmask, bmask, names, n_names);
        }
        return;
    }
    if is_list_tid(tid) {
        // SAFETY: list payload; float_elems reads tid only.
        unsafe {
            append_show_list(buf, p, list_float_elems(p), false);
        }
        return;
    }
    if is_map_tid(tid) {
        unsafe {
            append_show_map(buf, p, false, false);
        }
        return;
    }
    if is_set_tid(tid) {
        unsafe {
            append_show_set(buf, p, false);
        }
        return;
    }
    let _ = write!(buf, "{bits}");
}

unsafe fn append_show_list(buf: &mut String, list: *mut u8, float_elems: bool, bool_elems: bool) {
    buf.push('[');
    let n = list_len_of(list);
    for i in 0..n {
        if i > 0 {
            buf.push_str(", ");
        }
        append_show_value(buf, list_get_of(list, i), float_elems, bool_elems);
    }
    buf.push(']');
}

unsafe fn append_show_map(
    buf: &mut String,
    map: *mut u8,
    key_as_bool: bool,
    val_as_bool: bool,
) {
    buf.push('{');
    let n = map_count(map);
    let float_keys = map_float_keys(map);
    let float_vals = map_float_vals(map);
    for i in 0..n as usize {
        if i > 0 {
            buf.push_str(", ");
        }
        let (k, v) = map_pair_at(map, i);
        // Float tid wins over Bool (mutually exclusive in practice).
        append_show_value(buf, k, float_keys, key_as_bool && !float_keys);
        buf.push_str(": ");
        append_show_value(buf, v, float_vals, val_as_bool && !float_vals);
    }
    buf.push('}');
}

unsafe fn append_show_set(buf: &mut String, set: *mut u8, bool_elems: bool) {
    buf.push_str("#{");
    let n = if set.is_null() {
        0i64
    } else {
        *(set as *const i64)
    };
    let float_elems = set_float_elems(set);
    for i in 0..n as usize {
        if i > 0 {
            buf.push_str(", ");
        }
        append_show_value(
            buf,
            set_elem_at(set, i),
            float_elems,
            bool_elems && !float_elems,
        );
    }
    buf.push('}');
}

unsafe fn append_show_adt(
    buf: &mut String,
    payload: *mut u8,
    float_mask: u64,
    bool_mask: u64,
    names: *const *const u8,
    n_names: i64,
) {
    let words = ((*header_from_payload(payload)).size as usize) / 8;
    let base = payload as *const i64;
    if words == 0 {
        buf.push_str("#()");
        return;
    }
    let tag = *base;
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
        Some(n) if !n.is_empty() => buf.push_str(n),
        _ => {
            buf.push('#');
            let _ = write!(buf, "{tag}");
        }
    }
    if words == 1 {
        return;
    }
    buf.push('(');
    for i in 1..words {
        if i > 1 {
            buf.push_str(", ");
        }
        let bits = *base.add(i);
        if crate::common::adt_float_slot(float_mask, i - 1) {
            append_show_value(buf, bits, true, false);
        } else if crate::common::adt_bool_slot(bool_mask, i - 1) {
            append_show_value(buf, bits, false, true);
        } else {
            append_show_value(buf, bits, false, false);
        }
    }
    buf.push(')');
}

/// Format a value as a heap String (for interpolation).
/// Strings are returned as-is; Chars become one-character strings;
/// List/Map/Set show element contents; ADTs use `#tag(…)` when untyped,
/// or constructor / type names when the print site knows the ADT layout;
/// otherwise decimal Int.
#[no_mangle]
pub extern "C" fn lumia_show(x: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    // Int/Bool/FunRef immediates cannot be managed payloads — skip heap Mutex.
    if !may_be_heap_payload_bits(x) {
        let s = x.to_string();
        return alloc_from_buf(&s);
    }
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
                return alloc_from_buf(s);
            }
            // Containers / ADTs: one Rust buffer → one heap String (no nested show allocs).
            if tid_base(tid) == TYPE_ADT
                || is_list_tid(tid)
                || is_map_tid(tid)
                || is_set_tid(tid)
            {
                let mut s = String::new();
                append_show_value(&mut s, x, false, false);
                return alloc_from_buf(&s);
            }
        }
    }
    let s = x.to_string();
    alloc_from_buf(&s)
}

unsafe fn show_list_mode(list: *mut u8, float_elems: bool, bool_elems: bool) -> *mut u8 {
    let mut s = String::new();
    append_show_list(&mut s, list, float_elems, bool_elems);
    alloc_from_buf(&s)
}

/// Show a list whose elements are Bool (typed print sites; no list TID flag yet).
#[no_mangle]
pub extern "C" fn lumia_show_list_bool(list: i64) -> *mut u8 {
    let p = list as *mut u8;
    if !is_heap_payload_bits(list) {
        return lumia_show(list);
    }
    // SAFETY: live heap payload.
    let tid = unsafe { (*header_from_payload(p)).type_id };
    if !is_list_tid(tid) {
        return lumia_show(list);
    }
    unsafe { show_list_mode(p, false, true) }
}

/// Show a set (`as_bool` 0/1). Empty Set is null (`setOf()` / remove-to-empty).
#[no_mangle]
pub extern "C" fn lumia_show_set_bool(set: i64, as_bool: i32) -> *mut u8 {
    let p = set as *mut u8;
    let bool_elems = as_bool != 0;
    if p.is_null() {
        let mut s = String::new();
        unsafe {
            append_show_set(&mut s, std::ptr::null_mut(), bool_elems);
        }
        return alloc_from_buf(&s);
    }
    if !is_heap_payload_bits(set) {
        return lumia_show(set);
    }
    // SAFETY: live heap payload.
    let tid = unsafe { (*header_from_payload(p)).type_id };
    if !is_set_tid(tid) {
        return lumia_show(set);
    }
    let mut s = String::new();
    unsafe {
        append_show_set(&mut s, p, bool_elems);
    }
    alloc_from_buf(&s)
}

/// Show a map with Bool keys and/or values (`key_as_bool`/`val_as_bool` are 0/1).
/// Float tid flags still win for IEEE key/val formatting.
#[no_mangle]
pub extern "C" fn lumia_show_map_bool(map: i64, key_as_bool: i32, val_as_bool: i32) -> *mut u8 {
    let p = map as *mut u8;
    // Empty Map is null (`mapOf()` / remove-to-empty).
    if p.is_null() {
        let mut s = String::new();
        unsafe {
            append_show_map(
                &mut s,
                std::ptr::null_mut(),
                key_as_bool != 0,
                val_as_bool != 0,
            );
        }
        return alloc_from_buf(&s);
    }
    if !is_heap_payload_bits(map) {
        return lumia_show(map);
    }
    // SAFETY: live heap payload.
    let tid = unsafe { (*header_from_payload(p)).type_id };
    if !is_map_tid(tid) {
        return lumia_show(map);
    }
    let mut s = String::new();
    unsafe {
        append_show_map(&mut s, p, key_as_bool != 0, val_as_bool != 0);
    }
    alloc_from_buf(&s)
}

/// Show `List[ADT]` with per-field Float/Bool masks (homogeneous elems).
/// Used for `listOf(Some(true))` / `map.items()` tuples. Nested untyped show
/// ORs call-site masks with each elem's `_pad` (float + bool).
#[no_mangle]
pub extern "C" fn lumia_show_list_adt(list: i64, float_mask: i64, bool_mask: i64) -> *mut u8 {
    let p = list as *mut u8;
    if !is_heap_payload_bits(list) {
        return lumia_show(list);
    }
    unsafe {
        let h = header_from_payload(p);
        if !is_list_tid((*h).type_id) {
            return lumia_show(list);
        }
        let fmask = float_mask as u64;
        let bmask = bool_mask as u64;
        let mut s = String::new();
        s.push('[');
        let n = list_len_of(p);
        for i in 0..n {
            if i > 0 {
                s.push_str(", ");
            }
            let bits = list_get_of(p, i);
            if may_be_heap_payload_bits(bits) {
                let elem = bits as *mut u8;
                if is_heap_payload(elem) {
                    let eh = header_from_payload(elem);
                    if tid_base((*eh).type_id) == TYPE_ADT {
                        let ptrs = adt_show_name_ptrs(adt_show_kind((*eh).type_id));
                        let (names, n_names) = if ptrs.is_empty() {
                            (std::ptr::null(), 0i64)
                        } else {
                            (ptrs.as_ptr(), ptrs.len() as i64)
                        };
                        append_show_adt(
                            &mut s,
                            elem,
                            fmask | crate::common::adt_float_mask((*eh)._pad),
                            bmask | crate::common::adt_bool_mask((*eh)._pad),
                            names,
                            n_names,
                        );
                        continue;
                    }
                }
            }
            append_show_value(&mut s, bits, false, false);
        }
        s.push(']');
        alloc_from_buf(&s)
    }
}

pub(crate) unsafe fn show_adt_masked(
    payload: *mut u8,
    float_mask: u64,
    bool_mask: u64,
) -> *mut u8 {
    show_adt_masked_named(payload, float_mask, bool_mask, std::ptr::null(), 0)
}

/// Like [`show_adt_masked`], but `names[tag]` (NUL-terminated) replaces `#tag` when present.
pub(crate) unsafe fn show_adt_masked_named(
    payload: *mut u8,
    float_mask: u64,
    bool_mask: u64,
    names: *const *const u8,
    n_names: i64,
) -> *mut u8 {
    let mut s = String::new();
    append_show_adt(&mut s, payload, float_mask, bool_mask, names, n_names);
    alloc_from_buf(&s)
}

/// Show ADT with IEEE / Bool formatting for mask-selected fields.
#[no_mangle]
pub extern "C" fn lumia_show_adt(x: i64, float_mask: i64, bool_mask: i64) -> *mut u8 {
    let p = x as *mut u8;
    if !is_heap_payload_bits(x) {
        return lumia_show(x);
    }
    // SAFETY: live heap payload; ADT tid / pad checked below.
    let h = header_from_payload(p);
    if unsafe { tid_base((*h).type_id) != TYPE_ADT } {
        return lumia_show(x);
    }
    let fmask =
        (float_mask as u64) | unsafe { crate::common::adt_float_mask((*h)._pad) };
    let bmask = (bool_mask as u64) | unsafe { crate::common::adt_bool_mask((*h)._pad) };
    // SAFETY: ADT payload verified.
    unsafe { show_adt_masked(p, fmask, bmask) }
}

/// Typed Show: `names` is an array of `n_names` NUL-terminated variant labels (by tag).
///
/// # Safety
/// `names` must be null or point to `n_names` valid NUL-terminated C strings; `x` bits
/// follow the usual Show heap/immediate contract.
#[no_mangle]
pub unsafe extern "C" fn lumia_show_adt_named(
    x: i64,
    float_mask: i64,
    bool_mask: i64,
    names: *const *const u8,
    n_names: i64,
) -> *mut u8 {
    let p = x as *mut u8;
    if !is_heap_payload_bits(x) {
        return lumia_show(x);
    }
    // SAFETY: live heap payload; caller guarantees `names` contract.
    let h = header_from_payload(p);
    if tid_base((*h).type_id) != TYPE_ADT {
        return lumia_show(x);
    }
    let fmask = (float_mask as u64) | crate::common::adt_float_mask((*h)._pad);
    let bmask = (bool_mask as u64) | crate::common::adt_bool_mask((*h)._pad);
    show_adt_masked_named(p, fmask, bmask, names, n_names)
}

/// Store per-field Float layout mask in ADT header `_pad` **low 32 bits**
/// (bit `i` ⇒ field `i` is unboxed Float). Preserves bool mask in the high half.
///
/// Call **after** field slots are written. Any mask bit whose slot currently holds a
/// live heap pointer is cleared — product mono sometimes types List fields as Float,
/// which would otherwise make GC skip those edges (UAF).
///
/// # Safety
/// `obj` is null (no-op) or a valid ADT heap payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_adt_set_float_mask(obj: *mut u8, float_mask: u64) {
    if obj.is_null() {
        return;
    }
    unsafe {
        let h = header_from_payload(obj);
        if tid_base((*h).type_id) != TYPE_ADT {
            return;
        }
        let mut mask = float_mask & 0xFFFF_FFFF;
        let words = ((*h).size as usize) / 8;
        let nfields = words.saturating_sub(1).min(32);
        let base = obj as *const i64;
        for i in 0..nfields {
            if (mask >> i) & 1 == 0 {
                continue;
            }
            let v = *base.add(i + 1);
            if may_be_heap_payload_bits(v) && is_heap_payload(v as *mut u8) {
                mask &= !(1u64 << i);
            }
        }
        let bool_hi = crate::common::adt_bool_mask((*h)._pad);
        (*h)._pad = crate::common::adt_pack_field_masks(mask, bool_hi);
    }
}

/// Store per-field Bool layout mask in ADT header `_pad` **high 32 bits**.
/// Preserves float mask in the low half. Sanitizes heap-pointer slots like float.
///
/// # Safety
/// `obj` is null (no-op) or a valid ADT heap payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_adt_set_bool_mask(obj: *mut u8, bool_mask: u64) {
    if obj.is_null() {
        return;
    }
    unsafe {
        let h = header_from_payload(obj);
        if tid_base((*h).type_id) != TYPE_ADT {
            return;
        }
        let mut mask = bool_mask & 0xFFFF_FFFF;
        let words = ((*h).size as usize) / 8;
        let nfields = words.saturating_sub(1).min(32);
        let base = obj as *const i64;
        for i in 0..nfields {
            if (mask >> i) & 1 == 0 {
                continue;
            }
            let v = *base.add(i + 1);
            if may_be_heap_payload_bits(v) && is_heap_payload(v as *mut u8) {
                mask &= !(1u64 << i);
            }
        }
        let float_lo = crate::common::adt_float_mask((*h)._pad);
        (*h)._pad = crate::common::adt_pack_field_masks(float_lo, mask);
    }
}

#[no_mangle]
pub extern "C" fn lumia_show_float(n: f64) -> *mut u8 {
    let s = n.to_string();
    alloc_from_buf(&s)
}

#[no_mangle]
pub extern "C" fn lumia_show_bool(b: i8) -> *mut u8 {
    let s = if b != 0 { "true" } else { "false" };
    alloc_from_buf(s)
}
