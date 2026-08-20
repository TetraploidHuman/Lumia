//! Set layout and C ABI operations.
//!
//! # Safety (FFI)
//! Set payloads are null or valid Set layouts (linear, HashOrdered, or Overlay).
//! Callers must not pass dangling or type-confused pointers; returned pointers
//! are young-heap owned unless null.
//!
//! Hash inserts may produce Overlay: `[-1][parent][dn][e0]…` (delta ≤
//! [`SMALL_CONTAINER_MAX`]), matching Map's persistent-update path.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use std::ptr;

use crate::common::{header_from_payload, map_rc_is_unique, trap_abort, GcInhibitGuard, TYPE_SET};
use crate::ensure::immortal_empty_container;
use crate::gc::{list_payload_bytes, lumia_alloc, mark_value_on};
use crate::heap::Heap;

use super::map_core::linear_grow_cap;
use super::overlay::{
    alloc_overlay_shell, is_overlay, mark_overlay_parent, overlay_compact_entries,
    overlay_delta_len, overlay_dn, overlay_entry_capacity, overlay_parent, SET_OVERLAY_MARK,
    SET_OVERLAY_MAX,
};
use super::tid::{key_eq, set_float_elems, set_is_assoc, set_tid};

/// Shared empty `Set` (`setOf()` / remove-to-empty). Immortal — survives GC.
/// Null is still accepted as empty by ops (compat); prefer this for nesting Show.
#[no_mangle]
pub extern "C" fn lumia_set_empty() -> *mut u8 {
    immortal_empty_container(|h| h.empty_set, |h, p| h.empty_set = p, TYPE_SET)
}

/// Set: small stays linear `[n][e0]…`; larger HashOrdered
/// `[n][cap][order×cap][elem,state × cap]`.
pub(crate) const SET_SMALL_MAX: i64 = lumia_abi::SMALL_CONTAINER_MAX as i64;
pub(crate) const SET_ST_EMPTY: i64 = super::OPEN_HASH_ST_EMPTY;
#[allow(dead_code)] // reserved for delete/tomb paths; claim uses OPEN_HASH_* directly
pub(crate) const SET_ST_FULL: i64 = super::OPEN_HASH_ST_FULL;
#[allow(dead_code)]
pub(crate) const SET_ST_TOMB: i64 = super::OPEN_HASH_ST_TOMB;

pub(crate) fn set_linear_nbytes(n: i64) -> usize {
    list_payload_bytes(n) as usize
}

/// Element slots in a linear set payload (`[n][e0]…`).
///
/// # Safety
/// `set` is a non-null linear Set (not hash/overlay).
pub(crate) unsafe fn set_linear_elem_capacity(set: *mut u8) -> i64 {
    let nbytes = (*header_from_payload(set)).size as i64;
    nbytes / 8 - 1
}

pub(crate) fn set_hash_nbytes(cap: usize) -> usize {
    // [n][cap] + order[cap] + (elem,state)[cap]
    cap.checked_mul(3)
        .and_then(|w| w.checked_add(2))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumia: set hash table too large (cap={cap})")))
}

pub(crate) fn set_is_overlay(set: *mut u8) -> bool {
    is_overlay(set)
}

pub(crate) fn set_is_hash(set: *mut u8) -> bool {
    if set.is_null() || set_is_overlay(set) {
        return false;
    }
    unsafe { lumia_abi::tid_hash((*header_from_payload(set)).type_id) }
}

pub(crate) unsafe fn set_overlay_parent(set: *mut u8) -> *mut u8 {
    overlay_parent(set)
}

pub(crate) unsafe fn set_overlay_dn(set: *mut u8) -> i64 {
    overlay_dn(set)
}

/// Logical element count (insertion-unique).
pub(crate) fn set_count(set: *mut u8) -> i64 {
    if set.is_null() {
        return 0;
    }
    unsafe {
        if set_is_overlay(set) {
            let parent = set_overlay_parent(set);
            let dn = set_overlay_dn(set) as usize;
            let base = set as *const i64;
            let float_elems = set_float_elems(set) || set_float_elems(parent);
            let mut n = set_count(parent);
            for i in 0..dn {
                let e = *base.add(3 + i);
                let mut seen = false;
                for j in 0..i {
                    if key_eq(*base.add(3 + j), e, float_elems) {
                        seen = true;
                        break;
                    }
                }
                if seen {
                    continue;
                }
                if lumia_set_contains(parent, e) == 0 {
                    n += 1;
                }
            }
            n
        } else {
            *(set as *const i64)
        }
    }
}

pub(crate) fn set_mark_payload(h: &mut Heap, payload: *mut u8, size: usize, float_elems: bool) {
    unsafe {
        let base = payload as *const i64;
        let n0 = *base;
        if n0 == SET_OVERLAY_MARK {
            mark_overlay_parent(h, payload);
            if float_elems {
                return;
            }
            let dn = overlay_delta_len(payload, size, 1);
            for i in 0..dn {
                mark_value_on(h, *base.add(3 + i));
            }
            return;
        }
        // Unboxed Float elems are never heap pointers (same as TYPE_LIST_F64).
        if float_elems {
            return;
        }
        if !lumia_abi::tid_hash((*header_from_payload(payload)).type_id) {
            if n0 > 0 {
                let max = size.saturating_sub(8) / 8;
                let n = (n0 as usize).min(max);
                for i in 0..n {
                    mark_value_on(h, *base.add(1 + i));
                }
            }
            return;
        }
        // HashOrdered — clamp like map_mark_payload.
        if n0 <= 0 {
            return;
        }
        let n = n0 as usize;
        let meta = *base.add(1);
        let cap = super::open_hash_cap(meta);
        let start = super::open_hash_order_start(meta);
        if cap == 0 {
            return;
        }
        // Layout: [n][meta][order×cap][cells×cap×2].
        let words = size / 8;
        if words < 2 + cap + cap * 2 {
            return;
        }
        let max_n = n.min(cap.saturating_sub(start)).min(words.saturating_sub(2 + cap));
        let order = base.add(2);
        for i in 0..max_n {
            let slot = *order.add(start + i);
            if slot < 0 {
                continue;
            }
            let slot = slot as usize;
            if slot >= cap {
                continue;
            }
            let cell = base.add(2 + cap + slot * 2);
            mark_value_on(h, *cell);
        }
    }
}

pub(crate) fn set_eq(a: *mut u8, b: *mut u8) -> i64 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let a = if set_is_overlay(a) {
            set_materialize(a)
        } else {
            a
        };
        let b = if set_is_overlay(b) {
            set_materialize(b)
        } else {
            b
        };
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
    let _gc = GcInhibitGuard::enter();
    let set = if set_is_overlay(set) {
        set_materialize(set)
    } else {
        set
    };
    let base = set as *const i64;
    if set_is_hash(set) {
        let meta = *base.add(1);
        let cap = super::open_hash_cap(meta);
        let start = super::open_hash_order_start(meta);
        let slot = *base.add(2 + start + i) as usize;
        *base.add(2 + cap + slot * 2)
    } else {
        *base.add(1 + i)
    }
}

pub(crate) unsafe fn set_hash_find_slot(set: *mut u8, elem: i64) -> Option<usize> {
    let float_elems = set_float_elems(set);
    let base = set as *const i64;
    let cap = super::open_hash_cap(*base.add(1));
    // Set cell: (elem, state) — stride 2, state at +1.
    super::open_hash_find_slot(base, cap, elem, float_elems, 2, 1)
}

/// If `set` is a linear table larger than [`SET_SMALL_MAX`], promote to HashOrdered.
/// Also compact duplicate elems in-place via [`key_eq`] (Float ±0 and Int/String/…).
///
/// # Safety
/// `set` is null or a valid Set payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_set_finish(set: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let set = if set_is_overlay(set) {
            set_materialize(set)
        } else {
            set
        };
        let skip = !set.is_null() && (set_is_hash(set) || set_is_assoc(set));
        super::finish_linear_container(
            set,
            skip,
            SET_SMALL_MAX,
            if set.is_null() {
                false
            } else {
                set_float_elems(set)
            },
            |p, fk| compact_linear_set_elems(p, fk),
            |p| set_from_linear_to_hash(p, None),
        )
    }
}

unsafe fn compact_linear_set_elems(set: *mut u8, float_elems: bool) {
    // Set linear: `[n][e0]…` — stride 1, keep-first.
    super::compact_linear_entries(set, float_elems, 1, false);
}

/// # Safety
/// `set` is null or a valid Set payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_set_contains(set: *mut u8, elem: i64) -> i64 {
    if set.is_null() {
        return 0;
    }
    unsafe {
        if set_is_overlay(set) {
            let dn = set_overlay_dn(set) as usize;
            let base = set as *const i64;
            let parent = set_overlay_parent(set);
            let float_elems = set_float_elems(set) || set_float_elems(parent);
            for i in (0..dn).rev() {
                if key_eq(*base.add(3 + i), elem, float_elems) {
                    return 1;
                }
            }
            return lumia_set_contains(parent, elem);
        }
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
    let dest = lumia_alloc(set_hash_nbytes(cap) as u64, lumia_abi::tid_with_hash(tid));
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

pub(crate) unsafe fn set_alloc_overlay(parent: *mut u8, elems: &[i64]) -> *mut u8 {
    let dn = elems.len() as i64;
    let dest = alloc_overlay_shell(parent, dn, 1, set_tid(parent), "set");
    let dst = dest as *mut i64;
    for (i, e) in elems.iter().enumerate() {
        *dst.add(3 + i) = *e;
    }
    dest
}

/// Flatten overlay (and nested overlays) into HashOrdered or linear.
pub(crate) unsafe fn set_materialize(set: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if set.is_null() || !set_is_overlay(set) {
        return set;
    }
    let parent = set_materialize(set_overlay_parent(set));
    let dn = set_overlay_dn(set) as usize;
    let base = set as *const i64;
    let parent_n = set_count(parent) as usize;
    // Worst-case size if every delta is new (dups are skipped by insert_build).
    let total = parent_n + dn;
    if set_is_hash(parent) || total as i64 > SET_SMALL_MAX {
        let mut cap = if set_is_hash(parent) {
            *(parent as *const i64).add(1) as usize
        } else {
            16
        };
        while total * 2 > cap {
            cap = cap.saturating_mul(2).max(16);
        }
        let dest = set_alloc_hash_tid(cap, 0, set_tid(parent));
        for i in 0..parent_n {
            set_hash_put_new(dest, set_elem_at(parent, i), i);
        }
        *(dest as *mut i64) = parent_n as i64;
        for i in 0..dn {
            set_hash_insert_build(dest, *base.add(3 + i));
        }
        dest
    } else {
        let mut dest = {
            let nbytes = set_linear_nbytes(parent_n as i64) as u64;
            let out = lumia_alloc(nbytes, set_tid(parent));
            if !parent.is_null() {
                ptr::copy_nonoverlapping(parent, out, nbytes as usize);
            } else {
                *(out as *mut i64) = 0;
            }
            out
        };
        for i in 0..dn {
            let e = *base.add(3 + i);
            dest = set_clone_insert_no_overlay(dest, e);
        }
        dest
    }
}

/// Insert without creating a new overlay (used while materializing).
unsafe fn set_clone_insert_no_overlay(set: *mut u8, elem: i64) -> *mut u8 {
    if lumia_set_contains(set, elem) != 0 {
        return set;
    }
    let tid = set_tid(set);
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
        let dest = lumia_alloc(nbytes, tid);
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
    let base = set as *const i64;
    let n = *base;
    let cap = super::open_hash_cap(*base.add(1));
    let n2 = n + 1;
    let need_grow = (n2 as usize * 2) > cap;
    let new_cap = if need_grow { cap * 2 } else { cap };
    let dest = set_alloc_hash_tid(new_cap, n2, tid);
    for i in 0..n as usize {
        set_hash_put_new(dest, set_elem_at(set, i), i);
    }
    set_hash_put_new(dest, elem, n as usize);
    dest
}

unsafe fn set_hash_compact_order_window(set: *mut u8) {
    let base = set as *mut i64;
    let n = *base as usize;
    let meta = *base.add(1);
    let cap = super::open_hash_cap(meta);
    let start = super::open_hash_order_start(meta);
    if start == 0 || start + n < cap {
        return;
    }
    let order = base.add(2);
    ptr::copy(order.add(start), order, n);
    for i in n..(start + n).min(cap) {
        *order.add(i) = -1;
    }
    *base.add(1) = super::open_hash_pack_meta(cap, 0);
}

pub(crate) unsafe fn set_hash_put_new(dest: *mut u8, elem: i64, order_i: usize) {
    let float_elems = set_float_elems(dest);
    let base = dest as *mut i64;
    let meta = *base.add(1);
    let cap = super::open_hash_cap(meta);
    let start = super::open_hash_order_start(meta);
    // Set cell: (elem, state) — stride 2, state at +1.
    let (idx, _cell) = super::open_hash_claim_slot_or_trap(
        base,
        cap,
        elem,
        float_elems,
        2,
        1,
        "lumia: set hash full",
    );
    if !float_elems {
        unsafe { crate::lumia_write_barrier(dest, order_i as u32, elem as *mut u8) };
    }
    *base.add(2 + start + order_i) = idx as i64;
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
    let tid = set_tid(src);
    let dest = super::open_hash_from_linear(
        src,
        usize::from(extra.is_some()),
        |cap| set_alloc_hash_tid(cap, 0, tid),
        |dest, i| {
            let base = src as *const i64;
            set_hash_insert_build(dest, *base.add(1 + i));
        },
    );
    if let Some(e) = extra {
        set_hash_insert_build(dest, e);
    }
    dest
}

/// Immutable insert: new Set with `elem` (identity if already present).
///
/// # Safety
/// `set` is null or a valid Set payload; returned set is newly allocated (or null).
#[no_mangle]
pub unsafe extern "C" fn lumia_set_insert(set: *mut u8, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let tid = set_tid(set);
        if lumia_set_contains(set, elem) != 0 {
            // Persistent identity: inserting an existing element is a no-op.
            if set.is_null() {
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                return dest;
            }
            return set;
        }
        if set_is_overlay(set) {
            let parent = set_overlay_parent(set);
            let dn = set_overlay_dn(set);
            let base = set as *const i64;
            if dn < SET_OVERLAY_MAX {
                if map_rc_is_unique(set) && (dn as usize) < overlay_entry_capacity(set, 1) {
                    let dst = set as *mut i64;
                    *dst.add(3 + dn as usize) = elem;
                    *dst.add(2) = dn + 1;
                    return set;
                }
                let mut elems = [0i64; (SET_OVERLAY_MAX as usize) + 1];
                for (j, slot) in elems.iter_mut().enumerate().take(dn as usize) {
                    *slot = *base.add(3 + j);
                }
                elems[dn as usize] = elem;
                return set_alloc_overlay(parent, &elems[..=dn as usize]);
            }
            let flat = set_materialize(set);
            return lumia_set_insert(flat, elem);
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
            let unique = (set.is_null() || map_rc_is_unique(set)) && !set_is_assoc(set);
            if unique && !set.is_null() && set_linear_elem_capacity(set) >= n2 {
                let dst = set as *mut i64;
                *dst = n2;
                *dst.add(1 + n as usize) = elem;
                if !set_float_elems(set) {
                    crate::lumia_write_barrier(set, (1 + n) as u32, elem as *mut u8);
                }
                return set;
            }
            let cap = if unique {
                linear_grow_cap(n2, SET_SMALL_MAX)
            } else {
                n2
            };
            let nbytes = set_linear_nbytes(cap) as u64;
            let dest = lumia_alloc(nbytes, tid);
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
        // Unique HashOrdered: insert in place, or grow+rehash once when full.
        // Spilling unique-but-full to Overlay forces materialize's multi-rehash path
        // on every subsequent insert (disastrous for large unique builders).
        if map_rc_is_unique(set) {
            let base = set as *mut i64;
            let n = *base;
            let cap = super::open_hash_cap(*base.add(1));
            if (n as usize + 1) * 2 <= cap {
                set_hash_compact_order_window(set);
                set_hash_put_new(set, elem, n as usize);
                *base = n + 1;
                return set;
            }
            return set_clone_insert_no_overlay(set, elem);
        }
        // Shared HashOrdered → Overlay (avoid full table clone on persist).
        set_alloc_overlay(set, &[elem])
    }
}

/// Drop an overlay-only elem (not present on parent) without flattening.
unsafe fn set_overlay_remove_delta_only(set: *mut u8, elem: i64) -> Option<*mut u8> {
    if !set_is_overlay(set) {
        return None;
    }
    let parent = set_overlay_parent(set);
    let dn = set_overlay_dn(set) as usize;
    let base = set as *const i64;
    let float_elems = set_float_elems(set) || set_float_elems(parent);
    let hit = (0..dn)
        .rev()
        .any(|i| key_eq(*base.add(3 + i), elem, float_elems));
    if !hit || lumia_set_contains(parent, elem) != 0 {
        return None;
    }
    if map_rc_is_unique(set) {
        let n2 = overlay_compact_entries(set, 1, |i| key_eq(*base.add(3 + i), elem, float_elems));
        return Some(if n2 == 0 { parent } else { set });
    }
    let mut elems = [0i64; SET_OVERLAY_MAX as usize];
    let mut w = 0usize;
    for i in 0..dn {
        let e = *base.add(3 + i);
        if key_eq(e, elem, float_elems) {
            continue;
        }
        elems[w] = e;
        w += 1;
    }
    Some(if w == 0 {
        parent
    } else {
        set_alloc_overlay(parent, &elems[..w])
    })
}

/// Drop element if present; returns new Set (insertion order of remaining elems).
///
/// # Safety
/// `set` is null or a valid Set payload; returned set is newly allocated or null.
#[no_mangle]
pub unsafe extern "C" fn lumia_set_remove(set: *mut u8, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        if lumia_set_contains(set, elem) == 0 {
            return if set.is_null() {
                lumia_set_empty()
            } else {
                set
            };
        }
        if let Some(out) = set_overlay_remove_delta_only(set, elem) {
            return out;
        }
        let set = if set_is_overlay(set) {
            set_materialize(set)
        } else {
            set
        };
        let tid = set_tid(set);
        // Empty Set is the immortal singleton (null still means empty for compat).
        if set.is_null() {
            return lumia_set_empty();
        }
        if set_is_hash(set) {
            let base = set as *const i64;
            let n = *base;
            let meta = *base.add(1);
            let cap = super::open_hash_cap(meta);
            let start = super::open_hash_order_start(meta);
            let Some(slot) = set_hash_find_slot(set, elem) else {
                return set;
            };
            let n2 = n - 1;
            if n2 == 0 {
                return crate::ensure::empty_container_preserving_tags(
                    tid,
                    TYPE_SET,
                    lumia_set_empty,
                );
            }
            if map_rc_is_unique(set) {
                if n2 > SET_SMALL_MAX {
                    super::open_hash_remove_slot(set as *mut i64, slot, n, 2, 1);
                } else {
                    super::open_hash_demote_linear_in_place(
                        set,
                        slot,
                        n,
                        cap,
                        start,
                        2,
                        1,
                        lumia_abi::tid_without_hash(tid),
                    );
                }
                return set;
            }
            if n2 <= SET_SMALL_MAX {
                let dest = lumia_alloc(
                    set_linear_nbytes(n2) as u64,
                    lumia_abi::tid_without_hash(tid),
                );
                let dst = dest as *mut i64;
                *dst = n2;
                let mut w = 0usize;
                for i in 0..n as usize {
                    let s = *base.add(2 + start + i) as usize;
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
                let s = *base.add(2 + start + i) as usize;
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
            return set;
        };
        let n2 = n - 1;
        if n2 == 0 {
            return crate::ensure::empty_container_preserving_tags(tid, TYPE_SET, lumia_set_empty);
        }
        if map_rc_is_unique(set) {
            let dst = set as *mut i64;
            for j in idx + 1..n as usize {
                *dst.add(1 + (j - 1)) = *dst.add(1 + j);
            }
            *dst = n2;
            return set;
        }
        let dest = lumia_alloc(set_linear_nbytes(n2) as u64, tid);
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

unsafe fn set_flatten(set: *mut u8) -> *mut u8 {
    if set_is_overlay(set) {
        set_materialize(set)
    } else {
        set
    }
}

fn set_hash_cap_for(n: usize) -> usize {
    let mut cap = 16usize;
    while n * 2 > cap {
        cap = cap.saturating_mul(2).max(16);
    }
    cap
}

unsafe fn set_alloc_sized(tid: u32, n: usize) -> *mut u8 {
    if n as i64 <= SET_SMALL_MAX {
        let dest = lumia_alloc(set_linear_nbytes(n as i64) as u64, tid);
        *(dest as *mut i64) = 0;
        dest
    } else {
        set_alloc_hash_tid(set_hash_cap_for(n), 0, tid)
    }
}

unsafe fn set_put_all_new(dest: *mut u8, src: *mut u8) {
    let n = set_count(src) as usize;
    if set_is_hash(dest) {
        let base = dest as *mut i64;
        let start = *base as usize;
        for i in 0..n {
            set_hash_put_new(dest, set_elem_at(src, i), start + i);
        }
        *base = (start + n) as i64;
    } else {
        let base = dest as *mut i64;
        let start = *base as usize;
        for i in 0..n {
            *base.add(1 + start + i) = set_elem_at(src, i);
        }
        *base = (start + n) as i64;
    }
}

unsafe fn set_push_new(dest: *mut u8, elem: i64) {
    if set_is_hash(dest) {
        set_hash_insert_build(dest, elem);
        return;
    }
    if lumia_set_contains(dest, elem) != 0 {
        return;
    }
    let base = dest as *mut i64;
    let n = *base;
    *base.add(1 + n as usize) = elem;
    *base = n + 1;
}

/// Immutable union: elements of `a` then `b` (insertion order; `b` dups skipped).
///
/// # Safety
/// `a`/`b` null or valid Set payloads.
#[no_mangle]
pub unsafe extern "C" fn lumia_set_union(a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let a = set_flatten(a);
    let b = set_flatten(b);
    let na = set_count(a);
    let nb = set_count(b);
    if nb == 0 {
        return if a.is_null() {
            lumia_set_empty()
        } else {
            a
        };
    }
    if na == 0 {
        return b;
    }
    let tid = set_tid(a);
    let dest = set_alloc_sized(tid, (na + nb) as usize);
    set_put_all_new(dest, a);
    for i in 0..nb as usize {
        set_push_new(dest, set_elem_at(b, i));
    }
    dest
}

/// Immutable intersection: elems of `a` that are in `b` (order of `a`).
///
/// # Safety
/// `a`/`b` null or valid Set payloads.
#[no_mangle]
pub unsafe extern "C" fn lumia_set_intersect(a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let a = set_flatten(a);
    let b = set_flatten(b);
    let na = set_count(a);
    let nb = set_count(b);
    if na == 0 || nb == 0 {
        return lumia_set_empty();
    }
    let tid = set_tid(a);
    let dest = set_alloc_sized(tid, na.min(nb) as usize);
    for i in 0..na as usize {
        let e = set_elem_at(a, i);
        if lumia_set_contains(b, e) != 0 {
            set_push_new(dest, e);
        }
    }
    if set_count(dest) == 0 {
        lumia_set_empty()
    } else {
        dest
    }
}

/// Immutable difference: elems of `a` not in `b` (order of `a`).
///
/// # Safety
/// `a`/`b` null or valid Set payloads.
#[no_mangle]
pub unsafe extern "C" fn lumia_set_diff(a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let a = set_flatten(a);
    let b = set_flatten(b);
    let na = set_count(a);
    let nb = set_count(b);
    if na == 0 {
        return lumia_set_empty();
    }
    if nb == 0 {
        return a;
    }
    let tid = set_tid(a);
    let dest = set_alloc_sized(tid, na as usize);
    for i in 0..na as usize {
        let e = set_elem_at(a, i);
        if lumia_set_contains(b, e) == 0 {
            set_push_new(dest, e);
        }
    }
    if set_count(dest) == 0 {
        lumia_set_empty()
    } else {
        dest
    }
}
