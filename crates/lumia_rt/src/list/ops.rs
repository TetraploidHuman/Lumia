//! List transforms: take/slice/concat/sort and ranges.
//!
//! # Safety (FFI)
//! Pointer args are null or valid List/Iota/String payloads per callee.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use super::core::{
    alloc_list_patch, force_heap_list, list_capacity_elems, list_get_of, list_grow_cap,
    list_is_patch, list_len_of, list_patch_dn, list_patch_parent, lumia_list_empty,
    lumia_list_promote, LIST_PATCH_MAX,
};
use super::tid::{heap_list_tid, list_bool_elems, list_float_elems, list_tid};
use crate::common::{list_rc_is_unique, trap_abort, GcInhibitGuard, TYPE_LIST, TYPE_LIST_IOTA};
use crate::gc::{list_payload_bytes, lumia_alloc};
use crate::hash_ord::lumia_ord_cmp;
use crate::string_io::{lumia_alloc_string, with_str_bytes};
use lumia_abi::{list_type_id_flags, tid_list_patch};
use std::ptr;

///
/// # Safety
/// `list` is null or a valid List/Iota payload; returned list is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_list_take(list: *mut u8, n: i64) -> *mut u8 {
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
    let list = if list_is_patch(list) {
        force_heap_list(list)
    } else {
        list
    };
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
            ptr::copy_nonoverlapping(src.add(1), dst.add(1), take as usize);
        }
        dest
    }
}

/// Reverse element order. Unique dense lists reverse in place.
///
/// # Safety
/// `list` is null or a valid List payload (promoted if needed).
#[no_mangle]
pub unsafe extern "C" fn lumia_list_reverse(list: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        if list_tid(list) == TYPE_LIST_IOTA {
            let n = list_len_of(list);
            let dest = lumia_alloc(list_payload_bytes(n), lumia_abi::list_type_id_int());
            if dest.is_null() {
                trap_abort("lumia: list reverse OOM");
            }
            let dst = dest as *mut i64;
            *dst = n;
            if n > 0 {
                let start = *(list as *const i64);
                for i in 0..n as usize {
                    let v = start
                        .checked_add(n - 1 - i as i64)
                        .unwrap_or_else(|| trap_abort("lumia: iota reverse overflow"));
                    *dst.add(1 + i) = v;
                }
            }
            return dest;
        }
        let list = force_heap_list(list);
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        if len <= 1 {
            return if list.is_null() {
                lumia_list_empty()
            } else {
                list
            };
        }
        if list_rc_is_unique(list) {
            let dst = list as *mut i64;
            let n = len as usize;
            for i in 0..n / 2 {
                let a = dst.add(1 + i);
                let b = dst.add(n - i);
                let tmp = *a;
                *a = *b;
                *b = tmp;
            }
            return list;
        }
        let dest = lumia_alloc(list_payload_bytes(len), heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: list reverse OOM");
        }
        let dst = dest as *mut i64;
        *dst = len;
        let src = list as *const i64;
        let n = len as usize;
        for i in 0..n {
            *dst.add(1 + i) = *src.add(n - i);
        }
        dest
    }
}

/// Sort `List[Int]` ascending (stable via slice::sort).
/// Float-elem lists are rejected (IEEE bit order ≠ numeric / key order).
///
/// # Safety
/// `list` is null or a valid Int-elem List payload; returned list is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_list_sort(list: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    if !list.is_null() && list_float_elems(list) {
        trap_abort("lumia: list.sort is not defined for List[Float]");
    }
    unsafe {
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        if len < 0 {
            trap_abort("lumia: list sort negative length");
        }
        let n = len as usize;
        if list_rc_is_unique(list) || list.is_null() {
            if n > 1 {
                let slice = std::slice::from_raw_parts_mut((list as *mut i64).add(1), n);
                slice.sort();
            }
            return if list.is_null() {
                lumia_list_empty()
            } else {
                list
            };
        }
        let dest = lumia_alloc(list_payload_bytes(len), heap_list_tid(list));
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
///
/// # Safety
/// `list`/`keys` are null or valid List payloads of equal length; returned list is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_list_sort_by_keys(values: *mut u8, keys: *mut u8) -> *mut u8 {
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
        if n <= 1 && (list_rc_is_unique(values) || values.is_null()) {
            return if values.is_null() {
                lumia_list_empty()
            } else {
                values
            };
        }
        let mut order: Vec<(i64, usize)> = Vec::with_capacity(n as usize);
        let kbase = keys as *const i64;
        for i in 0..n as usize {
            order.push((*kbase.add(1 + i), i));
        }
        order.sort_by(|a, b| lumia_ord_cmp(a.0, b.0).then(a.1.cmp(&b.1)));
        if list_rc_is_unique(values) {
            let mut tmp = Vec::with_capacity(n as usize);
            let vbase = values as *const i64;
            for i in 0..n as usize {
                tmp.push(*vbase.add(1 + i));
            }
            let dst = values as *mut i64;
            for (w, &(_, i)) in order.iter().enumerate() {
                *dst.add(1 + w) = tmp[i];
            }
            return values;
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
        let vbase = values as *const i64;
        for (w, &(_, i)) in order.iter().enumerate() {
            *dst.add(1 + w) = *vbase.add(1 + i);
        }
        dest
    }
}
///
/// # Safety
/// `list` is null or a valid List[String] payload; `sep` is null or a valid String.
#[no_mangle]
pub unsafe extern "C" fn lumia_list_join(list: *mut u8, sep: *mut u8) -> *mut u8 {
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
    unsafe { lumia_alloc_string(buf.as_ptr(), buf.len() as u64) }
}

/// Update index `i` to `elem` (bounds trap).
///
/// COW like append: unique RC → in-place; shared → fresh copy. Codegen must
/// `retain` the source when the old binding stays live (`val ys = xs.set(…)`);
/// `xs = xs.set(…)` may consume uniqueness and write in place.
///
/// Iota / sparse patch: identity `set` is a no-op; sparse writes stay virtual
/// until escape (`force_heap_list`) or the patch delta fills (`LIST_PATCH_MAX`).
///
/// # Safety
/// `list` is null or a valid List payload; returned list is newly allocated (or unique in-place).
#[no_mangle]
pub unsafe extern "C" fn lumia_list_set(list: *mut u8, index: i64, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        if list.is_null() || index < 0 {
            trap_abort("lumia: list set out of bounds");
        }
        let n = list_len_of(list);
        if index >= n {
            trap_abort("lumia: list set out of bounds");
        }
        // Identity: skip materialize (bench memTraffic does `set(0, get(0))`).
        let tid = list_tid(list);
        if tid == TYPE_LIST_IOTA || tid_list_patch(tid) || tid_base_is_dense_heap_list(list, tid) {
            if list_get_of(list, index) == elem {
                return list;
            }
        }
        if tid == TYPE_LIST_IOTA || tid_list_patch(tid) {
            return list_set_sparse(list, index, elem, n);
        }
        let list = force_heap_list(list);
        let idx = index as usize;
        if list_rc_is_unique(list) {
            let dst = list as *mut i64;
            *dst.add(1 + idx) = elem;
            // Float elems are unboxed bits, not GC pointers (TYPE_LIST_F64).
            // Scalar Int/Bool: write_barrier early-outs, but skip the call entirely.
            if !list_float_elems(list) && crate::common::may_be_heap_payload_bits(elem) {
                crate::lumia_write_barrier(list, (1 + idx) as u32, elem as *mut u8);
            }
            return list;
        }
        let nbytes = list_payload_bytes(n);
        let dest = lumia_alloc(nbytes, heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: list set OOM");
        }
        let src = list as *const i64;
        let dst = dest as *mut i64;
        ptr::copy_nonoverlapping(src, dst, (n as usize) + 1);
        *dst.add(1 + idx) = elem;
        dest
    }
}

fn tid_base_is_dense_heap_list(list: *mut u8, tid: u32) -> bool {
    use crate::common::{is_heap_payload, may_be_heap_payload_bits, tid_base};
    tid_base(tid) == TYPE_LIST
        && !tid_list_patch(tid)
        && may_be_heap_payload_bits(list as i64)
        && is_heap_payload(list)
}

/// Sparse set on Iota or an existing patch overlay.
unsafe fn list_set_sparse(list: *mut u8, index: i64, elem: i64, len: i64) -> *mut u8 {
    if list_is_patch(list) {
        let dn = list_patch_dn(list) as usize;
        let base = list as *const i64;
        let mut hit = None;
        for i in 0..dn {
            if *base.add(3 + i * 2) == index {
                hit = Some(i);
                break;
            }
        }
        if let Some(i) = hit {
            if list_rc_is_unique(list) {
                let dst = list as *mut i64;
                *dst.add(4 + i * 2) = elem;
                if crate::common::may_be_heap_payload_bits(elem) {
                    crate::lumia_write_barrier(list, (4 + i * 2) as u32, elem as *mut u8);
                }
                return list;
            }
            // Shared: copy delta with updated value; keep same parent.
            let parent = list_patch_parent(list);
            let mut pairs = Vec::with_capacity(dn);
            for j in 0..dn {
                let idx = *base.add(3 + j * 2);
                let val = if j == i { elem } else { *base.add(4 + j * 2) };
                pairs.push((idx, val));
            }
            return alloc_list_patch(parent, len, &pairs);
        }
        if (dn as i64) >= LIST_PATCH_MAX {
            // Delta full — flatten then dense set (avoid re-entering sparse path).
            let dense = force_heap_list(list);
            let idx = index as usize;
            if list_rc_is_unique(dense) {
                let dst = dense as *mut i64;
                *dst.add(1 + idx) = elem;
                if !list_float_elems(dense) && crate::common::may_be_heap_payload_bits(elem) {
                    crate::lumia_write_barrier(dense, (1 + idx) as u32, elem as *mut u8);
                }
                return dense;
            }
            let nbytes = list_payload_bytes(len);
            let dest = lumia_alloc(nbytes, heap_list_tid(dense));
            let src = dense as *const i64;
            let dst = dest as *mut i64;
            ptr::copy_nonoverlapping(src, dst, (len as usize) + 1);
            *dst.add(1 + idx) = elem;
            return dest;
        }
        if list_rc_is_unique(list)
            && dn < unsafe { crate::container_delta::delta_entry_capacity(list, 2) }
        {
            let dst = list as *mut i64;
            *dst.add(3 + dn * 2) = index;
            *dst.add(4 + dn * 2) = elem;
            *dst.add(2) = dn as i64 + 1;
            if crate::common::may_be_heap_payload_bits(elem) {
                crate::lumia_write_barrier(list, (4 + dn * 2) as u32, elem as *mut u8);
            }
            return list;
        }
        // New index: append to delta (shared → fresh overlay).
        let parent = list_patch_parent(list);
        let mut pairs = Vec::with_capacity(dn + 1);
        for j in 0..dn {
            pairs.push((*base.add(3 + j * 2), *base.add(4 + j * 2)));
        }
        pairs.push((index, elem));
        return alloc_list_patch(parent, len, &pairs);
    }
    // Fresh patch on Iota.
    alloc_list_patch(list, len, &[(index, elem)])
}

/// Return a new HeapList that is `a` followed by `b`.
///
/// # Safety
/// `a`/`b` are null or valid List/Iota/String payloads per concat rules; returned value is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_list_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    // Identity on empty without materializing Iota (len is O(1) for Iota).
    let na = list_len_of(a);
    let nb = list_len_of(b);
    unsafe {
        // Immutable lists: concat with empty is identity (share the other),
        // but stack LitList / Iota must be promoted before the pointer escapes.
        // Both empty: keep Float/Bool tags (immortal untagged empty would drop
        // TID and break later Auto Show / IEEE eq if ensure is skipped).
        if na == 0 {
            return if nb == 0 {
                let float = list_float_elems(a) || list_float_elems(b);
                let bool_elems = (list_bool_elems(a) || list_bool_elems(b)) && !float;
                crate::ensure::empty_container_preserving_tags(
                    list_type_id_flags(float, bool_elems),
                    TYPE_LIST,
                    lumia_list_empty,
                )
            } else {
                lumia_list_promote(b)
            };
        }
        if nb == 0 {
            return lumia_list_promote(a);
        }
        if list_tid(a) == TYPE_LIST_IOTA && list_tid(b) == TYPE_LIST_IOTA {
            let ab = a as *const i64;
            let bb = b as *const i64;
            let a0 = *ab;
            let a1 = *ab.add(1);
            let b0 = *bb;
            let b1 = *bb.add(1);
            // Concat is ordered: `[a0, a1) ++ [a1, b1)` stays Iota. Reverse
            // adjacency (`b` then `a`) is not the concat order.
            if a1 == b0 {
                return lumia_range(a0, b1);
            }
        }
        let a = force_heap_list(a);
        let b = force_heap_list(b);
        let n = na
            .checked_add(nb)
            .unwrap_or_else(|| trap_abort("lumia: list concat length overflow"));
        let tid = list_type_id_flags(
            list_float_elems(a) || list_float_elems(b),
            (list_bool_elems(a) || list_bool_elems(b))
                && !(list_float_elems(a) || list_float_elems(b)),
        );
        if list_rc_is_unique(a) && list_capacity_elems(a) >= n {
            let dst = a as *mut i64;
            ptr::copy_nonoverlapping(
                (b as *const i64).add(1),
                dst.add(1 + na as usize),
                nb as usize,
            );
            *dst = n;
            if !list_float_elems(a) {
                for i in 0..nb as usize {
                    let e = *dst.add(1 + na as usize + i);
                    if crate::common::may_be_heap_payload_bits(e) {
                        crate::lumia_write_barrier(a, (1 + na as usize + i) as u32, e as *mut u8);
                    }
                }
            }
            return a;
        }
        let cap = if list_rc_is_unique(a) {
            list_grow_cap(n.max(list_capacity_elems(a).saturating_mul(2)))
        } else {
            n
        };
        let dest = lumia_alloc(list_payload_bytes(cap), tid);
        if dest.is_null() {
            trap_abort("lumia: list concat OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        let src_a = a as *const i64;
        ptr::copy_nonoverlapping(src_a.add(1), dst.add(1), na as usize);
        let src_b = b as *const i64;
        ptr::copy_nonoverlapping(src_b.add(1), dst.add(1 + na as usize), nb as usize);
        dest
    }
}

/// Return a new list with elements from `start` to end (Iota stays virtual).
///
/// # Safety
/// `list` is null or a valid List/Iota payload; returned list is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_list_slice(list: *mut u8, start: i64) -> *mut u8 {
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
    let list = if list_is_patch(list) {
        force_heap_list(list)
    } else {
        list
    };
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
        None => {
            // `end == i64::MAX`: exclusive end would overflow. Represent as a
            // one-element heap list when `start == MAX`; otherwise the range is
            // enormous and cannot be an iota.
            if start == i64::MAX {
                let dest = lumia_alloc(16, lumia_abi::list_type_id_int());
                unsafe {
                    let base = dest as *mut i64;
                    *base = 1;
                    *base.add(1) = i64::MAX;
                }
                dest
            } else {
                trap_abort("lumia: rangeInclusive overflow")
            }
        }
    }
}
