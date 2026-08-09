//! Map collection C ABI operations.

use std::ptr;

use crate::common::{
    header_from_payload, GcInhibitGuard, TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA, TYPE_MAP,
};
use crate::gc::{list_payload_bytes, lumia_alloc};
use crate::list::force_heap_list;

use super::map_core::{
    alloc_adt, map_alloc_hash_tid, map_alloc_overlay, map_find, map_from_linear_to_hash,
    map_hash_find_slot, map_hash_nbytes, map_hash_put_new, map_is_hash, map_is_overlay,
    map_linear_nbytes, map_lookup_val, map_materialize, map_overlay_dn, map_overlay_parent,
    map_pair_at, MAP_OVERLAY_MAX, MAP_SMALL_MAX,
};
use super::tid::{key_eq, map_float_keys, map_is_assoc, map_type_id};

#[no_mangle]
pub extern "C" fn lumia_map_contains(map: *mut u8, key: i64) -> i64 {
    unsafe {
        if map_lookup_val(map, key).is_some() {
            1
        } else {
            0
        }
    }
}

/// Missing key → None ADT; hit → Some(value). Tags come from the program's `Option` decl.
#[no_mangle]
pub extern "C" fn lumia_map_get(map: *mut u8, key: i64, some_tag: i64, none_tag: i64) -> *mut u8 {
    unsafe {
        match map_lookup_val(map, key) {
            Some(val) => alloc_adt(some_tag, &[val]),
            None => alloc_adt(none_tag, &[]),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_map_set(map: *mut u8, key: i64, val: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        if map_is_overlay(map) {
            let parent = map_overlay_parent(map);
            let dn = map_overlay_dn(map);
            let base = map as *const i64;
            // Replace existing delta key in-place in a new overlay copy.
            let float_keys = map_float_keys(parent) || map_float_keys(map);
            for i in (0..dn as usize).rev() {
                if key_eq(*base.add(3 + i * 2), key, float_keys) {
                    let mut pairs = Vec::with_capacity(dn as usize);
                    for j in 0..dn as usize {
                        let k = *base.add(3 + j * 2);
                        let v = if j == i { val } else { *base.add(4 + j * 2) };
                        pairs.push((k, v));
                    }
                    return map_alloc_overlay(parent, &pairs);
                }
            }
            if dn < MAP_OVERLAY_MAX {
                let mut pairs = Vec::with_capacity(dn as usize + 1);
                for j in 0..dn as usize {
                    pairs.push((*base.add(3 + j * 2), *base.add(4 + j * 2)));
                }
                pairs.push((key, val));
                return map_alloc_overlay(parent, &pairs);
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
                let dest = lumia_alloc(nbytes, map_type_id(map));
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
            let dest = lumia_alloc(nbytes, map_type_id(map));
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
#[no_mangle]
pub extern "C" fn lumia_map_remove(map: *mut u8, key: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let map = if map_is_overlay(map) {
            map_materialize(map)
        } else {
            map
        };
        let tid = map_type_id(map);
        if map.is_null() {
            let dest = lumia_alloc(8, TYPE_MAP);
            *(dest as *mut i64) = 0;
            return dest;
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
            if n2 <= MAP_SMALL_MAX {
                // Demote to linear
                let nbytes = map_linear_nbytes(n2) as u64;
                let dest = lumia_alloc(nbytes, tid);
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
#[no_mangle]
pub extern "C" fn lumia_map_finish(map: *mut u8) -> *mut u8 {
    // Literal build may call finish before the linear table is rooted; inhibit
    // while promoting so alloc inside `map_from_linear_to_hash` cannot collect it.
    let _gc = GcInhibitGuard::enter();
    if map.is_null() {
        return map;
    }
    unsafe {
        if map_is_overlay(map) || map_is_hash(map) || map_is_assoc(map) {
            return map;
        }
        let n = *(map as *const i64);
        if n > MAP_SMALL_MAX {
            map_from_linear_to_hash(map, None)
        } else {
            map
        }
    }
}

/// Keys in insertion order as HeapList.
#[no_mangle]
pub extern "C" fn lumia_map_keys(map: *mut u8) -> *mut u8 {
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
        let dest = lumia_alloc(nbytes, TYPE_LIST);
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
#[no_mangle]
pub extern "C" fn lumia_map_values(map: *mut u8) -> *mut u8 {
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
        let dest = lumia_alloc(nbytes, TYPE_LIST);
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
#[no_mangle]
pub extern "C" fn lumia_map_items(map: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if !map.is_null() {
        let tid = unsafe { (*header_from_payload(map)).type_id };
        if tid == TYPE_LIST || tid == TYPE_LIST_F64 {
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
            for i in 0..n as usize {
                let (k, v) = map_pair_at(map, i);
                let pair = alloc_adt(0, &[k, v]);
                *dst.add(1 + i) = pair as i64;
            }
        }
        dest
    }
}
