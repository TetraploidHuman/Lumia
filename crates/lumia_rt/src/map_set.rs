//! Map and Set collections.

use std::ptr;

use crate::common::{
    float_key_eq, float_key_hash, header_from_payload, is_heap_payload, trap_abort, GcInhibitGuard,
    TYPE_ADT, TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA, TYPE_MAP, TYPE_MAP_ASSOC,
    TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V, TYPE_MAP_ASSOC_VF64, TYPE_MAP_F64, TYPE_MAP_F64V,
    TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC, TYPE_SET_F64,
};
use crate::gc::{list_payload_bytes, lumia_alloc, mark, mark_value};
use crate::list::force_heap_list;
use crate::show_eq::{lumia_eq, lumia_hash};

pub(crate) fn map_is_assoc(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    matches!(
        unsafe { (*header_from_payload(map)).type_id },
        TYPE_MAP_ASSOC | TYPE_MAP_ASSOC_VF64 | TYPE_MAP_ASSOC_F64 | TYPE_MAP_ASSOC_F64V
    )
}

pub(crate) fn set_is_assoc(set: *mut u8) -> bool {
    if set.is_null() {
        return false;
    }
    unsafe { (*header_from_payload(set)).type_id == TYPE_SET_ASSOC }
}

#[inline]
pub(crate) fn is_map_tid(tid: u32) -> bool {
    matches!(
        tid,
        TYPE_MAP
            | TYPE_MAP_F64
            | TYPE_MAP_ASSOC
            | TYPE_MAP_VF64
            | TYPE_MAP_F64V
            | TYPE_MAP_ASSOC_VF64
            | TYPE_MAP_ASSOC_F64
            | TYPE_MAP_ASSOC_F64V
    )
}

pub(crate) fn map_float_keys(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    matches!(
        unsafe { (*header_from_payload(map)).type_id },
        TYPE_MAP_F64 | TYPE_MAP_F64V | TYPE_MAP_ASSOC_F64 | TYPE_MAP_ASSOC_F64V
    )
}

pub(crate) fn map_float_vals(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    matches!(
        unsafe { (*header_from_payload(map)).type_id },
        TYPE_MAP_VF64 | TYPE_MAP_F64V | TYPE_MAP_ASSOC_VF64 | TYPE_MAP_ASSOC_F64V
    )
}

pub(crate) fn map_tid_with_flags(float_keys: bool, float_vals: bool) -> u32 {
    match (float_keys, float_vals) {
        (true, true) => TYPE_MAP_F64V,
        (true, false) => TYPE_MAP_F64,
        (false, true) => TYPE_MAP_VF64,
        (false, false) => TYPE_MAP,
    }
}

pub(crate) fn map_assoc_tid_with_flags(float_keys: bool, float_vals: bool) -> u32 {
    match (float_keys, float_vals) {
        (true, true) => TYPE_MAP_ASSOC_F64V,
        (true, false) => TYPE_MAP_ASSOC_F64,
        (false, true) => TYPE_MAP_ASSOC_VF64,
        (false, false) => TYPE_MAP_ASSOC,
    }
}

pub(crate) fn set_float_elems(set: *mut u8) -> bool {
    if set.is_null() {
        return false;
    }
    unsafe { (*header_from_payload(set)).type_id == TYPE_SET_F64 }
}

pub(crate) fn key_eq(a: i64, b: i64, float_keys: bool) -> bool {
    if float_keys {
        float_key_eq(a, b)
    } else {
        lumia_eq(a, b) != 0
    }
}

pub(crate) fn key_hash(key: i64, float_keys: bool) -> u64 {
    if float_keys {
        float_key_hash(key)
    } else {
        lumia_hash(key)
    }
}

/// Ensure a map uses Float-key IEEE eq/hash.
/// Empty maps may be retagged (fresh alloc); non-empty wrong key sort traps.
pub(crate) fn ensure_map_f64(map: *mut u8) -> *mut u8 {
    if map.is_null() {
        let dest = lumia_alloc(8, TYPE_MAP_F64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(map);
        match (*h).type_id {
            TYPE_MAP_F64 | TYPE_MAP_F64V | TYPE_MAP_ASSOC_F64 | TYPE_MAP_ASSOC_F64V => map,
            TYPE_MAP | TYPE_MAP_VF64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_f64 on non-empty Int-key map");
                }
                let tid = map_tid_with_flags(true, (*h).type_id == TYPE_MAP_VF64);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            TYPE_MAP_ASSOC | TYPE_MAP_ASSOC_VF64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_f64 on non-empty Int-key assoc map");
                }
                let tid = map_assoc_tid_with_flags(true, (*h).type_id == TYPE_MAP_ASSOC_VF64);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            other => trap_abort(&format!("lumia: ensure_map_f64 on type_id={other}")),
        }
    }
}

/// Ensure a map uses IEEE equality for Float values.
pub(crate) fn ensure_map_vf64(map: *mut u8) -> *mut u8 {
    if map.is_null() {
        let dest = lumia_alloc(8, TYPE_MAP_VF64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(map);
        match (*h).type_id {
            TYPE_MAP_VF64 | TYPE_MAP_F64V | TYPE_MAP_ASSOC_VF64 | TYPE_MAP_ASSOC_F64V => map,
            TYPE_MAP | TYPE_MAP_F64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_vf64 on non-empty non-Float-value map");
                }
                let tid = map_tid_with_flags((*h).type_id == TYPE_MAP_F64, true);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            TYPE_MAP_ASSOC | TYPE_MAP_ASSOC_F64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_vf64 on non-empty non-Float-value assoc map");
                }
                let tid = map_assoc_tid_with_flags((*h).type_id == TYPE_MAP_ASSOC_F64, true);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            other => trap_abort(&format!("lumia: ensure_map_vf64 on type_id={other}")),
        }
    }
}

pub(crate) fn ensure_set_f64(set: *mut u8) -> *mut u8 {
    if set.is_null() {
        let dest = lumia_alloc(8, TYPE_SET_F64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(set);
        match (*h).type_id {
            TYPE_SET_F64 => set,
            TYPE_SET => {
                if *(set as *const i64) != 0 {
                    trap_abort("lumia: ensure_set_f64 on non-empty Int-elem set");
                }
                let dest = lumia_alloc(8, TYPE_SET_F64);
                *(dest as *mut i64) = 0;
                dest
            }
            other => trap_abort(&format!("lumia: ensure_set_f64 on type_id={other}")),
        }
    }
}

pub(crate) fn map_type_id(map: *mut u8) -> u32 {
    if map.is_null() {
        TYPE_MAP
    } else {
        unsafe { (*header_from_payload(map)).type_id }
    }
}

pub(crate) fn set_type_id(set: *mut u8) -> u32 {
    if set.is_null() {
        TYPE_SET
    } else {
        unsafe { (*header_from_payload(set)).type_id }
    }
}

#[no_mangle]
pub extern "C" fn lumia_ensure_map_f64(map: *mut u8) -> *mut u8 {
    ensure_map_f64(map)
}

#[no_mangle]
pub extern "C" fn lumia_ensure_map_vf64(map: *mut u8) -> *mut u8 {
    ensure_map_vf64(map)
}

#[no_mangle]
pub extern "C" fn lumia_ensure_set_f64(set: *mut u8) -> *mut u8 {
    ensure_set_f64(set)
}
pub(crate) fn is_set_tid(tid: u32) -> bool {
    matches!(tid, TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC)
}
/// Map: small maps stay linear `[n][k0][v0]…`; larger use HashOrdered
/// `[n][cap][order×cap][key,val,state × cap]` (DESIGN default path).
/// Hash writes may produce Overlay: `[-1][parent][dn][k0][v0]…` (delta ≤ 8).
pub(crate) const MAP_SMALL_MAX: i64 = 8;
pub(crate) const MAP_OVERLAY_MARK: i64 = -1;
pub(crate) const MAP_OVERLAY_MAX: i64 = 8;
pub(crate) const MAP_ST_EMPTY: i64 = 0;
pub(crate) const MAP_ST_FULL: i64 = 1;
pub(crate) const MAP_ST_TOMB: i64 = 2;

pub(crate) fn map_linear_nbytes(n: i64) -> usize {
    if n < 0 {
        trap_abort("lumia: negative map length");
    }
    (n as u64)
        .checked_mul(2)
        .and_then(|pairs| pairs.checked_add(1))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as u64)
        .map(|b| b as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumia: map too large (n={n})")))
}

pub(crate) fn map_hash_nbytes(cap: usize) -> usize {
    // [count][cap] + order[cap] + (key,val,state)[cap]
    cap.checked_mul(4)
        .and_then(|w| w.checked_add(2))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumia: map hash table too large (cap={cap})")))
}

pub(crate) fn map_overlay_nbytes(dn: i64) -> usize {
    if dn < 0 {
        trap_abort("lumia: negative overlay delta");
    }
    (dn as u64)
        .checked_mul(2)
        .and_then(|kv| kv.checked_add(3))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as u64)
        .map(|b| b as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumia: map overlay too large (dn={dn})")))
}

pub(crate) fn map_is_overlay(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    unsafe { *(map as *const i64) == MAP_OVERLAY_MARK }
}

pub(crate) fn map_is_hash(map: *mut u8) -> bool {
    if map.is_null() || map_is_overlay(map) {
        return false;
    }
    unsafe {
        let n = *(map as *const i64);
        if n < 0 {
            return false;
        }
        (*header_from_payload(map)).size as usize != map_linear_nbytes(n)
    }
}

pub(crate) unsafe fn map_overlay_parent(map: *mut u8) -> *mut u8 {
    *(map as *const i64).add(1) as *mut u8
}

pub(crate) unsafe fn map_overlay_dn(map: *mut u8) -> i64 {
    *(map as *const i64).add(2)
}

/// Logical entry count (insertion-unique keys).
pub(crate) fn map_count(map: *mut u8) -> i64 {
    if map.is_null() {
        return 0;
    }
    unsafe {
        if map_is_overlay(map) {
            let parent = map_overlay_parent(map);
            let dn = map_overlay_dn(map) as usize;
            let base = map as *const i64;
            let mut n = map_count(parent);
            for i in 0..dn {
                let k = *base.add(3 + i * 2);
                // Count as new if not in parent and not earlier in delta.
                let mut seen = false;
                for j in 0..i {
                    if lumia_eq(*base.add(3 + j * 2), k) != 0 {
                        seen = true;
                        break;
                    }
                }
                if seen {
                    continue;
                }
                if map_find(parent, k).is_none() {
                    n += 1;
                }
            }
            n
        } else {
            *(map as *const i64)
        }
    }
}

/// Lookup value through overlay chain then base map.
pub(crate) unsafe fn map_lookup_val(map: *mut u8, key: i64) -> Option<i64> {
    if map.is_null() {
        return None;
    }
    if map_is_overlay(map) {
        let dn = map_overlay_dn(map) as usize;
        let base = map as *const i64;
        for i in (0..dn).rev() {
            if lumia_eq(*base.add(3 + i * 2), key) != 0 {
                return Some(*base.add(4 + i * 2));
            }
        }
        return map_lookup_val(map_overlay_parent(map), key);
    }
    match map_find(map, key) {
        Some(i) if map_is_hash(map) => {
            let base = map as *const i64;
            let cap = *base.add(1) as usize;
            let cell = base.add(2 + cap + i * 3);
            Some(*cell.add(1))
        }
        Some(i) => {
            let base = map as *const i64;
            Some(*base.add(2 + i * 2))
        }
        None => None,
    }
}

/// Flatten overlay (and nested overlays) into a HashOrdered or linear map.
pub(crate) unsafe fn map_materialize(map: *mut u8) -> *mut u8 {
    // Multi-alloc helper: keep intermediates alive across soft-threshold GC.
    let _gc = GcInhibitGuard::enter();
    if map.is_null() || !map_is_overlay(map) {
        return map;
    }
    let parent = map_materialize(map_overlay_parent(map));
    let dn = map_overlay_dn(map) as usize;
    let base = map as *const i64;
    let mut dest = if map_is_hash(parent) || map_count(parent) + dn as i64 > MAP_SMALL_MAX {
        // Start from hash clone of parent
        if map_is_hash(parent) {
            let pbase = parent as *const i64;
            let n = *pbase;
            let cap = *pbase.add(1) as usize;
            let out = map_alloc_hash_tid(cap, 0, map_type_id(parent));
            for i in 0..n as usize {
                let s = *pbase.add(2 + i) as usize;
                let cell = pbase.add(2 + cap + s * 3);
                map_hash_put_new(out, *cell, *cell.add(1), i);
            }
            *(out as *mut i64) = n;
            out
        } else {
            map_from_linear_to_hash(parent, None)
        }
    } else {
        // Stay linear: copy parent then apply deltas via set path below
        let n = map_count(parent);
        let nbytes = map_linear_nbytes(n) as u64;
        let out = lumia_alloc(nbytes, map_type_id(parent));
        ptr::copy_nonoverlapping(parent, out, nbytes as usize);
        out
    };
    for i in 0..dn {
        let k = *base.add(3 + i * 2);
        let v = *base.add(4 + i * 2);
        dest = map_clone_hash_upsert_or_linear(dest, k, v);
    }
    dest
}

pub(crate) unsafe fn map_clone_hash_upsert_or_linear(map: *mut u8, key: i64, val: i64) -> *mut u8 {
    if map_is_hash(map) {
        map_clone_hash_upsert(map, key, val)
    } else {
        // linear upsert (same as lumia_map_set linear branch)
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
        dest
    }
}

pub(crate) unsafe fn map_alloc_overlay(parent: *mut u8, pairs: &[(i64, i64)]) -> *mut u8 {
    let dn = pairs.len() as i64;
    let nbytes = map_overlay_nbytes(dn) as u64;
    let dest = lumia_alloc(nbytes, map_type_id(parent));
    let dst = dest as *mut i64;
    *dst = MAP_OVERLAY_MARK;
    *dst.add(1) = parent as i64;
    *dst.add(2) = dn;
    for (i, (k, v)) in pairs.iter().enumerate() {
        *dst.add(3 + i * 2) = *k;
        *dst.add(4 + i * 2) = *v;
    }
    dest
}
pub(crate) fn map_mark_payload(payload: *mut u8, size: usize, float_keys: bool, float_vals: bool) {
    unsafe {
        let base = payload as *const i64;
        let n0 = *base;
        if n0 == MAP_OVERLAY_MARK {
            let parent = map_overlay_parent(payload);
            if is_heap_payload(parent) {
                mark(header_from_payload(parent));
            }
            let dn = map_overlay_dn(payload) as usize;
            for i in 0..dn {
                if !float_keys {
                    mark_value(*base.add(3 + i * 2));
                }
                if !float_vals {
                    mark_value(*base.add(4 + i * 2));
                }
            }
            return;
        }
        let n = n0;
        if size == map_linear_nbytes(n) {
            for i in 0..n as usize {
                if !float_keys {
                    mark_value(*base.add(1 + i * 2));
                }
                if !float_vals {
                    mark_value(*base.add(2 + i * 2));
                }
            }
            return;
        }
        // HashOrdered
        let cap = *base.add(1) as usize;
        let order = base.add(2);
        for i in 0..n as usize {
            let slot = *order.add(i) as usize;
            let cell = base.add(2 + cap + slot * 3);
            if !float_keys {
                mark_value(*cell);
            }
            if !float_vals {
                mark_value(*cell.add(1));
            }
        }
    }
}

pub(crate) fn map_eq(a: *mut u8, b: *mut u8) -> i64 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let a = if map_is_overlay(a) {
            map_materialize(a)
        } else {
            a
        };
        let b = if map_is_overlay(b) {
            map_materialize(b)
        } else {
            b
        };
        let na = if a.is_null() { 0 } else { *(a as *const i64) };
        let nb = if b.is_null() { 0 } else { *(b as *const i64) };
        if na != nb {
            return 0;
        }
        let float_keys = map_float_keys(a) || map_float_keys(b);
        let float_vals = map_float_vals(a) || map_float_vals(b);
        for i in 0..na as usize {
            let (ka, va) = map_pair_at(a, i);
            let mut found = false;
            for j in 0..nb as usize {
                let (kb, vb) = map_pair_at(b, j);
                let vals_ok = if float_vals {
                    float_key_eq(va, vb)
                } else {
                    lumia_eq(va, vb) != 0
                };
                if key_eq(ka, kb, float_keys) && vals_ok {
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

/// i-th pair in insertion order.
pub(crate) unsafe fn map_pair_at(map: *mut u8, i: usize) -> (i64, i64) {
    let _gc = GcInhibitGuard::enter();
    let map = if map_is_overlay(map) {
        map_materialize(map)
    } else {
        map
    };
    let base = map as *const i64;
    if map_is_hash(map) {
        let cap = *base.add(1) as usize;
        let slot = *base.add(2 + i) as usize;
        let cell = base.add(2 + cap + slot * 3);
        (*cell, *cell.add(1))
    } else {
        (*base.add(1 + i * 2), *base.add(2 + i * 2))
    }
}

pub(crate) unsafe fn map_hash_find_slot(map: *mut u8, key: i64) -> Option<usize> {
    let float_keys = map_float_keys(map);
    let base = map as *const i64;
    let cap = *base.add(1) as usize;
    if cap == 0 {
        return None;
    }
    let mut idx = (key_hash(key, float_keys) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * 3);
        let st = *cell.add(2);
        if st == MAP_ST_EMPTY {
            return None;
        }
        if st == MAP_ST_FULL && key_eq(*cell, key, float_keys) {
            return Some(idx);
        }
        idx = (idx + 1) % cap;
    }
    None
}

/// Map payload helpers — linear or HashOrdered (see above).
pub(crate) unsafe fn map_find(map: *mut u8, key: i64) -> Option<usize> {
    if map.is_null() || map_is_overlay(map) {
        return None;
    }
    if map_is_hash(map) {
        return map_hash_find_slot(map, key);
    }
    let float_keys = map_float_keys(map);
    let n = *(map as *const i64);
    let base = map as *const i64;
    let mut found = None;
    for i in 0..n as usize {
        if key_eq(*base.add(1 + i * 2), key, float_keys) {
            found = Some(i);
        }
    }
    found
}

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

pub(crate) fn alloc_adt(tag: i64, fields: &[i64]) -> *mut u8 {
    let nbytes = list_payload_bytes(fields.len() as i64);
    let dest = lumia_alloc(nbytes, TYPE_ADT);
    if dest.is_null() {
        trap_abort("lumia: adt OOM");
    }
    unsafe {
        let dst = dest as *mut i64;
        *dst = tag;
        for (i, f) in fields.iter().enumerate() {
            *dst.add(1 + i) = *f;
        }
    }
    dest
}

pub(crate) unsafe fn map_alloc_hash_tid(cap: usize, count: i64, tid: u32) -> *mut u8 {
    let nbytes = map_hash_nbytes(cap) as u64;
    let dest = lumia_alloc(nbytes, tid);
    if dest.is_null() {
        trap_abort("lumia: map hash OOM");
    }
    let dst = dest as *mut i64;
    *dst = count;
    *dst.add(1) = cap as i64;
    for i in 0..cap {
        *dst.add(2 + i) = -1;
        let cell = dst.add(2 + cap + i * 3);
        *cell = 0;
        *cell.add(1) = 0;
        *cell.add(2) = MAP_ST_EMPTY;
    }
    dest
}

pub(crate) unsafe fn map_hash_put_new(dest: *mut u8, key: i64, val: i64, order_i: usize) {
    let float_keys = map_float_keys(dest);
    let base = dest as *mut i64;
    let cap = *base.add(1) as usize;
    let mut idx = (key_hash(key, float_keys) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * 3);
        let st = *cell.add(2);
        if st == MAP_ST_EMPTY || st == MAP_ST_TOMB {
            *cell = key;
            *cell.add(1) = val;
            *cell.add(2) = MAP_ST_FULL;
            *base.add(2 + order_i) = idx as i64;
            return;
        }
        idx = (idx + 1) % cap;
    }
    trap_abort("lumia: map hash full");
}

/// Insert or replace during hash-table build. Returns true if a new key was added.
pub(crate) unsafe fn map_hash_upsert_build(dest: *mut u8, key: i64, val: i64) -> bool {
    if let Some(slot) = map_hash_find_slot(dest, key) {
        let base = dest as *mut i64;
        let cap = *base.add(1) as usize;
        let cell = base.add(2 + cap + slot * 3);
        *cell.add(1) = val; // last wins
        return false;
    }
    let base = dest as *mut i64;
    let n = *base as usize;
    map_hash_put_new(dest, key, val, n);
    *base = (n as i64) + 1;
    true
}

pub(crate) unsafe fn map_from_linear_to_hash(
    src: *mut u8,
    extra_key: Option<(i64, i64)>,
) -> *mut u8 {
    let n = if src.is_null() {
        0i64
    } else {
        *(src as *const i64)
    };
    let n2 = n + if extra_key.is_some() { 1 } else { 0 };
    let mut cap = 16usize;
    while (cap as i64) < n2 * 2 {
        cap *= 2;
    }
    let dest = map_alloc_hash_tid(cap, 0, map_type_id(src)); // count filled by upserts
    let base = src as *const i64;
    for i in 0..n as usize {
        let k = *base.add(1 + i * 2);
        let v = *base.add(2 + i * 2);
        map_hash_upsert_build(dest, k, v);
    }
    if let Some((k, v)) = extra_key {
        map_hash_upsert_build(dest, k, v);
    }
    dest
}

pub(crate) unsafe fn map_clone_hash_upsert(src: *mut u8, key: i64, val: i64) -> *mut u8 {
    let base = src as *const i64;
    let n = *base;
    let cap = *base.add(1) as usize;
    let replace = map_hash_find_slot(src, key);
    let n2 = if replace.is_some() { n } else { n + 1 };
    let need_grow = replace.is_none() && (n2 as usize * 2 > cap);
    let new_cap = if need_grow { cap * 2 } else { cap };
    let dest = map_alloc_hash_tid(new_cap, n2, map_type_id(src));
    let mut w = 0usize;
    for i in 0..n as usize {
        let slot = *base.add(2 + i) as usize;
        let cell = base.add(2 + cap + slot * 3);
        let k = *cell;
        let v = if replace == Some(slot) {
            val
        } else {
            *cell.add(1)
        };
        map_hash_put_new(dest, k, v, w);
        w += 1;
    }
    if replace.is_none() {
        map_hash_put_new(dest, key, val, w);
    }
    dest
}

/// Immutable upsert: new Map with `key → val` (overwrite keeps insertion slot).
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
        .unwrap_or_else(|| trap_abort(&format!("lumia: set hash table too large (cap={cap})")))
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
        let n = *base;
        if size == set_linear_nbytes(n) {
            for i in 0..n as usize {
                mark_value(*base.add(1 + i));
            }
            return;
        }
        let cap = *base.add(1) as usize;
        let order = base.add(2);
        for i in 0..n as usize {
            let slot = *order.add(i) as usize;
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
pub extern "C" fn lumia_set_finish(set: *mut u8) -> *mut u8 {
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
pub extern "C" fn lumia_set_contains(set: *mut u8, elem: i64) -> i64 {
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
    let dest = lumia_alloc(set_hash_nbytes(cap) as u64, tid);
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
            *base.add(2 + order_i) = idx as i64;
            return;
        }
        idx = (idx + 1) % cap;
    }
    trap_abort("lumia: set hash full");
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
    let dest = set_alloc_hash_tid(cap, 0, set_type_id(src));
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
#[no_mangle]
pub extern "C" fn lumia_set_insert(set: *mut u8, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let tid = set_type_id(set);
        if lumia_set_contains(set, elem) != 0 {
            if set.is_null() {
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                return dest;
            }
            let nbytes = (*header_from_payload(set)).size as u64;
            let dest = lumia_alloc(nbytes, tid);
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
        // Hash insert
        let base = set as *const i64;
        let n = *base;
        let cap = *base.add(1) as usize;
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
}

/// Drop element if present; returns new Set (insertion order of remaining elems).
#[no_mangle]
pub extern "C" fn lumia_set_remove(set: *mut u8, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let tid = set_type_id(set);
        if set.is_null() {
            let dest = lumia_alloc(8, TYPE_SET);
            *(dest as *mut i64) = 0;
            return dest;
        }
        if set_is_hash(set) {
            let base = set as *const i64;
            let n = *base;
            let cap = *base.add(1) as usize;
            let Some(slot) = set_hash_find_slot(set, elem) else {
                let nbytes = set_hash_nbytes(cap) as u64;
                let dest = lumia_alloc(nbytes, tid);
                ptr::copy_nonoverlapping(set, dest, nbytes as usize);
                return dest;
            };
            let n2 = n - 1;
            if n2 <= SET_SMALL_MAX {
                let dest = lumia_alloc(set_linear_nbytes(n2) as u64, tid);
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
            let dest = lumia_alloc(nbytes, tid);
            ptr::copy_nonoverlapping(set, dest, nbytes as usize);
            return dest;
        };
        let n2 = n - 1;
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
