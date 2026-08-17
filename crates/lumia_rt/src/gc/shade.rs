//! Incremental shade / scan / mark helpers for [`super::MarkSweep`].

use crate::common::{
    header_from_payload, is_heap_payload, may_be_heap_payload_bits, payload_ptr, ObjectHeader,
    TYPE_ADT, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_IOTA, TYPE_MAP, TYPE_SET, TYPE_TASK,
};
use crate::heap::{with_heap, Heap};
use crate::map_set::{map_mark_payload, set_mark_payload};
use lumia_abi::{
    list_elem_is_float, map_key_is_float, map_val_is_float, set_elem_is_float, tid_base,
};

/// Shade a heap object grey for the incremental full mark (Dijkstra).
pub(super) fn shade_on(h: &mut Heap, obj: *mut ObjectHeader) {
    if obj.is_null() {
        return;
    }
    unsafe {
        if (*obj).marked != 0 {
            return;
        }
        (*obj).marked = 1;
        h.mark_work.push(obj);
    }
}

pub(super) fn shade(obj: *mut ObjectHeader) {
    with_heap(|h| shade_on(h, obj));
}

pub(super) fn shade_payload(payload: *mut u8) {
    if payload.is_null() || !is_heap_payload(payload) {
        return;
    }
    shade(header_from_payload(payload));
}

/// Scan fields of an already-black object; shade (or recursively mark) children.
/// Caller must hold the process heap lock (`h`).
pub(super) fn scan_fields_on(h: &mut Heap, obj: *mut ObjectHeader) {
    unsafe {
        let payload = payload_ptr(obj);
        let tid = (*obj).type_id;
        match tid_base(tid) {
            TYPE_LIST => {
                if !list_elem_is_float(tid) {
                    let n = *(payload as *const i64);
                    let base = payload as *const i64;
                    let max_elems = ((*obj).size as usize).saturating_sub(8) / 8;
                    if n > 0 {
                        let n = (n as usize).min(max_elems);
                        for i in 0..n {
                            mark_value_on(h, *base.add(1 + i));
                        }
                    }
                }
            }
            TYPE_LIST_IOTA => {}
            TYPE_SET => {
                set_mark_payload(h, payload, (*obj).size as usize, set_elem_is_float(tid));
            }
            TYPE_MAP => {
                map_mark_payload(
                    h,
                    payload,
                    (*obj).size as usize,
                    map_key_is_float(tid),
                    map_val_is_float(tid),
                );
            }
            TYPE_ADT => {
                // Payload: [tag][field0]… — `_pad` bit `i` ⇒ field `i` is unboxed Float
                // (sanitized by `lumia_adt_set_float_mask`). Skip those without
                // membership probes; mistagged masks that bypass sanitize are UB.
                let words = ((*obj).size as usize) / 8;
                let base = payload as *const i64;
                let mask = crate::common::adt_float_mask((*obj)._pad);
                for i in 1..words {
                    let field_i = i - 1;
                    if crate::common::adt_float_slot(mask, field_i) {
                        continue;
                    }
                    mark_value_on(h, *base.add(i));
                }
            }
            TYPE_CLOSURE => {
                let words = ((*obj).size as usize) / 8;
                let base = payload as *const i64;
                for i in 1..words {
                    mark_value_on(h, *base.add(i));
                }
            }
            TYPE_TASK => {
                // [task_id, result]
                let words = ((*obj).size as usize) / 8;
                if words >= 2 {
                    mark_value_on(h, *(payload as *const i64).add(1));
                }
            }
            _ => {}
        }
    }
}

/// Used by `map_set` mark helpers; respects heap `mark_minor` / `full_marking`.
pub(crate) fn mark_value(x: i64) {
    // Int/Bool/FunRef immediates cannot be managed payloads — skip heap Mutex.
    if !may_be_heap_payload_bits(x) {
        return;
    }
    with_heap(|h| mark_value_on(h, x));
}

/// Mark one value word while the caller already holds the heap lock.
pub(crate) fn mark_value_on(h: &mut Heap, x: i64) {
    if !may_be_heap_payload_bits(x) {
        return;
    }
    let hdr = header_from_payload(x as *mut u8);
    if h.mark_minor {
        if h.contains_header(hdr) && !h.is_old_header(hdr) {
            mark_on(h, hdr);
        }
    } else if h.contains_header(hdr) {
        mark_on(h, hdr);
    }
}

pub(crate) fn mark(obj: *mut ObjectHeader) {
    with_heap(|h| mark_on(h, obj));
}

pub(crate) fn mark_on(h: &mut Heap, obj: *mut ObjectHeader) {
    unsafe {
        if obj.is_null() || (*obj).marked != 0 {
            return;
        }
        if h.mark_minor && h.is_old_header(obj) {
            return;
        }
        if h.full_marking {
            shade_on(h, obj);
            return;
        }
        (*obj).marked = 1;
        scan_fields_on(h, obj);
    }
}

pub(super) fn scan_old_for_young_on(h: &mut Heap, obj: *mut ObjectHeader) {
    unsafe {
        if obj.is_null() || (*obj).marked != 0 {
            return;
        }
        (*obj).marked = 1;
        scan_fields_on(h, obj);
    }
}

pub(super) fn scan_old_for_young(obj: *mut ObjectHeader) {
    with_heap(|h| scan_old_for_young_on(h, obj));
}

