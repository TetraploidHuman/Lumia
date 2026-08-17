//! Map layout, lookup, hash table internals, marking, and equality.

use std::ptr;

use crate::common::{
    float_key_eq, header_from_payload, is_heap_payload_bits, trap_abort, GcInhibitGuard,
};
use crate::eq::lumia_eq;
use crate::gc::{list_payload_bytes, lumia_alloc, mark_on, mark_value_on};
use crate::heap::Heap;

use super::tid::{key_eq, map_float_keys, map_float_vals, map_is_assoc, map_tid};

/// Map: small maps stay linear `[n][k0][v0]…`; larger use HashOrdered
/// `[n][cap][order×cap][key,val,state × cap]` (DESIGN default path).
/// Hash writes may produce Overlay: `[-1][parent][dn][k0][v0]…` (delta ≤ [`SMALL_CONTAINER_MAX`]).
pub(crate) const MAP_SMALL_MAX: i64 = lumia_abi::SMALL_CONTAINER_MAX as i64;
pub(crate) const MAP_OVERLAY_MARK: i64 = -1;
pub(crate) const MAP_OVERLAY_MAX: i64 = lumia_abi::SMALL_CONTAINER_MAX as i64;
pub(crate) const MAP_ST_EMPTY: i64 = super::OPEN_HASH_ST_EMPTY;
#[allow(dead_code)] // reserved for delete/tomb paths; claim uses OPEN_HASH_* directly
pub(crate) const MAP_ST_FULL: i64 = super::OPEN_HASH_ST_FULL;
#[allow(dead_code)]
pub(crate) const MAP_ST_TOMB: i64 = super::OPEN_HASH_ST_TOMB;

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
    unsafe { lumia_abi::tid_hash((*header_from_payload(map)).type_id) }
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
            // Overlay may sit on a Float-keyed parent — use key_eq (±0), not bit `lumia_eq`.
            let float_keys = map_float_keys(map) || map_float_keys(parent);
            let mut n = map_count(parent);
            for i in 0..dn {
                let k = *base.add(3 + i * 2);
                // Count as new if not in parent and not earlier in delta.
                let mut seen = false;
                for j in 0..i {
                    if key_eq(*base.add(3 + j * 2), k, float_keys) {
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
        let parent = map_overlay_parent(map);
        let float_keys = map_float_keys(map) || map_float_keys(parent);
        for i in (0..dn).rev() {
            if key_eq(*base.add(3 + i * 2), key, float_keys) {
                return Some(*base.add(4 + i * 2));
            }
        }
        return map_lookup_val(parent, key);
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
            let out = map_alloc_hash_tid(cap, 0, map_tid(parent));
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
        let out = lumia_alloc(nbytes, map_tid(parent));
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
        dest
    }
}

pub(crate) unsafe fn map_alloc_overlay(parent: *mut u8, pairs: &[(i64, i64)]) -> *mut u8 {
    let dn = pairs.len() as i64;
    let nbytes = map_overlay_nbytes(dn) as u64;
    let dest = lumia_alloc(nbytes, map_tid(parent));
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
pub(crate) fn map_mark_payload(
    h: &mut Heap,
    payload: *mut u8,
    size: usize,
    float_keys: bool,
    float_vals: bool,
) {
    unsafe {
        let base = payload as *const i64;
        let n0 = *base;
        if n0 == MAP_OVERLAY_MARK {
            let parent = map_overlay_parent(payload);
            // Parent word is a payload ptr; filter immediates before mark lock.
            if is_heap_payload_bits(parent as i64) {
                mark_on(h, header_from_payload(parent));
            }
            let dn0 = map_overlay_dn(payload);
            // Overlay pairs start at word 3; clamp to declared payload size.
            let max_pairs = size.saturating_sub(3 * 8) / 16;
            let dn = if dn0 > 0 {
                (dn0 as usize).min(max_pairs)
            } else {
                0
            };
            for i in 0..dn {
                if !float_keys {
                    mark_value_on(h, *base.add(3 + i * 2));
                }
                if !float_vals {
                    mark_value_on(h, *base.add(4 + i * 2));
                }
            }
            return;
        }
        let n = n0;
        if !lumia_abi::tid_hash((*header_from_payload(payload)).type_id) {
            for i in 0..n as usize {
                if !float_keys {
                    mark_value_on(h, *base.add(1 + i * 2));
                }
                if !float_vals {
                    mark_value_on(h, *base.add(2 + i * 2));
                }
            }
            return;
        }
        // HashOrdered — clamp n/cap/slot to payload so corrupt headers cannot OOB.
        if n0 <= 0 {
            return;
        }
        let n = n0 as usize;
        let cap = *base.add(1);
        if cap <= 0 {
            return;
        }
        let cap = cap as usize;
        // Layout: [n][cap][order×cap][cells×cap×3] — words after the two headers.
        let words = size / 8;
        if words < 2 + cap + cap * 3 {
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
            let cell = base.add(2 + cap + slot * 3);
            if !float_keys {
                mark_value_on(h, *cell);
            }
            if !float_vals {
                mark_value_on(h, *cell.add(1));
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
    // Map cell: (key, val, state) — stride 3, state at +2.
    super::open_hash_find_slot(base, cap, key, float_keys, 3, 2)
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
    // First hit wins (linear maps do not store duplicates after set).
    (0..n as usize).find(|&i| key_eq(*base.add(1 + i * 2), key, float_keys))
}

/// Allocate an ADT with Show-kind packed into `type_id` and optional field masks.
///
/// `show_kind` `0` → anonymous `#tag`; `≥ 1` indexes the ADT Show registry.
/// Masks are applied after fields are live (same sanitize as codegen AllocAdt).
/// Callers that need zero meta pass `show_kind=0`, `float_mask=0`, `bool_mask=0`.
pub(crate) fn alloc_adt_with_meta(
    tag: i64,
    fields: &[i64],
    show_kind: u16,
    float_mask: u64,
    bool_mask: u64,
) -> *mut u8 {
    let nbytes = list_payload_bytes(fields.len() as i64);
    let dest = lumia_alloc(nbytes, lumia_abi::adt_type_id(show_kind));
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
    if float_mask != 0 {
        // SAFETY: `dest` is a fresh ADT payload with fields already written.
        unsafe { crate::show::lumia_adt_set_float_mask(dest, float_mask) };
    }
    if bool_mask != 0 {
        // SAFETY: same as float mask.
        unsafe { crate::show::lumia_adt_set_bool_mask(dest, bool_mask) };
    }
    dest
}

/// Immortal nullary ADT for `None` (map_get miss). Tagged like a heap Option;
/// `RC_SHARED` so retain/release are no-ops. One singleton per `(none_tag, show_kind)`.
pub(crate) fn alloc_adt_none_immortal(tag: i64, show_kind: u16) -> *mut u8 {
    use crate::common::{header_from_payload, header_layout, payload_ptr, RC_SHARED};
    use crate::gc::{init_alloc_header, insert_young};
    use crate::heap::with_heap;
    use std::alloc::alloc;

    // Pack kind into the cache key so a wrong anonymous tid is never reused.
    let cache_key = (tag as u64) | ((show_kind as u64) << 32);
    with_heap(|h| {
        if let Some(&p) = h.option_none.get(&cache_key) {
            return p;
        }
        let nbytes = list_payload_bytes(0) as usize;
        let dest = unsafe {
            let layout = header_layout(nbytes);
            let mem = alloc(layout);
            if mem.is_null() {
                trap_abort("lumia: out of memory");
            }
            let header = init_alloc_header(mem, nbytes, lumia_abi::adt_type_id(show_kind));
            insert_young(h, header, nbytes);
            payload_ptr(header)
        };
        unsafe {
            *(dest as *mut i64) = tag;
            (*header_from_payload(dest)).rc = RC_SHARED;
            (*header_from_payload(dest))._pad = 0;
        }
        h.perm.push(dest);
        h.option_none.insert(cache_key, dest);
        dest
    })
}

pub(crate) unsafe fn map_alloc_hash_tid(cap: usize, count: i64, tid: u32) -> *mut u8 {
    let nbytes = map_hash_nbytes(cap) as u64;
    let dest = lumia_alloc(nbytes, lumia_abi::tid_with_hash(tid));
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
    // Map cell: (key, val, state) — stride 3, state at +2.
    let (idx, cell) = super::open_hash_claim_slot_or_trap(
        base,
        cap,
        key,
        float_keys,
        3,
        2,
        "lumia: map hash full",
    );
    *cell.add(1) = val;
    if !float_keys {
        unsafe { crate::lumia_write_barrier(dest, order_i as u32, key as *mut u8) };
    }
    if !map_float_vals(dest) {
        unsafe { crate::lumia_write_barrier(dest, order_i as u32, val as *mut u8) };
    }
    *base.add(2 + order_i) = idx as i64;
}

/// Insert or replace during hash-table build. Returns true if a new key was added.
pub(crate) unsafe fn map_hash_upsert_build(dest: *mut u8, key: i64, val: i64) -> bool {
    if let Some(slot) = map_hash_find_slot(dest, key) {
        let base = dest as *mut i64;
        let cap = *base.add(1) as usize;
        let cell = base.add(2 + cap + slot * 3);
        *cell.add(1) = val; // last wins
        if !map_float_vals(dest) {
            unsafe { crate::lumia_write_barrier(dest, slot as u32, val as *mut u8) };
        }
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
    let tid = map_tid(src);
    let dest = super::open_hash_from_linear(
        src,
        usize::from(extra_key.is_some()),
        |cap| map_alloc_hash_tid(cap, 0, tid), // count filled by upserts
        |dest, i| {
            let base = src as *const i64;
            map_hash_upsert_build(dest, *base.add(1 + i * 2), *base.add(2 + i * 2));
        },
    );
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
    let dest = map_alloc_hash_tid(new_cap, n2, map_tid(src));
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
