//! Allocation / root / write-barrier C ABI.
//!
//! # Safety (FFI)
//! `slot` must point at a live root slot; `obj`/`new_ptr` follow write-barrier contracts.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use super::shade::shade_payload;
use super::MarkSweep;
use crate::common::{
    header_layout, payload_ptr, trap_abort, ObjectHeader, TYPE_ADT, TYPE_LIST, TYPE_MAP, TYPE_SET,
};
use crate::heap::Heap;
use crate::mutator::{lock_lab, LabState};
use lumia_abi::tid_base;
use std::sync::Mutex;

pub(crate) fn list_payload_bytes(len: i64) -> u64 {
    if len < 0 {
        trap_abort("lumia: negative list length");
    }
    (len as u64)
        .checked_add(1)
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as u64)
        .unwrap_or_else(|| trap_abort(&format!("lumia: list too large (len={len})")))
}

/// Fill object header fields except `marked` (set under the heap lock).
pub(crate) unsafe fn init_alloc_header(
    mem: *mut u8,
    nbytes: usize,
    type_id: u32,
) -> *mut ObjectHeader {
    if nbytes > u32::MAX as usize {
        trap_abort("lumia: allocation too large (exceeds u32 size field)");
    }
    let header = mem as *mut ObjectHeader;
    (*header).type_id = type_id;
    (*header).size = nbytes as u32;
    (*header).rc = if matches!(
        tid_base(type_id),
        TYPE_LIST | TYPE_MAP | TYPE_SET | TYPE_ADT
    ) {
        1
    } else {
        0
    };
    (*header)._pad = 0;
    // `marked` published in [`insert_young`].
    header
}

/// Publish a young object into the heap (caller holds [`with_heap`]).
///
/// Nursery-bump / TLS-LAB headers are tracked in [`crate::gc::nursery::Nursery`]
/// (`live_set` or unflushed LAB below cursor) — they are **not** dual-written
/// into `heap_set`.
pub(crate) unsafe fn insert_young(h: &mut Heap, header: *mut ObjectHeader, nbytes: usize) {
    (*header).marked = if h.full_marking { 1 } else { 0 };
    h.young.push(header);
    if !h.nursery.contains_header(header) {
        h.heap_set.insert(header);
    }
    h.bytes_young += nbytes;
    h.refresh_alloc_pressure_fast();
}

/// Publish directly into old (large-object tenure — skips young accounting).
///
/// Objects ≥ [`tenure_threshold`] would otherwise dominate `bytes_young` and
/// force a soft minor on the next alloc; tenuring keeps the nursery for churn.
pub(crate) unsafe fn insert_old(h: &mut Heap, header: *mut ObjectHeader, nbytes: usize) {
    (*header).marked = if h.full_marking { 1 } else { 0 };
    h.old.push(header);
    h.old_set.insert(header);
    h.heap_set.insert(header);
    h.bytes_old += nbytes;
    h.refresh_alloc_pressure_fast();
}

/// Size at which [`MarkSweep::alloc`] inserts into old instead of young.
#[inline]
pub(crate) fn tenure_threshold(young_limit: usize) -> usize {
    (young_limit / 2).max(1)
}

#[inline]
pub(crate) fn should_tenure(nbytes: usize, young_limit: usize) -> bool {
    nbytes >= tenure_threshold(young_limit)
}

/// Soft-GC threshold trip (same predicate as the former alloc peek).
#[inline]
pub(crate) fn soft_gc_needed(h: &Heap) -> bool {
    h.gc_inhibit == 0
        && (h.full_marking || h.bytes_young >= h.young_limit || h.bytes_old >= h.old_limit)
}

/// Flush one mutator's TLS LAB pending headers into `h.young` (caller holds heap).
pub(crate) fn flush_lab_into_heap(h: &mut Heap, lab: &Mutex<LabState>) {
    let mut g = lock_lab(lab);
    let pending = std::mem::take(&mut g.pending);
    g.clear_claim();
    drop(g);
    for header in pending {
        unsafe {
            let nbytes = (*header).size as usize;
            h.nursery.note_flushed(header);
            insert_young(h, header, nbytes);
        }
    }
}

/// TLS LAB bump without holding the heap Mutex on the fast path.
///
/// Falls back (`None`) under soft pressure / tenure / claim miss — caller uses
/// the locked nursery / system alloc path.
pub(crate) unsafe fn try_tls_lab_alloc(nbytes: usize, type_id: u32) -> Option<*mut u8> {
    use crate::gc::nursery::LAB_CLAIM_BYTES;
    use crate::heap::with_heap;
    use crate::mutator::{ensure_mutator_registered, local_lab};

    ensure_mutator_registered();
    let total = header_layout(nbytes).size().next_multiple_of(8);
    if total > LAB_CLAIM_BYTES {
        return None;
    }

    let lab = local_lab();
    {
        let mut g = lock_lab(lab);
        let fits_space = g.base != 0 && g.cursor.checked_add(total).is_some_and(|n| n <= g.end);
        let fits_budget = g.pending_bytes.saturating_add(nbytes) <= g.pending_budget;
        if fits_space && fits_budget {
            let header = g.cursor as *mut ObjectHeader;
            g.cursor += total;
            init_alloc_header(header as *mut u8, nbytes, type_id);
            (*header).marked = 0;
            g.pending.push(header);
            g.pending_bytes = g.pending_bytes.saturating_add(nbytes);
            return Some(payload_ptr(header));
        }
    }

    with_heap(|h| {
        flush_lab_into_heap(h, lab);
        // Soft pressure may trip only after publishing pending bytes.
        if soft_gc_needed(h) || should_tenure(nbytes, h.young_limit) {
            return None;
        }
        let room = h.young_limit.saturating_sub(h.bytes_young);
        if room < total {
            return None;
        }
        // Cap LAB claims by remaining young budget so soft minor still trips.
        let claim = LAB_CLAIM_BYTES.min(room).max(total);
        let (start, len) = h.nursery.claim_lab(claim)?;
        let mut g = lock_lab(lab);
        g.base = start as usize;
        g.end = (start as usize).wrapping_add(len);
        g.cursor = g.base;
        g.pending_budget = room;
        g.pending_bytes = 0;
        if g.cursor.checked_add(total).is_none_or(|n| n > g.end) {
            g.clear_claim();
            return None;
        }
        let header = g.cursor as *mut ObjectHeader;
        g.cursor += total;
        init_alloc_header(header as *mut u8, nbytes, type_id);
        (*header).marked = 0;
        g.pending.push(header);
        g.pending_bytes = nbytes;
        Some(payload_ptr(header))
    })
}

/// Insert into young or old under the current heap limits (caller holds lock).
#[inline]
pub(crate) unsafe fn insert_alloc(h: &mut Heap, header: *mut ObjectHeader, nbytes: usize) {
    if should_tenure(nbytes, h.young_limit) {
        insert_old(h, header, nbytes);
    } else {
        insert_young(h, header, nbytes);
    }
}

#[no_mangle]
pub extern "C" fn lumia_alloc(nbytes: u64, type_id: u32) -> *mut u8 {
    MarkSweep.alloc(nbytes as usize, type_id)
}

///
/// # Safety
/// `slot` must point at a live mutator root slot for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn lumia_root_push(slot: *mut *mut u8) {
    crate::mutator::ensure_mutator_registered();
    crate::mutator::push_root(slot);
    // Dijkstra: shade new roots during incremental full mark without taking
    // the heap Mutex solely to read the flag (see `full_marking_fast`).
    if crate::gc::full_marking_fast() {
        unsafe {
            shade_payload(*slot);
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_root_pop() {
    crate::mutator::pop_root();
}

///
/// # Safety
/// `obj` is null or a valid heap payload; `new_ptr` follows write-barrier contracts.
#[no_mangle]
pub unsafe extern "C" fn lumia_write_barrier(obj: *mut u8, field: u32, new_ptr: *mut u8) {
    MarkSweep.write_barrier(obj, field, new_ptr);
}

#[no_mangle]
pub extern "C" fn lumia_gc_collect() {
    MarkSweep.collect();
}
