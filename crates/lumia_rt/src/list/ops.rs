//! List transforms: take/slice/concat/sort and ranges.

use super::core::{force_heap_list, list_len_of, lumia_list_empty, lumia_list_promote};
use super::tid::{heap_list_tid, list_float_elems, list_tid};
use crate::common::{trap_abort, GcInhibitGuard, TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA};
use crate::gc::{list_payload_bytes, lumia_alloc};
use crate::show_eq::lumia_ord_cmp;
use crate::string_io::{lumia_alloc_string, with_str_bytes};

#[no_mangle]
pub extern "C" fn lumia_list_take(list: *mut u8, n: i64) -> *mut u8 {
    // Iota take can stay virtual: [start, start+take).
    if list_tid(list) == TYPE_LIST_IOTA {
        let len = list_len_of(list);
        let take = if n < 0 {
            0
        } else if n > len {
            len
        } else {
            n
        };
        unsafe {
            let base = list as *const i64;
            let start = *base;
            let end = start
                .checked_add(take)
                .unwrap_or_else(|| trap_abort("lumia: iota take overflow"));
            return lumia_range(start, end);
        }
    }
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let take = if n < 0 {
            0
        } else if n > len {
            len
        } else {
            n
        };
        let dest = lumia_alloc(list_payload_bytes(take), heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: list take OOM");
        }
        let dst = dest as *mut i64;
        *dst = take;
        if !list.is_null() && take > 0 {
            let src = list as *const i64;
            for i in 0..take as usize {
                *dst.add(1 + i) = *src.add(1 + i);
            }
        }
        dest
    }
}

/// Reverse element order into a new list.
#[no_mangle]
pub extern "C" fn lumia_list_reverse(list: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let dest = lumia_alloc(list_payload_bytes(len), heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: list reverse OOM");
        }
        let dst = dest as *mut i64;
        *dst = len;
        if !list.is_null() && len > 0 {
            let src = list as *const i64;
            let n = len as usize;
            for i in 0..n {
                *dst.add(1 + i) = *src.add(n - i);
            }
        }
        dest
    }
}

/// Sort `List[Int]` ascending (stable via slice::sort).
#[no_mangle]
pub extern "C" fn lumia_list_sort(list: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let n = len as usize;
        let dest = lumia_alloc(list_payload_bytes(len), TYPE_LIST);
        if dest.is_null() {
            trap_abort("lumia: list sort OOM");
        }
        let dst = dest as *mut i64;
        *dst = len;
        if !list.is_null() && n > 0 {
            let src = list as *const i64;
            for i in 0..n {
                *dst.add(1 + i) = *src.add(1 + i);
            }
            let slice = std::slice::from_raw_parts_mut(dst.add(1), n);
            slice.sort();
        }
        dest
    }
}

/// Stable permute of `values` by parallel Ord keys (Int / String / Char).
#[no_mangle]
pub extern "C" fn lumia_list_sort_by_keys(values: *mut u8, keys: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let values = force_heap_list(values);
    let keys = force_heap_list(keys);
    unsafe {
        let n = if values.is_null() {
            0i64
        } else {
            *(values as *const i64)
        };
        let nk = if keys.is_null() {
            0i64
        } else {
            *(keys as *const i64)
        };
        if n != nk {
            trap_abort("lumia: sortBy keys/values length mismatch");
        }
        let dest = lumia_alloc(list_payload_bytes(n), heap_list_tid(values));
        if dest.is_null() {
            trap_abort("lumia: list sortBy OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        if n == 0 {
            return dest;
        }
        let mut order: Vec<(i64, usize)> = Vec::with_capacity(n as usize);
        let kbase = keys as *const i64;
        for i in 0..n as usize {
            order.push((*kbase.add(1 + i), i));
        }
        order.sort_by(|a, b| lumia_ord_cmp(a.0, b.0).then(a.1.cmp(&b.1)));
        let vbase = values as *const i64;
        for (w, &(_, i)) in order.iter().enumerate() {
            *dst.add(1 + w) = *vbase.add(1 + i);
        }
        dest
    }
}
#[no_mangle]
pub extern "C" fn lumia_list_join(list: *mut u8, sep: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    let sep_bytes = with_str_bytes(sep, |b| b.to_vec());
    let parts: Vec<Vec<u8>> = unsafe {
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let mut out = Vec::with_capacity(len as usize);
        if !list.is_null() {
            let base = list as *const i64;
            for i in 0..len as usize {
                let p = *base.add(1 + i) as *mut u8;
                out.push(with_str_bytes(p, |b| b.to_vec()));
            }
        }
        out
    };
    let mut buf: Vec<u8> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            buf.extend_from_slice(&sep_bytes);
        }
        buf.extend_from_slice(p);
    }
    lumia_alloc_string(buf.as_ptr(), buf.len() as u64)
}

/// Immutable update: new List with index `i` set to `elem` (bounds trap).
#[no_mangle]
pub extern "C" fn lumia_list_set(list: *mut u8, index: i64, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        if list.is_null() || index < 0 {
            trap_abort("lumia: list set out of bounds");
        }
        let n = *(list as *const i64);
        if index >= n {
            trap_abort("lumia: list set out of bounds");
        }
        let nbytes = list_payload_bytes(n);
        let dest = lumia_alloc(nbytes, heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: list set OOM");
        }
        let src = list as *const i64;
        let dst = dest as *mut i64;
        *dst = n;
        for j in 0..n as usize {
            *dst.add(1 + j) = *src.add(1 + j);
        }
        *dst.add(1 + index as usize) = elem;
        dest
    }
}

/// Return a new HeapList that is `a` followed by `b`.
#[no_mangle]
pub extern "C" fn lumia_list_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let a = force_heap_list(a);
    let b = force_heap_list(b);
    unsafe {
        let na = if a.is_null() {
            0i64
        } else {
            *(a as *const i64)
        };
        let nb = if b.is_null() {
            0i64
        } else {
            *(b as *const i64)
        };
        // Immutable lists: concat with empty is identity (share the other),
        // but stack LitList must be promoted before the pointer escapes.
        if na == 0 {
            return if nb == 0 {
                lumia_list_empty()
            } else {
                lumia_list_promote(b)
            };
        }
        if nb == 0 {
            return lumia_list_promote(a);
        }
        let n = na
            .checked_add(nb)
            .unwrap_or_else(|| trap_abort("lumia: list concat length overflow"));
        let nbytes = list_payload_bytes(n);
        let tid = if list_float_elems(a) || list_float_elems(b) {
            TYPE_LIST_F64
        } else {
            TYPE_LIST
        };
        let dest = lumia_alloc(nbytes, tid);
        if dest.is_null() {
            trap_abort("lumia: list concat OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        let src = a as *const i64;
        for i in 0..na as usize {
            *dst.add(1 + i) = *src.add(1 + i);
        }
        let src = b as *const i64;
        for i in 0..nb as usize {
            *dst.add(1 + na as usize + i) = *src.add(1 + i);
        }
        dest
    }
}

/// Return a new list with elements from `start` to end (Iota stays virtual).
#[no_mangle]
pub extern "C" fn lumia_list_slice(list: *mut u8, start: i64) -> *mut u8 {
    if list.is_null() {
        return lumia_list_empty();
    }
    if list_tid(list) == TYPE_LIST_IOTA {
        unsafe {
            let base = list as *const i64;
            let s0 = *base;
            let end = *base.add(1);
            let start = if start < 0 { 0 } else { start };
            let abs = s0
                .checked_add(start)
                .unwrap_or_else(|| trap_abort("lumia: iota slice overflow"));
            if abs >= end {
                return lumia_range(s0, s0);
            }
            return lumia_range(abs, end);
        }
    }
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let len = *(list as *const i64);
        let start = if start < 0 { 0 } else { start };
        let n = if start >= len { 0i64 } else { len - start };
        let dest = lumia_alloc(list_payload_bytes(n), heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: slice OOM");
        }
        *(dest as *mut i64) = n;
        let src = list as *const i64;
        let dst = dest as *mut i64;
        for i in 0..n as usize {
            *dst.add(1 + i) = *src.add(1 + start as usize + i);
        }
        dest
    }
}

/// Build `[start, end)` as Iota (`TYPE_LIST_IOTA`) — O(1), no element materialization.
#[no_mangle]
pub extern "C" fn lumia_range(start: i64, end: i64) -> *mut u8 {
    let end = if end > start { end } else { start };
    let dest = lumia_alloc(16, TYPE_LIST_IOTA);
    unsafe {
        let base = dest as *mut i64;
        *base = start;
        *base.add(1) = end;
    }
    dest
}

/// Build `[start, end]` inclusive.
#[no_mangle]
pub extern "C" fn lumia_range_inclusive(start: i64, end: i64) -> *mut u8 {
    if end < start {
        return lumia_range(start, start);
    }
    match end.checked_add(1) {
        Some(excl) => lumia_range(start, excl),
        None => trap_abort("lumia: rangeInclusive overflow"),
    }
}
