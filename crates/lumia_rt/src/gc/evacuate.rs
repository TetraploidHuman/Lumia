//! Evacuating minor for nursery bump objects.
//!
//! Marked nursery survivors are copied into old (system `alloc`); the nursery
//! header is stamped [`super::nursery::TYPE_NURSERY_FWD`] with `_pad` → new header.
//! Roots / fields are rewritten, then the nursery slab rewinds.

use std::alloc::{alloc, dealloc};
use std::ptr;

use super::nursery::{TYPE_NURSERY_FREE, TYPE_NURSERY_FWD};
use crate::common::{
    adt_float_mask, adt_float_slot, header_from_payload, header_layout, may_be_heap_payload_bits,
    payload_ptr, ObjectHeader, TYPE_ADT, TYPE_CHANNEL, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_IOTA,
    TYPE_MAP, TYPE_SET, TYPE_TASK,
};
use crate::container_delta::delta_len_clamped;
use crate::heap::Heap;
use crate::map_set::{overlay_delta_len, MAP_OVERLAY_MARK, SET_OVERLAY_MARK};
use crate::mutator::for_each_mutator_root;
use lumia_abi::{
    list_elem_skip_gc_mark, map_key_is_float, map_val_is_float, set_elem_is_float, tid_base,
    tid_list_patch,
};

/// True when `payload` bits may legally be passed to [`follow_fwd_payload`].
///
/// Small Int/Bool immediates are often 8-byte aligned (`8`, `16`, …) and pass
/// [`may_be_heap_payload_bits`], but must **not** be dereferenced as headers.
/// Nursery FWD stamps are forgotten from `live_set` before field rewrite, so
/// plain [`crate::common::is_heap_payload`] would miss them — accept FWD by tid.
#[inline]
unsafe fn payload_bits_are_followable(payload: *mut u8) -> bool {
    if payload.is_null() || !may_be_heap_payload_bits(payload as i64) {
        return false;
    }
    let h = header_from_payload(payload);
    if super::nursery::nursery_range_contains_header(h) {
        // SAFETY: `h` is an aligned address inside the process nursery slab
        // (still mapped until [`Nursery::rewind`] after field rewrite).
        let tid = unsafe { (*h).type_id };
        if tid == TYPE_NURSERY_FWD {
            return true;
        }
        if tid == TYPE_NURSERY_FREE || tid == 0 {
            return false;
        }
        return matches!(super::nursery::nursery_probe_live_header(h), Some(true));
    }
    crate::heap::with_heap(|heap| heap.contains_header(h))
}

/// Follow a nursery forwarding stamp (identity if not forwarded).
///
/// Non-heap immediates that sneak past alignment filters are returned unchanged
/// (see [`payload_bits_are_followable`]).
#[inline]
pub(crate) unsafe fn follow_fwd_payload(payload: *mut u8) -> *mut u8 {
    if payload.is_null() {
        return payload;
    }
    if !payload_bits_are_followable(payload) {
        return payload;
    }
    let h = header_from_payload(payload);
    if (*h).type_id != TYPE_NURSERY_FWD {
        return payload;
    }
    let new_h = (*h)._pad as *mut ObjectHeader;
    if new_h.is_null() {
        return payload;
    }
    payload_ptr(new_h)
}

#[inline]
unsafe fn rewrite_i64_slot(slot: *mut i64) {
    let bits = *slot;
    if !may_be_heap_payload_bits(bits) {
        return;
    }
    let p = bits as *mut u8;
    if !payload_bits_are_followable(p) {
        return;
    }
    let q = follow_fwd_payload(p);
    if q != p {
        *slot = q as i64;
    }
}

fn rewrite_fields_on(obj: *mut ObjectHeader) {
    unsafe {
        let payload = payload_ptr(obj);
        let tid = (*obj).type_id;
        match tid_base(tid) {
            TYPE_LIST => {
                if tid_list_patch(tid) {
                    let base = payload as *mut i64;
                    rewrite_i64_slot(base.add(1)); // parent
                    let dn = delta_len_clamped(payload, (*obj).size as usize, 2);
                    for i in 0..dn {
                        rewrite_i64_slot(base.add(4 + i * 2));
                    }
                } else if !list_elem_skip_gc_mark(tid) {
                    let n = *(payload as *const i64);
                    let base = payload as *mut i64;
                    let max_elems = ((*obj).size as usize).saturating_sub(8) / 8;
                    if n > 0 {
                        let n = (n as usize).min(max_elems);
                        for i in 0..n {
                            rewrite_i64_slot(base.add(1 + i));
                        }
                    }
                }
            }
            TYPE_LIST_IOTA => {}
            TYPE_SET => rewrite_set_payload(payload, (*obj).size as usize, set_elem_is_float(tid)),
            TYPE_MAP => rewrite_map_payload(
                payload,
                (*obj).size as usize,
                map_key_is_float(tid),
                map_val_is_float(tid),
            ),
            TYPE_ADT => {
                let words = ((*obj).size as usize) / 8;
                let base = payload as *mut i64;
                let mask = adt_float_mask((*obj)._pad);
                for i in 1..words {
                    if adt_float_slot(mask, i - 1) {
                        continue;
                    }
                    rewrite_i64_slot(base.add(i));
                }
            }
            TYPE_CLOSURE => {
                let words = ((*obj).size as usize) / 8;
                let base = payload as *mut i64;
                for i in 1..words {
                    rewrite_i64_slot(base.add(i));
                }
            }
            TYPE_TASK => {
                let words = ((*obj).size as usize) / 8;
                if words >= 2 {
                    rewrite_i64_slot((payload as *mut i64).add(1));
                }
            }
            _ => {}
        }
    }
}

unsafe fn rewrite_set_payload(payload: *mut u8, size: usize, float_elems: bool) {
    let base = payload as *mut i64;
    let n0 = *base;
    if n0 == SET_OVERLAY_MARK {
        rewrite_i64_slot(base.add(1)); // parent payload bits
        if float_elems {
            return;
        }
        let dn = overlay_delta_len(payload, size, 1);
        for i in 0..dn {
            rewrite_i64_slot(base.add(3 + i));
        }
        return;
    }
    if float_elems {
        return;
    }
    if !lumia_abi::tid_hash((*header_from_payload(payload)).type_id) {
        if n0 > 0 {
            let max = size.saturating_sub(8) / 8;
            let n = (n0 as usize).min(max);
            for i in 0..n {
                rewrite_i64_slot(base.add(1 + i));
            }
        }
        return;
    }
    if n0 <= 0 {
        return;
    }
    let n = n0 as usize;
    let cap = *base.add(1);
    if cap <= 0 {
        return;
    }
    let cap = cap as usize;
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
        rewrite_i64_slot(cell);
    }
}

unsafe fn rewrite_map_payload(payload: *mut u8, size: usize, float_keys: bool, float_vals: bool) {
    let base = payload as *mut i64;
    let n0 = *base;
    if n0 == MAP_OVERLAY_MARK {
        rewrite_i64_slot(base.add(1)); // parent payload bits
        let dn = overlay_delta_len(payload, size, 2);
        for i in 0..dn {
            if !float_keys {
                rewrite_i64_slot(base.add(3 + i * 2));
            }
            if !float_vals {
                rewrite_i64_slot(base.add(4 + i * 2));
            }
        }
        return;
    }
    if !lumia_abi::tid_hash((*header_from_payload(payload)).type_id) {
        if n0 <= 0 {
            return;
        }
        let n = n0 as usize;
        let max_pairs = size.saturating_sub(8) / 16;
        let n = n.min(max_pairs);
        for i in 0..n {
            if !float_keys {
                rewrite_i64_slot(base.add(1 + i * 2));
            }
            if !float_vals {
                rewrite_i64_slot(base.add(2 + i * 2));
            }
        }
        return;
    }
    if n0 <= 0 {
        return;
    }
    let n = n0 as usize;
    let cap = *base.add(1);
    if cap <= 0 {
        return;
    }
    let cap = cap as usize;
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
            rewrite_i64_slot(cell);
        }
        if !float_vals {
            rewrite_i64_slot(cell.add(1));
        }
    }
}

unsafe fn copy_to_old(h: &mut Heap, old: *mut ObjectHeader) -> *mut ObjectHeader {
    let nbytes = (*old).size as usize;
    let layout = header_layout(nbytes);
    let mem = alloc(layout);
    if mem.is_null() {
        crate::common::trap_abort("lumia: out of memory during nursery evacuate");
    }
    ptr::copy_nonoverlapping(old as *const u8, mem, layout.size());
    let new = mem as *mut ObjectHeader;
    (*new).marked = 0;
    h.old.push(new);
    h.old_set.insert(new);
    h.heap_set.insert(new);
    h.bytes_old = h.bytes_old.saturating_add(nbytes);
    // Forwarding stamp on the nursery original (rewound after rewrite).
    (*old).type_id = TYPE_NURSERY_FWD;
    (*old)._pad = new as u64;
    (*old).marked = 0;
    new
}

/// After minor mark: evacuate nursery survivors, promote system young in place,
/// free dead young, rewrite pointers, rewind nursery.
pub(super) fn finish_minor_young(h: &mut Heap) {
    let young = std::mem::take(&mut h.young);
    let mut freed = 0usize;
    let mut promoted = 0usize;
    let mut inplace: Vec<*mut ObjectHeader> = Vec::new();

    for obj in young {
        unsafe {
            if (*obj).marked == 0 {
                freed = freed.saturating_add((*obj).size as usize);
                let base = tid_base((*obj).type_id);
                if base == TYPE_TASK || base == TYPE_CHANNEL {
                    let id = *(payload_ptr(obj) as *const i64) as u64;
                    if base == TYPE_TASK {
                        crate::task::scheduler::on_task_handle_swept(id);
                    } else {
                        crate::task::scheduler::on_channel_handle_swept(id);
                    }
                }
                if h.nursery.contains_header(obj) {
                    h.nursery.forget_live(obj);
                    (*obj).type_id = TYPE_NURSERY_FREE;
                } else {
                    h.heap_set.remove(&obj);
                    let layout = header_layout((*obj).size as usize);
                    dealloc(obj as *mut u8, layout);
                }
                continue;
            }
            (*obj).marked = 0;
            promoted = promoted.saturating_add((*obj).size as usize);
            if h.nursery.contains_header(obj) {
                let _new = copy_to_old(h, obj);
                h.nursery.forget_live(obj);
            } else {
                inplace.push(obj);
            }
        }
    }

    for &obj in &inplace {
        h.old_set.insert(obj);
        h.old.push(obj);
    }

    for_each_mutator_root(|root| unsafe {
        let p = *root;
        if !p.is_null() {
            *root = follow_fwd_payload(p);
        }
    });
    for i in 0..h.perm.len() {
        let p = h.perm[i];
        if !p.is_null() {
            h.perm[i] = unsafe { follow_fwd_payload(p) };
        }
    }
    crate::memo::for_each_memo_i64_mut(|bits| {
        if may_be_heap_payload_bits(*bits) {
            let q = unsafe { follow_fwd_payload(*bits as *mut u8) };
            *bits = q as i64;
        }
    });
    crate::concurrency_policy::rewrite_sched_gc_roots(|bits| {
        if may_be_heap_payload_bits(bits) {
            unsafe { follow_fwd_payload(bits as *mut u8) as i64 }
        } else {
            bits
        }
    });

    let old_objs: Vec<*mut ObjectHeader> = h.old.iter().copied().collect();
    for obj in old_objs {
        rewrite_fields_on(obj);
    }

    h.nursery.rewind();
    h.remembered.clear();
    for &obj in &h.old {
        unsafe {
            (*obj).marked = 0;
        }
    }
    h.bytes_young = h.bytes_young.saturating_sub(freed.saturating_add(promoted));
    // Evacuated bytes already counted in `copy_to_old`; inplace still needs old bump.
    let inplace_bytes: usize = inplace.iter().map(|&o| unsafe { (*o).size as usize }).sum();
    h.bytes_old = h.bytes_old.saturating_add(inplace_bytes);
    h.refresh_alloc_pressure_fast();
}
