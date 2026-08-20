//! Shared `List[Float]` payload views for dense / CN / EFE kernels.

use crate::common::trap_abort;
use crate::list::list_len_of;

#[inline]
pub(crate) fn require_len(list: *mut u8, expect: i64, what: &str) {
    let n = list_len_of(list);
    if n != expect {
        trap_abort(&format!("lumia: {what} len {n} != {expect}"));
    }
}

/// Immutable view of a dense `TYPE_LIST_F64` payload (`[len][f64…]`).
///
/// # Safety
/// `list` must be a non-null heap list whose payload is float elems.
#[inline]
pub(crate) unsafe fn f64_elems(list: *mut u8) -> (*const f64, usize) {
    let n = *(list as *const i64) as usize;
    ((list as *const i64).add(1) as *const f64, n)
}

/// Mutable view of a dense `TYPE_LIST_F64` payload.
///
/// # Safety
/// Same as [`f64_elems`]; caller must own uniqueness for in-place writes.
#[inline]
pub(crate) unsafe fn f64_elems_mut(list: *mut u8) -> (*mut f64, usize) {
    let n = *(list as *const i64) as usize;
    ((list as *mut i64).add(1) as *mut f64, n)
}
