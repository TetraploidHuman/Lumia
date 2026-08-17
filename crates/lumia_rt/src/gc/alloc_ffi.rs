//! Allocation / root / write-barrier C ABI.
//!
//! # Safety (FFI)
//! `slot` must point at a live root slot; `obj`/`new_ptr` follow write-barrier contracts.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use super::shade::shade_payload;
use super::MarkSweep;
use crate::common::{trap_abort, ObjectHeader, TYPE_ADT, TYPE_LIST};
use crate::heap::Heap;
use lumia_abi::tid_base;

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
    (*header).rc = if matches!(tid_base(type_id), TYPE_LIST | TYPE_ADT) {
        1
    } else {
        0
    };
    (*header)._pad = 0;
    // `marked` published in [`insert_young`].
    header
}

/// Publish a young object into the heap (caller holds [`with_heap`]).
pub(crate) unsafe fn insert_young(h: &mut Heap, header: *mut ObjectHeader, nbytes: usize) {
    (*header).marked = if h.full_marking { 1 } else { 0 };
    h.young.push(header);
    h.heap_set.insert(header);
    h.bytes_young += nbytes;
    h.refresh_alloc_pressure_fast();
}

/// Soft-GC threshold trip (same predicate as the former alloc peek).
#[inline]
pub(crate) fn soft_gc_needed(h: &Heap) -> bool {
    h.gc_inhibit == 0
        && (h.full_marking || h.bytes_young >= h.young_limit || h.bytes_old >= h.old_limit)
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
