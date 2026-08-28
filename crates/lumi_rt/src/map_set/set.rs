//! Set layout and C ABI operations.

use std::ptr;

use crate::common::{header_from_payload, trap_abort, GcInhibitGuard, TYPE_SET};
use crate::gc::{list_payload_bytes, lumi_alloc, mark_value};

use super::tid::{key_eq, key_hash, set_float_elems, set_is_assoc, set_tid};

/// Set: small stays linear `[n][e0]…`; larger HashOrdered
/// `[n][cap][order×cap][elem,state × cap]`.
pub(crate) const SET_SMALL_MAX: i64 = 8;
pub(crate) const SET_ST_EMPTY: i64 = 0;
pub(crate) const SET_ST_FULL: i64 = 1;
pub(crate) const SET_ST_TOMB: i64 = 2;

pub(crate) fn set_linear_nbytes(n: i64) -> usize {
    list_payload_bytes(n) as usize
}

pub(crate) fn set_hash_nbytes(cap: usize) -> usize {
    // [n][cap] + order[cap] + (elem,state)[cap]
    cap.checked_mul(3)
        .and_then(|w| w.checked_add(2))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumi: set hash table too large (cap={cap})")))
}

pub(crate) fn set_is_hash(set: *mut u8) -> bool {
    if set.is_null() {
        return false;
    }
    unsafe {
        let n = *(set as *const i64);
        if n < 0 {
            return false;
        }
        (*header_from_payload(set)).size as usize != set_linear_nbytes(n)
    }
}

pub(crate) fn set_mark_payload(payload: *mut u8, size: usize, float_elems: bool) {
    // Unboxed Float elems are never heap pointers (same as TYPE_LIST_F64).
    if float_elems {
        return;
    }
    unsafe {
        let base = payload as *const i64;
        let n0 = *base;
        if size == set_linear_nbytes(n0) {
            if n0 > 0 {
                let max = size.saturating_sub(8) / 8;
                let n = (n0 as usize).min(max);
                for i in 0..n {
                    mark_value(*base.add(1 + i));
                }
            }
            return;
        }
        // HashOrdered — clamp like map_mark_payload.
        if n0 <= 0 {
            return;
        }
        let n = n0 as usize;
        let cap = *base.add(1);
        if cap <= 0 {
            return;
        }
        let cap = cap as usize;
        // Layout: [n][cap][order×cap][cells×cap×2].
        let words = size / 8;
        if words < 2 + cap + cap * 2 {
            return;
        }
        let max_n = n.min(cap).min(words.saturating_sub(2 + cap));
        let order = base.add(2);
        for i in 0..max_n {
            let slot = *order.add(i);
            if slot < 0 {
                continue;
            }
            let slot = slot as usize;
            if slot >= cap {
                continue;
            }
            let cell = base.add(2 + cap + slot * 2);
            mark_value(*cell);
        }
    }
}

pub(crate) fn set_eq(a: *mut u8, b: *mut u8) -> i64 {
    unsafe {
        let na = if a.is_null() { 0 } else { *(a as *const i64) };
        let nb = if b.is_null() { 0 } else { *(b as *const i64) };
        if na != nb {
            return 0;
        }
        let float_elems = set_float_elems(a) || set_float_elems(b);
        for i in 0..na as usize {
            let ea = set_elem_at(a, i);
            let mut found = false;
            for j in 0..nb as usize {
                if key_eq(ea, set_elem_at(b, j), float_elems) {
                    found = true;
                    break;
                }
            }
            if !found {
                return 0;
            }
        }
        1
    }
}

pub(crate) unsafe fn set_elem_at(set: *mut u8, i: usize) -> i64 {
    let base = set as *const i64;
    if set_is_hash(set) {
        let cap = *base.add(1) as usize;
        let slot = *base.add(2 + i) as usize;
        *base.add(2 + cap + slot * 2)
    } else {
        *base.add(1 + i)
    }
}

pub(crate) unsafe fn set_hash_find_slot(set: *mut u8, elem: i64) -> Option<usize> {
    let float_elems = set_float_elems(set);
    let base = set as *const i64;
    let cap = *base.add(1) as usize;
    if cap == 0 {
        return None;
    }
    let mut idx = (key_hash(elem, float_elems) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * 2);
        let st = *cell.add(1);
        if st == SET_ST_EMPTY {
            return None;
        }
        if st == SET_ST_FULL && key_eq(*cell, elem, float_elems) {
            return Some(idx);
        }
        idx = (idx + 1) % cap;
    }
    None
}

/// If `set` is a linear table larger than [`SET_SMALL_MAX`], promote to HashOrdered.
#[no_mangle]
pub extern "C" fn lumi_set_finish(set: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if set.is_null() {
        return set;
    }
    unsafe {
        if set_is_hash(set) || set_is_assoc(set) {
            return set;
        }
        let n = *(set as *const i64);
        if n > SET_SMALL_MAX {
            set_from_linear_to_hash(set, None)
        } else {
            set
        }
    }
}

#[no_mangle]
pub extern "C" fn lumi_set_contains(set: *mut u8, elem: i64) -> i64 {
    if set.is_null() {
        return 0;
    }
    unsafe {
        if set_is_hash(set) {
            return if set_hash_find_slot(set, elem).is_some() {
                1
            } else {
                0
            };
        }
        let float_elems = set_float_elems(set);
        let n = *(set as *const i64);
        let base = set as *const i64;
        for i in 0..n as usize {
            if key_eq(*base.add(1 + i), elem, float_elems) {
                return 1;
            }
        }
        0
    }
}

pub(crate) unsafe fn set_alloc_hash_tid(cap: usize, count: i64, tid: u32) -> *mut u8 {
    let dest = lumi_alloc(set_hash_nbytes(cap) as u64, tid);
    let dst = dest as *mut i64;
    *dst = count;
    *dst.add(1) = cap as i64;
    for i in 0..cap {
        *dst.add(2 + i) = -1;
        let cell = dst.add(2 + cap + i * 2);
        *cell = 0;
        *cell.add(1) = SET_ST_EMPTY;
    }
    dest
}

pub(crate) unsafe fn set_hash_put_new(dest: *mut u8, elem: i64, order_i: usize) {
    let float_elems = set_float_elems(dest);
    let base = dest as *mut i64;
    let cap = *base.add(1) as usize;
    let mut idx = (key_hash(elem, float_elems) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * 2);
        let st = *cell.add(1);
        if st == SET_ST_EMPTY || st == SET_ST_TOMB {
            *cell = elem;
            *cell.add(1) = SET_ST_FULL;
            if !float_elems {
                crate::lumi_write_barrier(dest, order_i as u32, elem as *mut u8);
            }
            *base.add(2 + order_i) = idx as i64;
            return;
        }
        idx = (idx + 1) % cap;
    }
    trap_abort("lumi: set hash full");
}

/// Insert during hash build; skip if already present. Returns true if newly added.
pub(crate) unsafe fn set_hash_insert_build(dest: *mut u8, elem: i64) -> bool {
    if set_hash_find_slot(dest, elem).is_some() {
        return false;
    }
    let base = dest as *mut i64;
    let n = *base as usize;
    set_hash_put_new(dest, elem, n);
    *base = (n as i64) + 1;
    true
}

pub(crate) unsafe fn set_from_linear_to_hash(src: *mut u8, extra: Option<i64>) -> *mut u8 {
    let n = if src.is_null() {
        0i64
    } else {
        *(src as *const i64)
    };
    let n2 = n + if extra.is_some() { 1 } else { 0 };
    let mut cap = 16usize;
    while (cap as i64) < n2 * 2 {
        cap *= 2;
    }
    let dest = set_alloc_hash_tid(cap, 0, set_tid(src));
    let base = src as *const i64;
    for i in 0..n as usize {
        set_hash_insert_build(dest, *base.add(1 + i));
    }
    if let Some(e) = extra {
        set_hash_insert_build(dest, e);
    }
    dest
}

/// Immutable insert: new Set with `elem` (no-op copy if already present).
/// Unique set → in-place when possible (COW consume `s = s.insert(e)`).
#[no_mangle]
pub extern "C" fn lumi_set_insert(set: *mut u8, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let tid = set_tid(set);
        if lumi_set_contains(set, elem) != 0 {
            // Already present: unique → identity; shared → clone.
            if set.is_null() {
                let dest = lumi_alloc(8, tid);
                *(dest as *mut i64) = 0;
                return dest;
            }
            if crate::common::cow_rc_is_unique(set, false) {
                return set;
            }
            let nbytes = (*header_from_payload(set)).size as u64;
            let dest = lumi_alloc(nbytes, tid);
            ptr::copy_nonoverlapping(set, dest, nbytes as usize);
            return dest;
        }
        if set.is_null() || !set_is_hash(set) {
            let n = if set.is_null() {
                0i64
            } else {
                *(set as *const i64)
            };
            let n2 = n + 1;
            if n2 > SET_SMALL_MAX && !set_is_assoc(set) {
                return set_from_linear_to_hash(set, Some(elem));
            }
            let nbytes = set_linear_nbytes(n2) as u64;
            let dest = lumi_alloc(nbytes, tid);
            let dst = dest as *mut i64;
            *dst = n2;
            if !set.is_null() {
                let src = set as *const i64;
                for i in 0..n as usize {
                    *dst.add(1 + i) = *src.add(1 + i);
                }
            }
            *dst.add(1 + n as usize) = elem;
            return dest;
        }
        // Hash insert
        let base = set as *mut i64;
        let n = *base;
        let cap = *base.add(1) as usize;
        let n2 = n + 1;
        let need_grow = (n2 as usize * 2) > cap;
        if crate::common::cow_rc_is_unique(set, false) && !need_grow {
            set_hash_insert_build(set, elem);
            return set;
        }
        let new_cap = if need_grow { cap * 2 } else { cap };
        let dest = set_alloc_hash_tid(new_cap, n2, tid);
        for i in 0..n as usize {
            set_hash_put_new(dest, set_elem_at(set, i), i);
        }
        set_hash_put_new(dest, elem, n as usize);
        dest
    }
}

/// Drop element if present; returns new Set (insertion order of remaining elems).
#[no_mangle]
pub extern "C" fn lumi_set_remove(set: *mut u8, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let tid = set_tid(set);
        if set.is_null() {
            let dest = lumi_alloc(8, TYPE_SET);
            *(dest as *mut i64) = 0;
            return dest;
        }
        if set_is_hash(set) {
            let base = set as *const i64;
            let n = *base;
            let cap = *base.add(1) as usize;
            let Some(slot) = set_hash_find_slot(set, elem) else {
                let nbytes = set_hash_nbytes(cap) as u64;
                let dest = lumi_alloc(nbytes, tid);
                ptr::copy_nonoverlapping(set, dest, nbytes as usize);
                return dest;
            };
            let n2 = n - 1;
            if n2 <= SET_SMALL_MAX {
                let dest = lumi_alloc(set_linear_nbytes(n2) as u64, tid);
                let dst = dest as *mut i64;
                *dst = n2;
                let mut w = 0usize;
                for i in 0..n as usize {
                    let s = *base.add(2 + i) as usize;
                    if s == slot {
                        continue;
                    }
                    *dst.add(1 + w) = *base.add(2 + cap + s * 2);
                    w += 1;
                }
                return dest;
            }
            let dest = set_alloc_hash_tid(cap, n2, tid);
            let mut w = 0usize;
            for i in 0..n as usize {
                let s = *base.add(2 + i) as usize;
                if s == slot {
                    continue;
                }
                set_hash_put_new(dest, *base.add(2 + cap + s * 2), w);
                w += 1;
            }
            return dest;
        }

        let n = *(set as *const i64);
        let base = set as *const i64;
        let float_elems = set_float_elems(set);
        let mut idx = None;
        for i in 0..n as usize {
            if key_eq(*base.add(1 + i), elem, float_elems) {
                idx = Some(i);
                break;
            }
        }
        let Some(idx) = idx else {
            let nbytes = set_linear_nbytes(n) as u64;
            let dest = lumi_alloc(nbytes, tid);
            ptr::copy_nonoverlapping(set, dest, nbytes as usize);
            return dest;
        };
        let n2 = n - 1;
        let dest = lumi_alloc(set_linear_nbytes(n2) as u64, tid);
        let dst = dest as *mut i64;
        *dst = n2;
        let mut w = 0usize;
        for j in 0..n as usize {
            if j == idx {
                continue;
            }
            *dst.add(1 + w) = *base.add(1 + j);
            w += 1;
        }
        dest
    }
}
