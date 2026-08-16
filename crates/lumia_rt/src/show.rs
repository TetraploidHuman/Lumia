//! Show / format values as heap Strings.

use std::fmt::Write as _;

use crate::adt_show::adt_show_name_ptrs;
use crate::common::{
    header_from_payload, is_heap_payload, may_be_heap_payload_bits, tid_base, trap_abort,
    GcInhibitGuard, TYPE_ADT, TYPE_CHAR, TYPE_STRING,
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
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
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
    unsafe {
        let h = header_from_payload(p);
        let tid = (*h).type_id;
        if tid == TYPE_STRING {
            let n = (*h).size as usize;
            let bytes = std::slice::from_raw_parts(p, n);
            buf.push_str(std::str::from_utf8(bytes).unwrap_or("?"));
            return;
        }
        if tid == TYPE_CHAR {
            let cp = *(p as *const i64) as u32;
            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
            buf.push(ch);
            return;
        }
        if tid_base(tid) == TYPE_ADT {
            append_show_adt(buf, p, (*h)._pad, 0, std::ptr::null(), 0);
            return;
        }
        if is_list_tid(tid) {
            append_show_list(buf, p, list_float_elems(p), false);
            return;
        }
        if is_map_tid(tid) {
            append_show_map(buf, p);
            return;
        }
        if is_set_tid(tid) {
            append_show_set(buf, p);
            return;
        }
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

unsafe fn append_show_map(buf: &mut String, map: *mut u8) {
    buf.push('{');
    let n = map_count(map);
    let float_keys = map_float_keys(map);
    let float_vals = map_float_vals(map);
    for i in 0..n as usize {
        if i > 0 {
            buf.push_str(", ");
        }
        let (k, v) = map_pair_at(map, i);
        append_show_value(buf, k, float_keys, false);
        buf.push_str(": ");
        append_show_value(buf, v, float_vals, false);
    }
    buf.push('}');
}

unsafe fn append_show_set(buf: &mut String, set: *mut u8) {
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
        append_show_value(buf, set_elem_at(set, i), float_elems, false);
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
        } else if crate::common::adt_float_slot(bool_mask, i - 1) {
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

pub(crate) unsafe fn show_adt(payload: *mut u8) -> *mut u8 {
    let h = header_from_payload(payload);
    let tid = (*h).type_id;
    let mask = (*h)._pad;
    let kind = adt_show_kind(tid);
    let ptrs = adt_show_name_ptrs(kind);
    if !ptrs.is_empty() {
        return show_adt_masked_named(payload, mask, 0, ptrs.as_ptr(), ptrs.len() as i64);
    }
    show_adt_masked(payload, mask, 0)
}

pub(crate) fn show_value_bits(bits: i64, as_float: bool) -> String {
    show_value_bits_mode(bits, as_float, false)
}

fn show_value_bits_mode(bits: i64, as_float: bool, as_bool: bool) -> String {
    let mut s = String::new();
    append_show_value(&mut s, bits, as_float, as_bool);
    s
}

pub(crate) unsafe fn show_list(list: *mut u8) -> *mut u8 {
    show_list_mode(list, list_float_elems(list), false)
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
    if !is_heap_payload(p) {
        return lumia_show(list);
    }
    unsafe {
        let h = header_from_payload(p);
        if !is_list_tid((*h).type_id) {
            return lumia_show(list);
        }
        show_list_mode(p, false, true)
    }
}

pub(crate) unsafe fn show_map(map: *mut u8) -> *mut u8 {
    let mut s = String::new();
    append_show_map(&mut s, map);
    alloc_from_buf(&s)
}

pub(crate) unsafe fn show_set(set: *mut u8) -> *mut u8 {
    let mut s = String::new();
    append_show_set(&mut s, set);
    alloc_from_buf(&s)
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
    if !is_heap_payload(p) {
        return lumia_show(x);
    }
    unsafe {
        let h = header_from_payload(p);
        if tid_base((*h).type_id) != TYPE_ADT {
            return lumia_show(x);
        }
        let mask = (float_mask as u64) | (*h)._pad;
        show_adt_masked(p, mask, bool_mask as u64)
    }
}

/// Typed Show: `names` is an array of `n_names` NUL-terminated variant labels (by tag).
#[no_mangle]
pub extern "C" fn lumia_show_adt_named(
    x: i64,
    float_mask: i64,
    bool_mask: i64,
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
        show_adt_masked_named(p, mask, bool_mask as u64, names, n_names)
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
    alloc_from_buf(&s)
}

#[no_mangle]
pub extern "C" fn lumia_show_bool(b: i8) -> *mut u8 {
    let s = if b != 0 { "true" } else { "false" };
    alloc_from_buf(s)
}
