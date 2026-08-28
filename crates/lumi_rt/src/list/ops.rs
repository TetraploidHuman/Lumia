//! List transforms: take/slice/concat/sort and ranges.

use super::core::{
    alloc_list_slice, copy_list_elems, force_heap_list, list_capacity_elems, list_grow_cap,
    list_len_of, lumi_list_empty, lumi_list_promote,
};
use super::tid::{heap_list_tid, list_float_elems, list_tid};
use crate::common::{
    list_rc_is_unique, list_rc_retain, tid_base, trap_abort, GcInhibitGuard, TYPE_LIST,
    TYPE_LIST_IOTA, TYPE_LIST_SLICE,
};
use crate::gc::{list_payload_bytes, lumi_alloc};
use crate::hash_ord::lumi_ord_cmp;
use crate::string_io::{lumi_alloc_string, with_str_bytes};
use lumi_abi::list_type_id;
use std::ptr;

#[no_mangle]
pub extern "C" fn lumi_list_take(list: *mut u8, n: i64) -> *mut u8 {
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
                .unwrap_or_else(|| trap_abort("lumi: iota take overflow"));
            return lumi_range(start, end);
        }
    }
    let len = list_len_of(list);
    let take = if n < 0 {
        0
    } else if n > len {
        len
    } else {
        n
    };
    if take == 0 {
        return lumi_list_empty();
    }
    // Unique in-place shrink is only safe for `xs = xs.take(…)` (codegen
    // `lumi_list_take_consume`). Plain `take` must not mutate a live parent.
    // Full prefix → share identity (retain).
    if take == len && tid_base(list_tid(list)) != TYPE_LIST_IOTA {
        let p = if tid_base(list_tid(list)) == TYPE_LIST && !is_heap_payload_list(list) {
            lumi_list_promote(list)
        } else {
            list
        };
        list_rc_retain(p);
        return p;
    }
    // Shared / prefix → Slice view; parent retain for COW.
    alloc_list_slice(list, 0, take)
}

/// `xs = xs.take(n)` when RC-unique: shrink dense/slice in place (no new alloc).
#[no_mangle]
pub extern "C" fn lumi_list_take_consume(list: *mut u8, n: i64) -> *mut u8 {
    if list_tid(list) == TYPE_LIST_IOTA {
        return lumi_list_take(list, n);
    }
    let len = list_len_of(list);
    let take = if n < 0 {
        0
    } else if n > len {
        len
    } else {
        n
    };
    if take == 0 {
        return lumi_list_empty();
    }
    if tid_base(list_tid(list)) == TYPE_LIST
        && is_heap_payload_list(list)
        && list_rc_is_unique(list)
    {
        unsafe {
            *(list as *mut i64) = take;
        }
        return list;
    }
    if tid_base(list_tid(list)) == TYPE_LIST_SLICE && list_rc_is_unique(list) {
        unsafe {
            *(list as *mut i64).add(2) = take;
        }
        return list;
    }
    lumi_list_take(list, n)
}

fn is_heap_payload_list(list: *mut u8) -> bool {
    crate::common::is_heap_payload(list)
}

/// Reverse element order into a fresh list (never mutates a live binding).
#[no_mangle]
pub extern "C" fn lumi_list_reverse(list: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let n = list_len_of(list);
    if n <= 1 {
        if n == 0 {
            return lumi_list_empty();
        }
        // Single element: share identity (retain).
        let p = lumi_list_promote(list);
        list_rc_retain(p);
        return p;
    }
    let dest = lumi_alloc(list_payload_bytes(n), heap_list_tid(list));
    if dest.is_null() {
        trap_abort("lumi: list reverse OOM");
    }
    unsafe {
        let dst = dest as *mut i64;
        *dst = n;
        copy_list_elems(dst.add(1), list, n);
        let half = (n as usize) / 2;
        for i in 0..half {
            let a = dst.add(1 + i);
            let b = dst.add(n as usize - i);
            let tmp = *a;
            *a = *b;
            *b = tmp;
        }
    }
    dest
}

/// `xs = xs.reverse()` when RC-unique: swap in place (no alloc).
#[no_mangle]
pub extern "C" fn lumi_list_reverse_consume(list: *mut u8) -> *mut u8 {
    let n = list_len_of(list);
    if n <= 1 {
        return lumi_list_reverse(list);
    }
    if tid_base(list_tid(list)) == TYPE_LIST
        && is_heap_payload_list(list)
        && list_rc_is_unique(list)
    {
        unsafe {
            let dst = list as *mut i64;
            let half = (n as usize) / 2;
            for i in 0..half {
                let a = dst.add(1 + i);
                let b = dst.add(n as usize - i);
                let tmp = *a;
                *a = *b;
                *b = tmp;
            }
        }
        return list;
    }
    lumi_list_reverse(list)
}

/// Sort `List[Int]` ascending (stable via slice::sort).
/// Float-elem lists are rejected (IEEE bit order ≠ numeric / key order).
#[no_mangle]
pub extern "C" fn lumi_list_sort(list: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let n = list_len_of(list);
    if !list.is_null() && list_float_elems(list) {
        trap_abort("lumi: list.sort is not defined for List[Float]");
    }
    if n <= 1 {
        if n == 0 {
            return lumi_list_empty();
        }
        let p = lumi_list_promote(list);
        list_rc_retain(p);
        return p;
    }
    let dest = lumi_alloc(list_payload_bytes(n), heap_list_tid(list));
    if dest.is_null() {
        trap_abort("lumi: list sort OOM");
    }
    unsafe {
        let dst = dest as *mut i64;
        *dst = n;
        copy_list_elems(dst.add(1), list, n);
        let slice = std::slice::from_raw_parts_mut(dst.add(1), n as usize);
        slice.sort();
    }
    dest
}

/// `xs = xs.sort()` when RC-unique: sort the buffer in place.
#[no_mangle]
pub extern "C" fn lumi_list_sort_consume(list: *mut u8) -> *mut u8 {
    let n = list_len_of(list);
    if n <= 1 {
        return lumi_list_sort(list);
    }
    if !list.is_null() && list_float_elems(list) {
        trap_abort("lumi: list.sort is not defined for List[Float]");
    }
    if tid_base(list_tid(list)) == TYPE_LIST
        && is_heap_payload_list(list)
        && list_rc_is_unique(list)
    {
        unsafe {
            let dst = list as *mut i64;
            let slice = std::slice::from_raw_parts_mut(dst.add(1), n as usize);
            slice.sort();
        }
        return list;
    }
    lumi_list_sort(list)
}

/// Stable permute of `values` by parallel Ord keys (Int / String / Char).
#[no_mangle]
pub extern "C" fn lumi_list_sort_by_keys(values: *mut u8, keys: *mut u8) -> *mut u8 {
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
            trap_abort("lumi: sortBy keys/values length mismatch");
        }
        let dest = lumi_alloc(list_payload_bytes(n), heap_list_tid(values));
        if dest.is_null() {
            trap_abort("lumi: list sortBy OOM");
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
        order.sort_by(|a, b| lumi_ord_cmp(a.0, b.0).then(a.1.cmp(&b.1)));
        let vbase = values as *const i64;
        for (w, &(_, i)) in order.iter().enumerate() {
            *dst.add(1 + w) = *vbase.add(1 + i);
        }
        dest
    }
}
#[no_mangle]
pub extern "C" fn lumi_list_join(list: *mut u8, sep: *mut u8) -> *mut u8 {
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
    lumi_alloc_string(buf.as_ptr(), buf.len() as u64)
}

/// Update index `i` to `elem` (bounds trap).
///
/// COW like append: unique RC → in-place; shared → fresh copy. Codegen must
/// `retain` the source when the old binding stays live (`val ys = xs.set(…)`);
/// `xs = xs.set(…)` may consume uniqueness and write in place.
#[no_mangle]
pub extern "C" fn lumi_list_set(list: *mut u8, index: i64, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if list.is_null() || index < 0 {
        trap_abort("lumi: list set out of bounds");
    }
    let n = list_len_of(list);
    if index >= n {
        trap_abort("lumi: list set out of bounds");
    }
    let idx = index as usize;
    let base = tid_base(list_tid(list));

    // Unique dense → in-place.
    if base == TYPE_LIST && is_heap_payload_list(list) && list_rc_is_unique(list) {
        unsafe {
            let dst = list as *mut i64;
            *dst.add(1 + idx) = elem;
            if !list_float_elems(list) {
                crate::lumi_write_barrier(list, (1 + idx) as u32, elem as *mut u8);
            }
        }
        return list;
    }

    // Shared dense / slice / iota → one bulk copy + write.
    let nbytes = list_payload_bytes(n);
    let dest = lumi_alloc(nbytes, heap_list_tid(list));
    if dest.is_null() {
        trap_abort("lumi: list set OOM");
    }
    unsafe {
        let dst = dest as *mut i64;
        *dst = n;
        copy_list_elems(dst.add(1), list, n);
        *dst.add(1 + idx) = elem;
    }
    dest
}

/// Return a new HeapList that is `a` followed by `b`.
/// Unique dense `a` with spare capacity → extend in place (like append).
/// Unique dense without spare → geometric grow (amortized concat loops).
#[no_mangle]
pub extern "C" fn lumi_list_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    // Identity on empty without materializing Iota (len is O(1) for Iota).
    let na = list_len_of(a);
    let nb = list_len_of(b);
    unsafe {
        // Immutable lists: concat with empty is identity (share the other),
        // but stack LitList / Iota must be promoted before the pointer escapes.
        if na == 0 {
            return if nb == 0 {
                lumi_list_empty()
            } else {
                lumi_list_promote(b)
            };
        }
        if nb == 0 {
            return lumi_list_promote(a);
        }
        let n = na
            .checked_add(nb)
            .unwrap_or_else(|| trap_abort("lumi: list concat length overflow"));
        let a_base = tid_base(list_tid(a));
        let float = list_float_elems(a) || list_float_elems(b);
        let tid = list_type_id(float);

        // Unique dense left → in-place or geometric grow (COW consume path).
        if a_base == TYPE_LIST && is_heap_payload_list(a) && list_rc_is_unique(a) {
            if list_capacity_elems(a) >= n {
                let dst = a as *mut i64;
                copy_list_elems(dst.add(1 + na as usize), b, nb);
                *dst = n;
                if !float {
                    for i in 0..nb as usize {
                        let e = *dst.add(1 + na as usize + i);
                        crate::lumi_write_barrier(a, (1 + na as usize + i) as u32, e as *mut u8);
                    }
                }
                return a;
            }
            let cap = list_grow_cap(n.max(list_capacity_elems(a).saturating_mul(2)));
            let dest = lumi_alloc(list_payload_bytes(cap), tid);
            if dest.is_null() {
                trap_abort("lumi: list concat OOM");
            }
            let dst = dest as *mut i64;
            *dst = n;
            copy_list_elems(dst.add(1), a, na);
            copy_list_elems(dst.add(1 + na as usize), b, nb);
            return dest;
        }

        let dest = lumi_alloc(list_payload_bytes(n), tid);
        if dest.is_null() {
            trap_abort("lumi: list concat OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        // Bulk copy from dense/slice/iota — no intermediate materialize.
        copy_list_elems(dst.add(1), a, na);
        copy_list_elems(dst.add(1 + na as usize), b, nb);
        // Unique Slice left: release parent retain after bulk copy.
        if a_base == TYPE_LIST_SLICE && list_rc_is_unique(a) {
            let sbase = a as *mut i64;
            let parent = *sbase as *mut u8;
            *sbase = 0;
            if !parent.is_null() {
                crate::common::list_rc_release(parent);
            }
        }
        dest
    }
}

/// Return a new list with elements from `start` to end (Iota stays virtual).
#[no_mangle]
pub extern "C" fn lumi_list_slice(list: *mut u8, start: i64) -> *mut u8 {
    if list.is_null() {
        return lumi_list_empty();
    }
    if list_tid(list) == TYPE_LIST_IOTA {
        unsafe {
            let base = list as *const i64;
            let s0 = *base;
            let end = *base.add(1);
            let start = if start < 0 { 0 } else { start };
            let abs = s0
                .checked_add(start)
                .unwrap_or_else(|| trap_abort("lumi: iota slice overflow"));
            if abs >= end {
                return lumi_range(s0, s0);
            }
            return lumi_range(abs, end);
        }
    }
    let len = list_len_of(list);
    let start = if start < 0 { 0 } else { start };
    if start >= len {
        return lumi_list_empty();
    }
    let n = len - start;
    if start == 0 && n == len {
        let p = if tid_base(list_tid(list)) == TYPE_LIST && !is_heap_payload_list(list) {
            lumi_list_promote(list)
        } else {
            list
        };
        list_rc_retain(p);
        return p;
    }
    // Never mutate a live parent — views only (consume path can shrink later).
    alloc_list_slice(list, start, n)
}

/// `xs = xs.slice(n)` / `xs = xs.drop(n)` when RC-unique:
/// dense → memmove+shrink in place; slice → bump offset in place.
/// Iota has no RC (alias shares the pointer), so always allocate a fresh range.
#[no_mangle]
pub extern "C" fn lumi_list_slice_consume(list: *mut u8, start: i64) -> *mut u8 {
    if list.is_null() || list_tid(list) == TYPE_LIST_IOTA {
        return lumi_list_slice(list, start);
    }
    let len = list_len_of(list);
    let start = if start < 0 { 0 } else { start };
    if start >= len {
        return lumi_list_empty();
    }
    if start == 0 {
        return list;
    }
    let n = len - start;
    if tid_base(list_tid(list)) == TYPE_LIST
        && is_heap_payload_list(list)
        && list_rc_is_unique(list)
    {
        // Small remainder → memmove+shrink so later unique append keeps spare capacity.
        // Large remainder → Slice view (O(1)); memmove would dominate.
        if n <= 64 {
            unsafe {
                let base = list as *mut i64;
                ptr::copy(base.add(1 + start as usize), base.add(1), n as usize);
                *base = n;
            }
            return list;
        }
        return alloc_list_slice(list, start, n);
    }
    if tid_base(list_tid(list)) == TYPE_LIST_SLICE && list_rc_is_unique(list) {
        unsafe {
            let base = list as *mut i64;
            let off0 = *base.add(1);
            let abs = off0
                .checked_add(start)
                .unwrap_or_else(|| trap_abort("lumi: slice offset overflow"));
            *base.add(1) = abs;
            *base.add(2) = n;
        }
        return list;
    }
    lumi_list_slice(list, start)
}

/// Build `[start, end)` as Iota (`TYPE_LIST_IOTA`) — O(1), no element materialization.
#[no_mangle]
pub extern "C" fn lumi_range(start: i64, end: i64) -> *mut u8 {
    let end = if end > start { end } else { start };
    let dest = lumi_alloc(16, TYPE_LIST_IOTA);
    unsafe {
        let base = dest as *mut i64;
        *base = start;
        *base.add(1) = end;
    }
    dest
}

/// Build `[start, end]` inclusive.
#[no_mangle]
pub extern "C" fn lumi_range_inclusive(start: i64, end: i64) -> *mut u8 {
    if end < start {
        return lumi_range(start, start);
    }
    match end.checked_add(1) {
        Some(excl) => lumi_range(start, excl),
        None => {
            // `end == i64::MAX`: exclusive end would overflow. Represent as a
            // one-element heap list when `start == MAX`; otherwise the range is
            // enormous and cannot be an iota.
            if start == i64::MAX {
                let dest = lumi_alloc(16, TYPE_LIST);
                unsafe {
                    let base = dest as *mut i64;
                    *base = 1;
                    *base.add(1) = i64::MAX;
                }
                dest
            } else {
                trap_abort("lumi: rangeInclusive overflow")
            }
        }
    }
}
