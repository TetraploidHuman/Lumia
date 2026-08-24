//! Shared empty-container Float ensure skeleton (`ensure_*_f64`).

use crate::common::{header_from_payload, trap_abort};
use crate::gc::lumi_alloc;

/// Allocate an empty container payload (`len = 0`) with the given packed `type_id`.
#[inline]
pub(crate) fn alloc_empty_container(tid: u32) -> *mut u8 {
    let dest = lumi_alloc(8, tid);
    unsafe {
        *(dest as *mut i64) = 0;
    }
    dest
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
