//! Shared sparse-delta shell for Map/Set Overlay and List patch.
//!
//! Layout: `[word0][parent][dn][entry×dn…]`
//! - Map/Set Overlay: `word0 == -1`
//! - List patch: `word0 == logical len`
//!
//! Entry width differs (`words_per_entry`); header is always 3 `i64` words.

use crate::common::{header_from_payload, is_heap_payload_bits, trap_abort};
use crate::gc::mark_on;
use crate::heap::Heap;

/// Words before the first delta entry (`word0`, `parent`, `dn`).
pub(crate) const DELTA_HEADER_WORDS: usize = 3;
/// Bytes in the fixed header.
pub(crate) const DELTA_HEADER_BYTES: usize = DELTA_HEADER_WORDS * 8;

/// Bytes for header + `dn * words_per_entry` payload words.
pub(crate) fn delta_nbytes(dn: i64, words_per_entry: usize, kind: &str) -> usize {
    if dn < 0 {
        trap_abort(&format!("lumia: negative {kind} delta"));
    }
    if words_per_entry == 0 {
        trap_abort(&format!("lumia: {kind} words_per_entry=0"));
    }
    (dn as u64)
        .checked_mul(words_per_entry as u64)
        .and_then(|entries| entries.checked_add(DELTA_HEADER_WORDS as u64))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as u64)
        .map(|b| b as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumia: {kind} too large (dn={dn})")))
}

/// # Safety
/// `payload` is a non-null delta object (`[word0][parent][dn]…`).
#[inline]
pub(crate) unsafe fn delta_parent(payload: *mut u8) -> *mut u8 {
    *(payload as *const i64).add(1) as *mut u8
}

/// # Safety
/// `payload` is a non-null delta object.
#[inline]
pub(crate) unsafe fn delta_dn(payload: *mut u8) -> i64 {
    *(payload as *const i64).add(2)
}

/// Write `[parent][dn]` at words 1–2 (caller sets word0).
///
/// # Safety
/// `dst` points at a writable buffer of at least 3 `i64` words.
#[inline]
pub(crate) unsafe fn write_delta_parent_dn(dst: *mut i64, parent: *mut u8, dn: i64) {
    *dst.add(1) = parent as i64;
    *dst.add(2) = dn;
}

/// Clamp declared `dn` to the payload size for GC mark / evacuate.
///
/// # Safety
/// `payload` is a non-null delta object of `size` bytes.
pub(crate) unsafe fn delta_len_clamped(
    payload: *mut u8,
    size: usize,
    words_per_entry: usize,
) -> usize {
    let dn0 = delta_dn(payload);
    let max = size.saturating_sub(DELTA_HEADER_BYTES) / (8 * words_per_entry.max(1));
    if dn0 > 0 {
        (dn0 as usize).min(max)
    } else {
        0
    }
}

/// How many delta entries fit in this object's allocated payload.
///
/// # Safety
/// `payload` is a non-null delta object.
#[inline]
pub(crate) unsafe fn delta_entry_capacity(payload: *mut u8, words_per_entry: usize) -> usize {
    let size = (*header_from_payload(payload)).size as usize;
    size.saturating_sub(DELTA_HEADER_BYTES) / (8 * words_per_entry.max(1))
}

/// Mark the parent pointer (word 1) if it is a heap payload.
///
/// # Safety
/// `payload` is a non-null delta object.
pub(crate) unsafe fn mark_delta_parent(h: &mut Heap, payload: *mut u8) {
    let parent = delta_parent(payload);
    if is_heap_payload_bits(parent as i64) {
        mark_on(h, header_from_payload(parent));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nbytes_header_and_stride() {
        assert_eq!(delta_nbytes(0, 2, "map"), 24);
        assert_eq!(delta_nbytes(1, 2, "map"), 40);
        assert_eq!(delta_nbytes(1, 1, "set"), 32);
        assert_eq!(delta_nbytes(2, 2, "list"), 56); // 3+4 words
    }

    #[test]
    fn clamp_respects_size() {
        // 3 header + 1 pair (2 words) = 40 bytes declared capacity for dn=1.
        let mut buf = [0i64; 5];
        buf[2] = 99; // claimed dn
        let payload = buf.as_mut_ptr() as *mut u8;
        unsafe {
            assert_eq!(delta_len_clamped(payload, 40, 2), 1);
            assert_eq!(delta_len_clamped(payload, 24, 2), 0);
        }
    }
}
