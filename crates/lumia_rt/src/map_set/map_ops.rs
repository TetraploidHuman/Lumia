//! Map collection C ABI operations.
//!
//! # Safety (FFI)
//! Map payloads are null or valid Map/overlay layouts. Callers must not pass
//! dangling or type-confused pointers; returned pointers are young-heap owned
//! unless documented as immortal (e.g. None singleton).

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use std::ptr;

use crate::common::{header_from_payload, GcInhibitGuard, TYPE_LIST, TYPE_LIST_IOTA};
use crate::gc::{list_payload_bytes, lumia_alloc};
use crate::list::force_heap_list;

use super::map_core::{
    alloc_adt, alloc_adt_none_immortal, map_alloc_hash_tid, map_alloc_overlay, map_find,
    map_from_linear_to_hash, map_hash_find_slot, map_hash_nbytes, map_hash_put_new, map_is_hash,
    map_is_overlay, map_linear_nbytes, map_lookup_val, map_materialize, map_overlay_dn,
    map_overlay_parent, map_pair_at, MAP_OVERLAY_MAX, MAP_SMALL_MAX,
};
use super::tid::{key_eq, map_float_keys, map_float_vals, map_is_assoc, map_tid};

/// # Safety
/// `map` is null or a valid Map/overlay payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_map_contains(map: *mut u8, key: i64) -> i64 {
    unsafe {
        if map_lookup_val(map, key).is_some() {
            1
        } else {
            0
        }
    }
}

/// Missing key → None ADT; hit → Some(value). Tags come from the program's `Option` decl.
/// Misses reuse an immortal per-tag None singleton (no young-heap traffic).
///
/// # Safety
/// `map` is null or a valid Map/overlay payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_map_get(map: *mut u8, key: i64, some_tag: i64, none_tag: i64) -> *mut u8 {
    unsafe {
        match map_lookup_val(map, key) {
            Some(val) => {
                let opt = alloc_adt(some_tag, &[val]);
                // Show/eq read ADT `_pad` float mask. Codegen AllocAdt sets it;
                // RT-built Option[Float] must too (else `println(m.get(k))` prints IEEE bits).
                if map_float_vals(map) {
                    crate::show::lumia_adt_set_float_mask(opt, 0b1);
                }
                opt
            }
            None => alloc_adt_none_immortal(none_tag),
        }
    }
}

/// # Safety
/// `map` is null or a valid Map/overlay payload; returned map is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_map_set(map: *mut u8, key: i64, val: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        if map_is_overlay(map) {
            let parent = map_overlay_parent(map);
            let dn = map_overlay_dn(map);
            let base = map as *const i64;
            if let Some(i) = {
                let float_keys = map_float_keys(parent) || map_float_keys(map);
                (0..dn as usize).rev().find(|&i| key_eq(*base.add(3 + i * 2), key, float_keys))
            } {
                // Replace existing delta key in a new overlay (stack pair buf).
                let mut pairs = [(0i64, 0i64); MAP_OVERLAY_MAX as usize];
                debug_assert!(dn as usize <= pairs.len());
                for (j, slot) in pairs.iter_mut().enumerate().take(dn as usize) {
                    let k = *base.add(3 + j * 2);
                    let v = if j == i { val } else { *base.add(4 + j * 2) };
                    *slot = (k, v);
                }
                return map_alloc_overlay(parent, &pairs[..dn as usize]);
            }
            if dn < MAP_OVERLAY_MAX {
                let mut pairs = [(0i64, 0i64); (MAP_OVERLAY_MAX as usize) + 1];
                for (j, slot) in pairs.iter_mut().enumerate().take(dn as usize) {
                    *slot = (*base.add(3 + j * 2), *base.add(4 + j * 2));
                }
                pairs[dn as usize] = (key, val);
                return map_alloc_overlay(parent, &pairs[..=dn as usize]);
            }
            // Delta full → flatten then upsert.
            let flat = map_materialize(map);
            return lumia_map_set(flat, key, val);
        }
        if map.is_null() || !map_is_hash(map) {
            let (n, base) = if map.is_null() {
                (0i64, ptr::null())
            } else {
                (*(map as *const i64), map as *const i64)
            };
            if let Some(i) = map_find(map, key) {
                let nbytes = map_linear_nbytes(n) as u64;
                let dest = lumia_alloc(nbytes, map_tid(map));
                let dst = dest as *mut i64;
                *dst = n;
                for j in 0..(n as usize * 2) {
                    *dst.add(1 + j) = *base.add(1 + j);
                }
                *dst.add(2 + i * 2) = val;
                return dest;
            }
            let n2 = n + 1;
            if n2 > MAP_SMALL_MAX && !map_is_assoc(map) {
                return map_from_linear_to_hash(map, Some((key, val)));
            }
            let nbytes = map_linear_nbytes(n2) as u64;
            let dest = lumia_alloc(nbytes, map_tid(map));
            let dst = dest as *mut i64;
            *dst = n2;
            for j in 0..(n as usize * 2) {
                *dst.add(1 + j) = *base.add(1 + j);
            }
            *dst.add(1 + n as usize * 2) = key;
            *dst.add(2 + n as usize * 2) = val;
            return dest;
        }
        // HashOrdered → Overlay (avoid full table clone on each set).
        map_alloc_overlay(map, &[(key, val)])
    }
}
/// # Safety
/// `map` is null or a valid Map/overlay payload; returned map is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_map_remove(map: *mut u8, key: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let map = if map_is_overlay(map) {
            map_materialize(map)
        } else {
            map
        };
        let tid = map_tid(map);
        // Empty Map is null (same as `mapOf()`); never allocate a count-0 heap object.
        if map.is_null() {
            return std::ptr::null_mut();
        }
        if map_is_hash(map) {
            let base = map as *const i64;
            let n = *base;
            let cap = *base.add(1) as usize;
            let Some(slot) = map_hash_find_slot(map, key) else {
                let nbytes = map_hash_nbytes(cap) as u64;
                let dest = lumia_alloc(nbytes, tid);
                ptr::copy_nonoverlapping(map, dest, nbytes as usize);
                return dest;
            };
            let n2 = n - 1;
            if n2 == 0 {
                return std::ptr::null_mut();
            }
            if n2 <= MAP_SMALL_MAX {
                // Demote to linear
                let nbytes = map_linear_nbytes(n2) as u64;
                let dest = lumia_alloc(nbytes, lumia_abi::tid_without_hash(tid));
                let dst = dest as *mut i64;
                *dst = n2;
                let mut w = 0usize;
                for i in 0..n as usize {
                    let s = *base.add(2 + i) as usize;
                    if s == slot {
                        continue;
                    }
                    let cell = base.add(2 + cap + s * 3);
                    *dst.add(1 + w * 2) = *cell;
                    *dst.add(2 + w * 2) = *cell.add(1);
                    w += 1;
                }
                return dest;
            }
            let dest = map_alloc_hash_tid(cap, n2, tid);
            let mut w = 0usize;
            for i in 0..n as usize {
                let s = *base.add(2 + i) as usize;
                if s == slot {
                    continue;
                }
                let cell = base.add(2 + cap + s * 3);
                map_hash_put_new(dest, *cell, *cell.add(1), w);
                w += 1;
            }
            return dest;
        }

        let n = *(map as *const i64);
        let base = map as *const i64;
        let Some(idx) = map_find(map, key) else {
            let nbytes = map_linear_nbytes(n) as u64;
            let dest = lumia_alloc(nbytes, tid);
            ptr::copy_nonoverlapping(map, dest, nbytes as usize);
            return dest;
        };
        let n2 = n - 1;
        if n2 == 0 {
            return std::ptr::null_mut();
        }
        let nbytes = map_linear_nbytes(n2) as u64;
        let dest = lumia_alloc(nbytes, tid);
        let dst = dest as *mut i64;
        *dst = n2;
        let mut w = 0usize;
        for j in 0..n as usize {
            if j == idx {
                continue;
            }
            *dst.add(1 + w * 2) = *base.add(1 + j * 2);
            *dst.add(2 + w * 2) = *base.add(2 + j * 2);
            w += 1;
        }
        dest
    }
}

/// If `map` is a linear table larger than [`MAP_SMALL_MAX`], promote to HashOrdered.
/// Also compact duplicate keys in-place via [`key_eq`] (Float ±0 and Int/String/…).
///
/// # Safety
/// `map` is null or a valid Map payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_map_finish(map: *mut u8) -> *mut u8 {
    // Literal build may call finish before the linear table is rooted; inhibit
    // while promoting so alloc inside `map_from_linear_to_hash` cannot collect it.
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let skip = !map.is_null()
            && (map_is_overlay(map) || map_is_hash(map) || map_is_assoc(map));
        super::finish_linear_container(
            map,
            skip,
            MAP_SMALL_MAX,
            if map.is_null() {
                false
            } else {
                map_float_keys(map)
            },
            |p, fk| compact_linear_map_keys(p, fk),
            |p| map_from_linear_to_hash(p, None),
        )
    }
}

/// In-place dedupe of a linear map (last value wins).
unsafe fn compact_linear_map_keys(map: *mut u8, float_keys: bool) {
    // Map linear: `[n][k0][v0]…` — stride 2, last-wins.
    super::compact_linear_entries(map, float_keys, 2, true);
}

/// Keys in insertion order as HeapList.
///
/// # Safety
/// `map` is null or a valid Map/overlay payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_map_keys(map: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let map = if map_is_overlay(map) {
            map_materialize(map)
        } else {
            map
        };
        let n = if map.is_null() {
            0i64
        } else {
            *(map as *const i64)
        };
        let nbytes = list_payload_bytes(n);
        let dest_tid = lumia_abi::list_type_id(map_float_keys(map));
        let dest = lumia_alloc(nbytes, dest_tid);
        let dst = dest as *mut i64;
        *dst = n;
        if !map.is_null() {
            for i in 0..n as usize {
                let (k, _) = map_pair_at(map, i);
                *dst.add(1 + i) = k;
            }
        }
        dest
    }
}
/// # Safety
/// `map` is null or a valid Map/overlay payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_map_values(map: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let map = if map_is_overlay(map) {
            map_materialize(map)
        } else {
            map
        };
        let n = if map.is_null() {
            0i64
        } else {
            *(map as *const i64)
        };
        let nbytes = list_payload_bytes(n);
        let dest_tid = lumia_abi::list_type_id(super::tid::map_float_vals(map));
        let dest = lumia_alloc(nbytes, dest_tid);
        let dst = dest as *mut i64;
        *dst = n;
        if !map.is_null() {
            for i in 0..n as usize {
                let (_, v) = map_pair_at(map, i);
                *dst.add(1 + i) = v;
            }
        }
        dest
    }
}

/// Insertion-ordered list of `(k, v)` pairs (each pair is ADT tag0 + 2 fields).
/// Also accepts an existing `List` of pairs (identity) so `for (k,v) in pairs` works.
///
/// # Safety
/// `map` is null, a valid Map/overlay, or a List-of-pairs payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_map_items(map: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if !map.is_null() {
        let tid = unsafe { (*header_from_payload(map)).type_id };
        if lumia_abi::tid_base(tid) == TYPE_LIST {
            return map;
        }
        if tid == TYPE_LIST_IOTA {
            return force_heap_list(map);
        }
    }
    unsafe {
        let map = if map_is_overlay(map) {
            map_materialize(map)
        } else {
            map
        };
        let n = if map.is_null() {
            0i64
        } else {
            *(map as *const i64)
        };
        let nbytes = list_payload_bytes(n);
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        let dst = dest as *mut i64;
        *dst = n;
        if !map.is_null() {
            let mut pair_fmask = 0u64;
            if map_float_keys(map) {
                pair_fmask |= 0b1;
            }
            if map_float_vals(map) {
                pair_fmask |= 0b10;
            }
            for i in 0..n as usize {
                let (k, v) = map_pair_at(map, i);
                let pair = alloc_adt(0, &[k, v]);
                // Nested list/`append_show_adt` uses pair `_pad`; without this,
                // `println(m.items())` prints float key/val as IEEE bit ints.
                if pair_fmask != 0 {
                    crate::show::lumia_adt_set_float_mask(pair, pair_fmask);
                }
                *dst.add(1 + i) = pair as i64;
            }
        }
        dest
    }
}
