//! Shared empty-container Float ensure skeleton (`ensure_*_f64`).

use crate::common::{header_from_payload, header_layout, payload_ptr, trap_abort, RC_SHARED};
use crate::gc::{init_alloc_header, insert_young, lumia_alloc};
use crate::heap::with_heap;
use lumia_abi::tid_without_hash;
use std::alloc::alloc;

/// Allocate an empty container payload (`len = 0`) with the given packed `type_id`.
#[inline]
pub(crate) fn alloc_empty_container(tid: u32) -> *mut u8 {
    let dest = lumia_alloc(8, tid);
    unsafe {
        *(dest as *mut i64) = 0;
    }
    dest
}

/// Immortal untagged empty when `tid` is plain `base`; otherwise a fresh empty
/// that keeps Float/Bool/Assoc tags (remove-to-empty / empty++empty).
#[inline]
pub(crate) fn empty_container_preserving_tags(
    tid: u32,
    base: u32,
    immortal: extern "C" fn() -> *mut u8,
) -> *mut u8 {
    let tid = tid_without_hash(tid);
    if tid == base {
        immortal()
    } else {
        alloc_empty_container(tid)
    }
}

/// Process-immortal empty Map/Set/List-style payload (`count = 0`, `RC_SHARED`).
/// Lazily fills `slot` under the heap lock (same pattern as [`crate::list::lumia_list_empty`]).
pub(crate) fn immortal_empty_container(
    get: fn(&crate::heap::Heap) -> *mut u8,
    set: fn(&mut crate::heap::Heap, *mut u8),
    tid: u32,
) -> *mut u8 {
    with_heap(|h| {
        let cur = get(h);
        if !cur.is_null() {
            return cur;
        }
        let dest = unsafe {
            let layout = header_layout(8);
            let mem = alloc(layout);
            if mem.is_null() {
                trap_abort("lumia: out of memory");
            }
            let header = init_alloc_header(mem, 8, tid);
            insert_young(h, header, 8);
            payload_ptr(header)
        };
        unsafe {
            *(dest as *mut i64) = 0;
            (*header_from_payload(dest)).rc = RC_SHARED;
            (*header_from_payload(dest))._pad = 0;
        }
        if get(h).is_null() {
            h.perm.push(dest);
            set(h, dest);
            dest
        } else {
            get(h)
        }
    })
}

/// Shared Float ensure control flow for List/Map/Set:
/// - null → fresh empty with `null_tid`
/// - `already_tagged(tid)` → identity
/// - else `classify_retag(tid, ptr)` → `Ok(new_tid)` allocates empty, `Err(msg)` traps
pub(crate) fn ensure_empty_float_retag(
    ptr: *mut u8,
    null_tid: u32,
    already_tagged: impl Fn(u32) -> bool,
    classify_retag: impl FnOnce(u32, *mut u8) -> Result<u32, String>,
) -> *mut u8 {
    if ptr.is_null() {
        return alloc_empty_container(null_tid);
    }
    unsafe {
        let tid = (*header_from_payload(ptr)).type_id;
        if already_tagged(tid) {
            return ptr;
        }
        match classify_retag(tid, ptr) {
            Ok(new_tid) => alloc_empty_container(new_tid),
            Err(msg) => trap_abort(&msg),
        }
    }
}
