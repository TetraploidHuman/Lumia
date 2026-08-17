//! Shared open-hash probe for Map/Set HashOrdered tables.
//!
//! Map cells are `(key, val, state)` stride 3; Set cells are `(elem, state)` stride 2.
//! Slot state constants are identical (`EMPTY`/`FULL`/`TOMB`).

use super::tid::{key_eq, key_hash};
use crate::common::trap_abort;

pub(crate) const OPEN_HASH_ST_EMPTY: i64 = 0;
pub(crate) const OPEN_HASH_ST_FULL: i64 = 1;
pub(crate) const OPEN_HASH_ST_TOMB: i64 = 2;

/// Linear-probe find in a HashOrdered table whose cells start at `base + 2 + cap`.
///
/// `cell_stride` is words per cell; `state_off` is the state word within the cell
/// (map: 2, set: 1). Key/elem always lives at word 0 of the cell.
pub(crate) unsafe fn open_hash_find_slot(
    base: *const i64,
    cap: usize,
    key: i64,
    float_keys: bool,
    cell_stride: usize,
    state_off: usize,
) -> Option<usize> {
    if cap == 0 {
        return None;
    }
    let mut idx = (key_hash(key, float_keys) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * cell_stride);
        let st = *cell.add(state_off);
        if st == OPEN_HASH_ST_EMPTY {
            return None;
        }
        if st == OPEN_HASH_ST_FULL && key_eq(*cell, key, float_keys) {
            return Some(idx);
        }
        idx = (idx + 1) % cap;
    }
    None
}

/// Claim an `EMPTY`/`TOMB` slot for a **new** key (does not check for duplicates).
///
/// Writes `key` at cell+0 and `FULL` at `state_off`. Returns `(slot_idx, cell)`.
/// Caller fills any extra cell words (map val) and runs write barriers / order[].
pub(crate) unsafe fn open_hash_claim_slot(
    base: *mut i64,
    cap: usize,
    key: i64,
    float_keys: bool,
    cell_stride: usize,
    state_off: usize,
) -> Option<(usize, *mut i64)> {
    if cap == 0 {
        return None;
    }
    let mut idx = (key_hash(key, float_keys) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * cell_stride);
        let st = *cell.add(state_off);
        if st == OPEN_HASH_ST_EMPTY || st == OPEN_HASH_ST_TOMB {
            *cell = key;
            *cell.add(state_off) = OPEN_HASH_ST_FULL;
            return Some((idx, cell));
        }
        idx = (idx + 1) % cap;
    }
    None
}

/// Like [`open_hash_claim_slot`], but traps with `full_msg` when the table is full.
pub(crate) unsafe fn open_hash_claim_slot_or_trap(
    base: *mut i64,
    cap: usize,
    key: i64,
    float_keys: bool,
    cell_stride: usize,
    state_off: usize,
    full_msg: &str,
) -> (usize, *mut i64) {
    open_hash_claim_slot(base, cap, key, float_keys, cell_stride, state_off)
        .unwrap_or_else(|| trap_abort(full_msg))
}

/// Promote a linear `[n][…]` table to HashOrdered.
///
/// `extra_slots` is added to `n` when sizing capacity (map/set optional insert).
/// `alloc_empty(cap)` allocates a zero-count hash table; `put_linear_at(dest, i)`
/// inserts the `i`-th linear entry (caller owns stride / upsert vs insert-skip).
pub(crate) unsafe fn open_hash_from_linear(
    src: *mut u8,
    extra_slots: usize,
    alloc_empty: impl FnOnce(usize) -> *mut u8,
    mut put_linear_at: impl FnMut(*mut u8, usize),
) -> *mut u8 {
    let n = if src.is_null() {
        0i64
    } else {
        *(src as *const i64)
    };
    let n2 = n + extra_slots as i64;
    let mut cap = 16usize;
    while (cap as i64) < n2 * 2 {
        cap *= 2;
    }
    let dest = alloc_empty(cap);
    for i in 0..n as usize {
        put_linear_at(dest, i);
    }
    dest
}

/// Shared finish skeleton: skip → compact → promote if `n > small_max`.
///
/// Caller owns GC inhibit and the `skip` predicate (hash / overlay / assoc).
pub(crate) unsafe fn finish_linear_container(
    ptr: *mut u8,
    skip: bool,
    small_max: i64,
    float_keys: bool,
    compact: impl FnOnce(*mut u8, bool),
    promote: impl FnOnce(*mut u8) -> *mut u8,
) -> *mut u8 {
    if ptr.is_null() || skip {
        return ptr;
    }
    compact(ptr, float_keys);
    let n = *(ptr as *const i64);
    if n > small_max {
        promote(ptr)
    } else {
        ptr
    }
}

/// In-place dedupe of a linear `[n][entry×stride…]` table.
///
/// Key/elem is word 0 of each entry. When `last_wins`, a duplicate key replaces
/// the earlier entry's remaining words (map val); otherwise the later entry is
/// dropped (set).
pub(crate) unsafe fn compact_linear_entries(
    ptr: *mut u8,
    float_keys: bool,
    stride: usize,
    last_wins: bool,
) {
    debug_assert!(stride >= 1);
    let n = *(ptr as *const i64);
    if n <= 1 {
        return;
    }
    let base = ptr as *mut i64;
    let mut w = 0i64;
    for i in 0..n as usize {
        let key = *base.add(1 + i * stride);
        let mut hit: Option<usize> = None;
        for j in 0..w as usize {
            if key_eq(*base.add(1 + j * stride), key, float_keys) {
                hit = Some(j);
                break;
            }
        }
        match hit {
            Some(j) if last_wins => {
                for o in 1..stride {
                    *base.add(1 + j * stride + o) = *base.add(1 + i * stride + o);
                }
            }
            Some(_) => {}
            None => {
                for o in 0..stride {
                    *base.add(1 + w as usize * stride + o) = *base.add(1 + i * stride + o);
                }
                w += 1;
            }
        }
    }
    *base = w;
}

#[cfg(test)]
#[path = "hash_probe_tests.rs"]
mod tests;
