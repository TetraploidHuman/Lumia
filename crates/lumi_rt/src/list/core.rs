//! List length/get, promote, COW append, and empty singleton.

use std::cell::Cell;
use std::ptr;

use super::tid::{heap_list_tid, list_float_elems, list_tid};
use crate::common::{
    header_from_payload, is_heap_payload, list_rc_is_unique, list_rc_release, list_rc_retain,
    tid_base, trap_abort, GcInhibitGuard, PERM_OBJECTS, RC_SHARED, TYPE_LIST, TYPE_LIST_IOTA,
    TYPE_LIST_SLICE,
};
use crate::gc::{list_payload_bytes, lumi_alloc};

/// HeapList: `[len][elem…]`; Iota: `[start][end_exclusive]`; Slice: `[parent][off][len]`.
pub(crate) fn list_len_of(list: *mut u8) -> i64 {
    if list.is_null() {
        return 0;
    }
    unsafe {
        match tid_base((*header_from_payload(list)).type_id) {
            TYPE_LIST_IOTA => {
                let base = list as *const i64;
                let start = *base;
                let end = *base.add(1);
                if end > start {
                    end.checked_sub(start)
                        .unwrap_or_else(|| trap_abort("lumi: iota length overflow"))
                } else {
                    0
                }
            }
            TYPE_LIST_SLICE => *(list as *const i64).add(2),
            _ => *(list as *const i64),
        }
    }
}

pub(crate) fn list_get_of(list: *mut u8, index: i64) -> i64 {
    if list.is_null() || index < 0 {
        trap_abort("lumi: list get out of bounds");
    }
    unsafe {
        match tid_base((*header_from_payload(list)).type_id) {
            TYPE_LIST_IOTA => {
                let base = list as *const i64;
                let start = *base;
                let end = *base.add(1);
                let len = if end > start {
                    end.checked_sub(start)
                        .unwrap_or_else(|| trap_abort("lumi: iota length overflow"))
                } else {
                    0
                };
                if index >= len {
                    trap_abort("lumi: list get out of bounds");
                }
                start
                    .checked_add(index)
                    .unwrap_or_else(|| trap_abort("lumi: iota index overflow"))
            }
            TYPE_LIST_SLICE => {
                let base = list as *const i64;
                let parent = *base as *mut u8;
                let offset = *base.add(1);
                let len = *base.add(2);
                if index >= len {
                    trap_abort("lumi: list get out of bounds");
                }
                let abs = offset
                    .checked_add(index)
                    .unwrap_or_else(|| trap_abort("lumi: slice index overflow"));
                list_get_of(parent, abs)
            }
            _ => {
                let len = *(list as *const i64);
                if index >= len {
                    trap_abort("lumi: list get out of bounds");
                }
                let base = list as *const i64;
                *base.add(1 + index as usize)
            }
        }
    }
}

/// Copy `n` leading elements of `list` (dense / iota / slice) into `dst`.
/// `dst` must have room for `n` i64 words (no length prefix).
pub(crate) unsafe fn copy_list_elems(dst: *mut i64, list: *mut u8, n: i64) {
    if n <= 0 || list.is_null() {
        return;
    }
    let n = n as usize;
    match tid_base(list_tid(list)) {
        TYPE_LIST_IOTA => {
            let start = *(list as *const i64);
            for i in 0..n {
                let v = start
                    .checked_add(i as i64)
                    .unwrap_or_else(|| trap_abort("lumi: iota element overflow"));
                *dst.add(i) = v;
            }
        }
        TYPE_LIST_SLICE => {
            let base = list as *const i64;
            let parent = *base as *mut u8;
            let offset = *base.add(1);
            match tid_base(list_tid(parent)) {
                TYPE_LIST => {
                    // Contiguous `[len][elem…]` window — bulk copy.
                    let src = (parent as *const i64).add(1 + offset as usize);
                    ptr::copy_nonoverlapping(src, dst, n);
                }
                TYPE_LIST_IOTA => {
                    let start0 = *(parent as *const i64);
                    let start = start0
                        .checked_add(offset)
                        .unwrap_or_else(|| trap_abort("lumi: slice iota overflow"));
                    for i in 0..n {
                        let v = start
                            .checked_add(i as i64)
                            .unwrap_or_else(|| trap_abort("lumi: iota element overflow"));
                        *dst.add(i) = v;
                    }
                }
                _ => {
                    for i in 0..n {
                        *dst.add(i) = list_get_of(list, i as i64);
                    }
                }
            }
        }
        _ => {
            // Dense HeapList / stack LitList.
            let src = (list as *const i64).add(1);
            ptr::copy_nonoverlapping(src, dst, n);
        }
    }
}

/// Materialize Iota/Slice → HeapList; promote stack LitList; identity for heap dense.
pub(crate) fn force_heap_list(list: *mut u8) -> *mut u8 {
    if list.is_null() {
        return list;
    }
    let tid = list_tid(list);
    let base = tid_base(tid);
    if base == TYPE_LIST_SLICE {
        return materialize_slice(list, /*release_parent_if_unique=*/ false);
    }
    if base != TYPE_LIST_IOTA {
        // Stack LitList must become heap before escape into containers / kernels.
        if base == TYPE_LIST && !is_heap_payload(list) {
            return lumi_list_promote(list);
        }
        return list;
    }
    let _guard = GcInhibitGuard::enter();
    let n = list_len_of(list);
    if n < 0 {
        trap_abort("lumi: iota length overflow");
    }
    let dest = lumi_alloc(list_payload_bytes(n), TYPE_LIST);
    unsafe {
        let dst = dest as *mut i64;
        *dst = n;
        copy_list_elems(dst.add(1), list, n);
    }
    dest
}

/// Like [`force_heap_list`], but a unique Slice drops its parent retain immediately
/// after the dense copy (DESIGN §7.1.1 — consume paths release sooner).
#[allow(dead_code)] // reserved for future consume-force call sites
pub(crate) fn force_heap_list_consume(list: *mut u8) -> *mut u8 {
    if list.is_null() {
        return list;
    }
    if tid_base(list_tid(list)) == TYPE_LIST_SLICE {
        return materialize_slice(list, /*release_parent_if_unique=*/ true);
    }
    force_heap_list(list)
}

/// Copy a slice view into a fresh dense HeapList.
/// When `release_parent_if_unique` and the slice is RC-unique, clear the parent
/// slot and `list_rc_release` so the parent can die before the next GC sweep.
fn materialize_slice(slice: *mut u8, release_parent_if_unique: bool) -> *mut u8 {
    let _guard = GcInhibitGuard::enter();
    let n = list_len_of(slice);
    let dest = lumi_alloc(list_payload_bytes(n), heap_list_tid(slice));
    unsafe {
        let dst = dest as *mut i64;
        *dst = n;
        copy_list_elems(dst.add(1), slice, n);
        if release_parent_if_unique && list_rc_is_unique(slice) {
            let base = slice as *mut i64;
            let parent = *base as *mut u8;
            *base = 0;
            if !parent.is_null() {
                list_rc_release(parent);
            }
        }
    }
    dest
}

/// Allocate `[parent][offset][len]` slice; retains `parent` for COW uniqueness.
pub(crate) fn alloc_list_slice(parent: *mut u8, offset: i64, len: i64) -> *mut u8 {
    let _guard = GcInhibitGuard::enter();
    if parent.is_null() || len <= 0 {
        return lumi_list_empty();
    }
    // Nested slice → flatten onto the dense/iota ancestor.
    let (root, off) = flatten_slice_parent(parent, offset);
    list_rc_retain(root);
    let float = list_float_elems(root);
    let tid = if float {
        TYPE_LIST_SLICE | lumi_abi::TID_F_KEY
    } else {
        TYPE_LIST_SLICE
    };
    let dest = lumi_alloc(24, tid);
    if dest.is_null() {
        trap_abort("lumi: list slice OOM");
    }
    unsafe {
        let dst = dest as *mut i64;
        *dst = root as i64;
        *dst.add(1) = off;
        *dst.add(2) = len;
    }
    dest
}

fn flatten_slice_parent(parent: *mut u8, offset: i64) -> (*mut u8, i64) {
    let tid = list_tid(parent);
    if tid_base(tid) != TYPE_LIST_SLICE {
        return (parent, offset);
    }
    unsafe {
        let base = parent as *const i64;
        let grand = *base as *mut u8;
        let off0 = *base.add(1);
        let abs = off0
            .checked_add(offset)
            .unwrap_or_else(|| trap_abort("lumi: slice offset overflow"));
        flatten_slice_parent(grand, abs)
    }
}

/// Promote stack `LitList` to a heap list so the pointer may escape.
/// Immortal empty singleton, existing heap payloads (incl. Iota) are unchanged.
#[no_mangle]
pub extern "C" fn lumi_list_promote(list: *mut u8) -> *mut u8 {
    if list.is_null() {
        return list;
    }
    // HeapList / Iota / permanent empty are already safe to escape.
    if is_heap_payload(list) {
        return list;
    }
    let tid = list_tid(list);
    if tid_base(tid) != TYPE_LIST {
        return list;
    }
    let _guard = GcInhibitGuard::enter();
    let n = list_len_of(list);
    if n < 0 {
        trap_abort("lumi: list promote length overflow");
    }
    let dest = lumi_alloc(list_payload_bytes(n), tid);
    unsafe {
        let dst = dest as *mut i64;
        let src = list as *const i64;
        *dst = n;
        // Bulk copy elems; layout is contiguous `[len][elem…]`.
        if n > 0 {
            std::ptr::copy_nonoverlapping(src.add(1), dst.add(1), n as usize);
        }
    }
    dest
}

/// List payload layout: HeapList `[len:i64][elem0:i64]…`; Iota `[start][end)`.
#[no_mangle]
pub extern "C" fn lumi_list_len(list: *mut u8) -> i64 {
    list_len_of(list)
}

#[no_mangle]
pub extern "C" fn lumi_list_get(list: *mut u8, index: i64) -> i64 {
    list_get_of(list, index)
}

/// Capacity (element slots) from the allocated payload size (`[len][elem…]`).
#[inline]
pub(crate) fn list_capacity_elems(list: *mut u8) -> i64 {
    if list.is_null() {
        return 0;
    }
    unsafe {
        let nbytes = (*header_from_payload(list)).size as i64;
        (nbytes / 8) - 1
    }
}

/// Geometric growth: amortize repeated unique appends / concats.
#[inline]
pub(crate) fn list_grow_cap(needed: i64) -> i64 {
    let mut cap = 4i64;
    while cap < needed {
        cap = cap
            .checked_mul(2)
            .unwrap_or_else(|| trap_abort("lumi: list capacity overflow"));
    }
    cap
}

/// Return a HeapList with `elem` appended (COW: unique + spare capacity → in-place).
/// Slice/Iota → one alloc + bulk copy (no materialize-then-grow double copy).
#[no_mangle]
pub extern "C" fn lumi_list_append(list: *mut u8, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let n = list_len_of(list);
    let n1 = n
        .checked_add(1)
        .unwrap_or_else(|| trap_abort("lumi: list append length overflow"));
    let tid = list_tid(list);
    let base = tid_base(tid);

    // Unique dense with spare capacity → write in place.
    if base == TYPE_LIST && !list.is_null() && is_heap_payload(list) && list_rc_is_unique(list) {
        unsafe {
            if list_capacity_elems(list) >= n1 {
                let dst = list as *mut i64;
                *dst = n1;
                *dst.add(n1 as usize) = elem;
                if !super::tid::list_float_elems(list) {
                    crate::lumi_write_barrier(list, n1 as u32, elem as *mut u8);
                }
                return list;
            }
            let cap = list_grow_cap(n1.max(list_capacity_elems(list).saturating_mul(2)));
            let nbytes = list_payload_bytes(cap);
            let dest = lumi_alloc(nbytes, heap_list_tid(list));
            if dest.is_null() {
                trap_abort("lumi: list append OOM");
            }
            let dst = dest as *mut i64;
            *dst = n1;
            copy_list_elems(dst.add(1), list, n);
            *dst.add(n1 as usize) = elem;
            return dest;
        }
    }

    // Slice / Iota / LitList / shared dense: single alloc with spare capacity.
    let list = if base == TYPE_LIST && !list.is_null() && !is_heap_payload(list) {
        lumi_list_promote(list)
    } else {
        list
    };
    let release_slice_parent =
        base == TYPE_LIST_SLICE && !list.is_null() && list_rc_is_unique(list);
    let cap = list_grow_cap(n1);
    let out_tid = if list.is_null() {
        TYPE_LIST
    } else {
        heap_list_tid(list)
    };
    let dest = lumi_alloc(list_payload_bytes(cap), out_tid);
    if dest.is_null() {
        trap_abort("lumi: list append OOM");
    }
    unsafe {
        let dst = dest as *mut i64;
        *dst = n1;
        if n > 0 {
            copy_list_elems(dst.add(1), list, n);
        }
        *dst.add(n1 as usize) = elem;
        if !super::tid::list_float_elems(dest) {
            crate::lumi_write_barrier(dest, n1 as u32, elem as *mut u8);
        }
        // Unique Slice consume: drop parent retain now (slice binding is abandoned).
        if release_slice_parent {
            let sbase = list as *mut i64;
            let parent = *sbase as *mut u8;
            *sbase = 0;
            if !parent.is_null() {
                list_rc_release(parent);
            }
        }
    }
    dest
}

/// Retain a List value when aliasing (`val a = xs`). No-op for non-lists / ADTs.
#[no_mangle]
pub extern "C" fn lumi_list_retain(list: *mut u8) {
    crate::common::list_rc_retain(list);
}

/// Release a List alias (does not free; GC reclaims). No-op for ADTs.
#[no_mangle]
pub extern "C" fn lumi_list_release(list: *mut u8) {
    crate::common::list_rc_release(list);
}

/// Pointer identity for heap values (`List` / ADT payloads). Used to skip
/// redundant `with` when a kernel mutated buffers in place.
#[no_mangle]
pub extern "C" fn lumi_ptr_eq(a: *mut u8, b: *mut u8) -> i64 {
    i64::from(a == b)
}

/// Retain a heap List **or** ADT alias (`val a = p`, `AdtField` extract, field store).
#[no_mangle]
pub extern "C" fn lumi_adt_retain(obj: *mut u8) {
    crate::common::value_rc_retain(obj);
}

/// Release a heap List **or** ADT alias (mut-slot overwrite / field replace).
#[no_mangle]
pub extern "C" fn lumi_adt_release(obj: *mut u8) {
    crate::common::value_rc_release(obj);
}

/// Shared empty `List` (`LitList` / `listOf()`). Immortal — survives GC.
#[no_mangle]
pub extern "C" fn lumi_list_empty() -> *mut u8 {
    thread_local! {
        static EMPTY: Cell<*mut u8> = const { Cell::new(ptr::null_mut()) };
    }
    EMPTY.with(|c| {
        let cur = c.get();
        if !cur.is_null() {
            return cur;
        }
        let dest = lumi_alloc(8, TYPE_LIST);
        unsafe {
            *(dest as *mut i64) = 0;
            // Immortal shared empty list — never COW in-place.
            (*header_from_payload(dest)).rc = RC_SHARED;
            (*header_from_payload(dest))._pad = 0;
        }
        PERM_OBJECTS.with(|p| p.borrow_mut().push(dest));
        c.set(dest);
        dest
    })
}
