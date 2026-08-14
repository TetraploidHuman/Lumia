//! List length/get, promote, COW append, and empty singleton.

use super::tid::{heap_list_tid, list_tid};
use crate::common::{
    header_from_payload, is_heap_payload, list_rc_is_unique, tid_base, trap_abort, GcInhibitGuard,
    TYPE_LIST, TYPE_LIST_IOTA,
};
use crate::gc::{list_payload_bytes, lumia_alloc};

/// HeapList: `[len][elem…]`; Iota: `[start][end_exclusive]`.
pub(crate) fn list_len_of(list: *mut u8) -> i64 {
    if list.is_null() {
        return 0;
    }
    unsafe {
        match (*header_from_payload(list)).type_id {
            TYPE_LIST_IOTA => {
                let base = list as *const i64;
                let start = *base;
                let end = *base.add(1);
                if end > start {
                    end.checked_sub(start)
                        .unwrap_or_else(|| trap_abort("lumia: iota length overflow"))
                } else {
                    0
                }
            }
            _ => *(list as *const i64),
        }
    }
}

pub(crate) fn list_get_of(list: *mut u8, index: i64) -> i64 {
    if list.is_null() || index < 0 {
        trap_abort("lumia: list get out of bounds");
    }
    unsafe {
        match (*header_from_payload(list)).type_id {
            TYPE_LIST_IOTA => {
                let base = list as *const i64;
                let start = *base;
                let end = *base.add(1);
                let len = if end > start {
                    end.checked_sub(start)
                        .unwrap_or_else(|| trap_abort("lumia: iota length overflow"))
                } else {
                    0
                };
                if index >= len {
                    trap_abort("lumia: list get out of bounds");
                }
                start
                    .checked_add(index)
                    .unwrap_or_else(|| trap_abort("lumia: iota index overflow"))
            }
            _ => {
                let len = *(list as *const i64);
                if index >= len {
                    trap_abort("lumia: list get out of bounds");
                }
                let base = list as *const i64;
                *base.add(1 + index as usize)
            }
        }
    }
}

/// Materialize Iota → HeapList; promote stack LitList; identity for heap / null.
pub(crate) fn force_heap_list(list: *mut u8) -> *mut u8 {
    if list.is_null() {
        return list;
    }
    let tid = list_tid(list);
    if tid != TYPE_LIST_IOTA {
        // Stack LitList must become heap before escape into containers / kernels.
        if tid_base(tid) == TYPE_LIST && !is_heap_payload(list) {
            return lumia_list_promote(list);
        }
        return list;
    }
    let _guard = GcInhibitGuard::enter();
    let n = list_len_of(list);
    if n < 0 {
        trap_abort("lumia: iota length overflow");
    }
    let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST);
    unsafe {
        let dst = dest as *mut i64;
        *dst = n;
        let base = list as *const i64;
        let start = *base;
        for i in 0..n as usize {
            let v = start
                .checked_add(i as i64)
                .unwrap_or_else(|| trap_abort("lumia: iota element overflow"));
            *dst.add(1 + i) = v;
        }
    }
    dest
}

/// Promote stack `LitList` to a heap list so the pointer may escape.
/// Immortal empty singleton, existing heap payloads (incl. Iota) are unchanged.
#[no_mangle]
pub extern "C" fn lumia_list_promote(list: *mut u8) -> *mut u8 {
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
        trap_abort("lumia: list promote length overflow");
    }
    let dest = lumia_alloc(list_payload_bytes(n), tid);
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
pub extern "C" fn lumia_list_len(list: *mut u8) -> i64 {
    list_len_of(list)
}

#[no_mangle]
pub extern "C" fn lumia_list_get(list: *mut u8, index: i64) -> i64 {
    list_get_of(list, index)
}

/// Capacity (element slots) from the allocated payload size (`[len][elem…]`).
#[inline]
fn list_capacity_elems(list: *mut u8) -> i64 {
    if list.is_null() {
        return 0;
    }
    unsafe {
        let nbytes = (*header_from_payload(list)).size as i64;
        (nbytes / 8) - 1
    }
}

fn list_grow_cap(needed: i64) -> i64 {
    // Geometric growth: amortize repeated unique appends.
    let mut cap = 4i64;
    while cap < needed {
        cap = cap
            .checked_mul(2)
            .unwrap_or_else(|| trap_abort("lumia: list capacity overflow"));
    }
    cap
}

/// Return a HeapList with `elem` appended (COW: unique + spare capacity → in-place).
#[no_mangle]
pub extern "C" fn lumia_list_append(list: *mut u8, elem: i64) -> *mut u8 {
    // Keep materialized Iota alive across the following alloc/copy.
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        let n = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let n1 = n
            .checked_add(1)
            .unwrap_or_else(|| trap_abort("lumia: list append length overflow"));
        let tid = heap_list_tid(list);

        // Unique owner with spare capacity → write in place (DESIGN §5.3 / §7.1.1 COWList).
        if !list.is_null() && list_rc_is_unique(list) && list_capacity_elems(list) >= n1 {
            let dst = list as *mut i64;
            *dst = n1;
            *dst.add(n1 as usize) = elem;
            // Old list + young heap elem → remembered set for minor GC.
            // Skip for Float-elem lists: payload words are IEEE bits, not pointers.
            if !super::tid::list_float_elems(list) {
                crate::lumia_write_barrier(list, n1 as u32, elem as *mut u8);
            }
            return list;
        }

        let cap = if !list.is_null() && list_rc_is_unique(list) {
            list_grow_cap(n1.max(list_capacity_elems(list).saturating_mul(2)))
        } else {
            list_grow_cap(n1)
        };
        let nbytes = list_payload_bytes(cap);
        let dest = lumia_alloc(nbytes, tid);
        if dest.is_null() {
            trap_abort("lumia: list append OOM");
        }
        let dst = dest as *mut i64;
        *dst = n1;
        if !list.is_null() {
            let src = list as *const i64;
            std::ptr::copy_nonoverlapping(src.add(1), dst.add(1), n as usize);
        }
        *dst.add(n1 as usize) = elem;
        dest
    }
}

/// Retain a List value when aliasing (`val a = xs`). No-op for non-lists / ADTs.
#[no_mangle]
pub extern "C" fn lumia_list_retain(list: *mut u8) {
    crate::common::list_rc_retain(list);
}

/// Release a List alias (does not free; GC reclaims). No-op for ADTs.
#[no_mangle]
pub extern "C" fn lumia_list_release(list: *mut u8) {
    crate::common::list_rc_release(list);
}

/// Pointer identity for heap values (`List` / ADT payloads). Used to skip
/// redundant `with` when a kernel mutated buffers in place.
#[no_mangle]
pub extern "C" fn lumia_ptr_eq(a: *mut u8, b: *mut u8) -> i64 {
    i64::from(a == b)
}

/// Retain a heap List **or** ADT alias (`val a = p`, `AdtField` extract, field store).
#[no_mangle]
pub extern "C" fn lumia_adt_retain(obj: *mut u8) {
    crate::common::value_rc_retain(obj);
}

/// Release a heap List **or** ADT alias (mut-slot overwrite / field replace).
#[no_mangle]
pub extern "C" fn lumia_adt_release(obj: *mut u8) {
    crate::common::value_rc_release(obj);
}

/// Shared empty `List` (`LitList` / `listOf()`). Immortal — survives GC.
#[no_mangle]
pub extern "C" fn lumia_list_empty() -> *mut u8 {
    use crate::common::{header_from_payload, header_layout, trap_abort, RC_SHARED};
    use crate::gc::finish_alloc;
    use crate::heap::with_heap;
    use std::alloc::alloc;

    with_heap(|h| {
        if !h.empty_list.is_null() {
            return h.empty_list;
        }
        // Alloc + publish under one heap lock so GC cannot miss the singleton.
        let dest = unsafe {
            let layout = header_layout(8);
            let mem = alloc(layout);
            if mem.is_null() {
                trap_abort("lumia: out of memory");
            }
            finish_alloc(mem, 8, TYPE_LIST)
        };
        unsafe {
            *(dest as *mut i64) = 0;
            (*header_from_payload(dest)).rc = RC_SHARED;
            (*header_from_payload(dest))._pad = 0;
        }
        if h.empty_list.is_null() {
            h.perm.push(dest);
            h.empty_list = dest;
            dest
        } else {
            h.empty_list
        }
    })
}
