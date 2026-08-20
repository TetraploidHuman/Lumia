//! Shared Overlay header for Map/Set persistent updates.
//!
//! Layout: `[-1][parent][dn][entry×dn…]`
//! - Map: each entry is 2 words `(k, v)`
//! - Set: each entry is 1 word `(e)`
//!
//! Delta length is capped at [`OVERLAY_MAX`] (`SMALL_CONTAINER_MAX`).
//! Byte/parent/dn/clamp primitives live in [`crate::container_delta`] (also used
//! by List patch, which keeps `word0 = len` instead of [`OVERLAY_MARK`]).

use crate::container_delta::{
    delta_dn, delta_len_clamped, delta_nbytes, delta_parent, mark_delta_parent,
    write_delta_parent_dn,
};
use crate::gc::lumia_alloc;
use crate::heap::Heap;

/// Overlay tag in word 0 (linear/hash use non-negative counts).
pub(crate) const OVERLAY_MARK: i64 = -1;
/// Max delta entries before materialize (same budget as small linear tables).
pub(crate) const OVERLAY_MAX: i64 = lumia_abi::SMALL_CONTAINER_MAX as i64;

/// Compatibility aliases (Map/Set call sites / evacuate).
pub(crate) const MAP_OVERLAY_MARK: i64 = OVERLAY_MARK;
pub(crate) const SET_OVERLAY_MARK: i64 = OVERLAY_MARK;
pub(crate) const MAP_OVERLAY_MAX: i64 = OVERLAY_MAX;
pub(crate) const SET_OVERLAY_MAX: i64 = OVERLAY_MAX;

/// Bytes for `[-1][parent][dn]` + `dn * words_per_entry` payload words.
#[inline]
pub(crate) fn overlay_nbytes(dn: i64, words_per_entry: usize, kind: &str) -> usize {
    delta_nbytes(dn, words_per_entry, kind)
}

pub(crate) fn is_overlay(payload: *mut u8) -> bool {
    if payload.is_null() {
        return false;
    }
    unsafe { *(payload as *const i64) == OVERLAY_MARK }
}

/// # Safety
/// `payload` is a non-null Overlay object.
#[inline]
pub(crate) unsafe fn overlay_parent(payload: *mut u8) -> *mut u8 {
    delta_parent(payload)
}

/// # Safety
/// `payload` is a non-null Overlay object.
#[inline]
pub(crate) unsafe fn overlay_dn(payload: *mut u8) -> i64 {
    delta_dn(payload)
}

/// Write `[-1][parent][dn]` at the start of an overlay allocation.
///
/// # Safety
/// `dst` points at a writable buffer of at least 3 `i64` words.
pub(crate) unsafe fn write_overlay_header(dst: *mut i64, parent: *mut u8, dn: i64) {
    *dst = OVERLAY_MARK;
    write_delta_parent_dn(dst, parent, dn);
}

/// Allocate Overlay shell and write the 3-word header; caller fills entries.
///
/// Capacity is always [`OVERLAY_MAX`] entries so a unique overlay can append
/// in place without reallocating. Declared `dn` is the used length.
///
/// # Safety
/// `parent` is null or a valid Map/Set payload; `tid` matches the collection.
pub(crate) unsafe fn alloc_overlay_shell(
    parent: *mut u8,
    dn: i64,
    words_per_entry: usize,
    tid: u32,
    kind: &str,
) -> *mut u8 {
    let nbytes = overlay_nbytes(OVERLAY_MAX, words_per_entry, kind) as u64;
    let dest = lumia_alloc(nbytes, tid);
    write_overlay_header(dest as *mut i64, parent, dn);
    dest
}

/// How many delta entries fit in this overlay's allocated payload.
///
/// # Safety
/// `payload` is a non-null Overlay object.
#[inline]
pub(crate) unsafe fn overlay_entry_capacity(payload: *mut u8, words_per_entry: usize) -> usize {
    crate::container_delta::delta_entry_capacity(payload, words_per_entry)
}

/// Clamp declared `dn` to the payload size for GC mark / evacuate.
///
/// # Safety
/// `payload` is a non-null Overlay object of `size` bytes.
#[inline]
pub(crate) unsafe fn overlay_delta_len(
    payload: *mut u8,
    size: usize,
    words_per_entry: usize,
) -> usize {
    delta_len_clamped(payload, size, words_per_entry)
}

/// Compact overlay entries in place, dropping those for which `drop_at(i)` is true.
///
/// Reads entry `i` before any write to slot `i`, so `drop_at` may inspect the
/// original i-th entry. Returns the new `dn`.
///
/// # Safety
/// `payload` is a non-null Overlay; `words_per_entry` matches the collection.
pub(crate) unsafe fn overlay_compact_entries(
    payload: *mut u8,
    words_per_entry: usize,
    mut drop_at: impl FnMut(usize) -> bool,
) -> i64 {
    let dn = overlay_dn(payload) as usize;
    let dst = payload as *mut i64;
    let mut w = 0usize;
    for i in 0..dn {
        if drop_at(i) {
            continue;
        }
        if w != i {
            std::ptr::copy_nonoverlapping(
                dst.add(3 + i * words_per_entry),
                dst.add(3 + w * words_per_entry),
                words_per_entry,
            );
        }
        w += 1;
    }
    *dst.add(2) = w as i64;
    w as i64
}

/// Mark the Overlay parent pointer (word 1) if it is a heap payload.
///
/// # Safety
/// `payload` is a non-null Overlay object.
#[inline]
pub(crate) unsafe fn mark_overlay_parent(h: &mut Heap, payload: *mut u8) {
    mark_delta_parent(h, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nbytes_map_vs_set_stride() {
        assert_eq!(overlay_nbytes(0, 2, "map"), 24);
        assert_eq!(overlay_nbytes(1, 2, "map"), 40); // 3+2
        assert_eq!(overlay_nbytes(1, 1, "set"), 32); // 3+1
        assert_eq!(overlay_nbytes(2, 1, "set"), 40);
    }

    #[test]
    fn alloc_capacity_is_overlay_max() {
        assert_eq!(
            overlay_nbytes(OVERLAY_MAX, 2, "map") / 8,
            3 + OVERLAY_MAX as usize * 2
        );
        assert_eq!(
            overlay_nbytes(OVERLAY_MAX, 1, "set") / 8,
            3 + OVERLAY_MAX as usize
        );
    }

    #[test]
    fn null_is_not_overlay() {
        assert!(!is_overlay(std::ptr::null_mut()));
    }

    #[test]
    fn overlay_mark_required() {
        let mut buf = [0i64; 3];
        assert!(!is_overlay(buf.as_mut_ptr() as *mut u8));
        buf[0] = OVERLAY_MARK;
        assert!(is_overlay(buf.as_mut_ptr() as *mut u8));
    }

    #[test]
    fn compact_drops_matching_map_entries() {
        let mut buf = [OVERLAY_MARK, 0, 3, 1, 10, 2, 20, 1, 11];
        let p = buf.as_mut_ptr();
        unsafe {
            let n2 = overlay_compact_entries(p as *mut u8, 2, |i| *p.add(3 + i * 2) == 1);
            assert_eq!(n2, 1);
            assert_eq!(buf[2], 1);
            assert_eq!(&buf[3..5], &[2, 20]);
        }
    }
}
