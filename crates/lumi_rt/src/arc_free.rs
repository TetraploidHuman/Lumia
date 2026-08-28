//! Eager free-on-zero when `--mm arc` / `LUMI_MM=arc`.
//!
//! Applies to all heap objects whose `rc` hits 0 (COW containers and non-COW
//! String/Bytes/Closure/…). Cycles are still reclaimed by mark-sweep
//! [`crate::gc::lumi_gc_collect`].

use crate::common::{
    cow_rc_release, cow_tid_ok, header_from_payload, header_layout, heap_rc_release,
    is_heap_payload, payload_ptr, ObjectHeader, BYTES_OLD, BYTES_YOUNG, HEAP_OLD, HEAP_OLD_SET,
    HEAP_SET, HEAP_YOUNG, REMEMBERED, TYPE_ADT, TYPE_BYTES, TYPE_CHAR, TYPE_CLOSURE, TYPE_LIST,
    TYPE_LIST_SLICE, TYPE_MAP, TYPE_SET, TYPE_STRING,
};
use crate::mm::{current_mm_mode, MmMode};
use lumi_abi::{list_elem_is_float, map_key_is_float, map_val_is_float, set_elem_is_float, tid_base};
use std::alloc::dealloc;

/// After `rc` drops to 0 in Arc mode, release children then unregister + dealloc.
///
/// Skipped while a full mark is in flight (object may still be grey); sweep or a
/// later release after mark finishes reclaims it. Cycles are broken by STW
/// mark-sweep (`--mm arc` forces STW full GC).
pub(crate) fn maybe_free_on_zero(payload: *mut u8, _adt_ok: bool) {
    if current_mm_mode() != MmMode::Arc || payload.is_null() {
        return;
    }
    if crate::gc::is_full_marking() {
        return;
    }
    if !is_heap_payload(payload) {
        return;
    }
    unsafe {
        let h = header_from_payload(payload);
        if (*h).rc != 0 {
            return;
        }
        free_heap_object(h);
    }
}

unsafe fn free_heap_object(obj: *mut ObjectHeader) {
    crate::cycle_cand::arc_free_enter();
    // Drop child aliases first (may recursively free).
    release_children(obj);
    let nbytes = (*obj).size as usize;
    unregister_header(obj, nbytes);
    let layout = header_layout(nbytes);
    dealloc(obj as *mut u8, layout);
    crate::cycle_cand::arc_free_leave();
}

unsafe fn release_children(obj: *mut ObjectHeader) {
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
                        release_value(*base.add(1 + i));
                    }
                }
            }
        }
        TYPE_LIST_SLICE => {
            let parent = *(payload as *const i64) as *mut u8;
            cow_rc_release(parent, false);
        }
        TYPE_SET => {
            if !set_elem_is_float(tid) {
                release_set_elems(payload, (*obj).size as usize);
            }
        }
        TYPE_MAP => {
            release_map_elems(
                payload,
                (*obj).size as usize,
                map_key_is_float(tid),
                map_val_is_float(tid),
            );
        }
        TYPE_ADT | TYPE_CLOSURE => {
            let words = ((*obj).size as usize) / 8;
            let base = payload as *const i64;
            for i in 1..words {
                release_value(*base.add(i));
            }
        }
        TYPE_STRING | TYPE_BYTES | TYPE_CHAR => {}
        _ => {}
    }
}

unsafe fn release_value(bits: i64) {
    let p = bits as *mut u8;
    if p.is_null() || !is_heap_payload(p) {
        return;
    }
    let h = header_from_payload(p);
    let tid = tid_base((*h).type_id);
    if cow_tid_ok((*h).type_id, tid == TYPE_ADT) {
        cow_rc_release(p, tid == TYPE_ADT);
    } else {
        // Non-COW heap object under Arc (String / Bytes / Closure / …).
        heap_rc_release(p);
    }
}

unsafe fn release_set_elems(payload: *mut u8, size: usize) {
    // Conservative: treat every aligned i64 in payload as a possible pointer.
    let words = size / 8;
    let base = payload as *const i64;
    for i in 0..words {
        release_value(*base.add(i));
    }
}

unsafe fn release_map_elems(payload: *mut u8, size: usize, float_keys: bool, float_vals: bool) {
    let words = size / 8;
    let base = payload as *const i64;
    for i in 0..words {
        // Skip float-tagged slots when both key and val are float (dense); otherwise
        // release conservatively — `release_value` no-ops on non-heap bits.
        if float_keys && float_vals {
            continue;
        }
        release_value(*base.add(i));
    }
}

unsafe fn unregister_header(obj: *mut ObjectHeader, nbytes: usize) {
    HEAP_SET.with(|s| {
        s.borrow_mut().remove(&obj);
    });
    let from_old = HEAP_OLD_SET.with(|s| {
        let mut set = s.borrow_mut();
        set.remove(&obj)
    });
    REMEMBERED.with(|r| {
        r.borrow_mut().remove(&obj);
    });
    if from_old {
        HEAP_OLD.with(|h| {
            let mut v = h.borrow_mut();
            if let Some(i) = v.iter().position(|&x| x == obj) {
                v.swap_remove(i);
            }
        });
        BYTES_OLD.with(|b| {
            let mut live = b.borrow_mut();
            *live = live.saturating_sub(nbytes);
        });
    } else {
        HEAP_YOUNG.with(|h| {
            let mut v = h.borrow_mut();
            if let Some(i) = v.iter().position(|&x| x == obj) {
                v.swap_remove(i);
            }
        });
        BYTES_YOUNG.with(|b| {
            let mut live = b.borrow_mut();
            *live = live.saturating_sub(nbytes);
        });
    }
}
