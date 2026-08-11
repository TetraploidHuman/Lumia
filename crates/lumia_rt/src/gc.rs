//! Generational mark-sweep GC backend and allocation ABI.
//!
//! Young allocations land in a nursery. Soft threshold → **minor** STW:
//! mark from roots + scan all old objects as roots (no write barrier / card
//! table yet), then free unmarked young and promote survivors.
//! Old-generation pressure or `lumia_gc_collect` → **full** mark-sweep.

use std::alloc::{alloc, dealloc};
use std::cell::RefCell;

use crate::common::{
    header_from_payload, header_layout, is_heap_payload, payload_ptr, trap_abort, MarkSweep,
    MmBackend, ObjectHeader, BYTES_OLD, BYTES_YOUNG, GC_INHIBIT, HEAP_LIMIT, HEAP_OLD, HEAP_SET,
    HEAP_YOUNG, PAR_WORKER, PERM_OBJECTS, ROOTS, TYPE_ADT, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_IOTA,
    TYPE_MAP, TYPE_SET, YOUNG_LIMIT,
};
use crate::map_set::{map_mark_payload, set_mark_payload};
use crate::memo;
use lumia_abi::{
    gc_skip_float_slot, list_elem_is_float, map_key_is_float, map_val_is_float, set_elem_is_float,
    tid_base,
};

impl MarkSweep {
    fn mark_from_roots() {
        ROOTS.with(|r| {
            for root in r.borrow().iter() {
                unsafe {
                    let p = **root;
                    // Slot may hold Int / FunRef bits; only mark real heap payloads.
                    if is_heap_payload(p) {
                        mark(header_from_payload(p));
                    }
                }
            }
        });
        PERM_OBJECTS.with(|p| {
            for obj in p.borrow().iter() {
                if is_heap_payload(*obj) {
                    mark(header_from_payload(*obj));
                }
            }
        });
        // Transparent memo tables may hold heap arg/result bits if a Fun's ABI
        // types were misclassified as scalar; keep them alive across GC.
        Self::mark_memo_tables();
    }

    /// Old→young edges are found by treating every tenured object as a root.
    /// Correct without a write barrier; later card tables can narrow this scan.
    fn mark_from_old_generation() {
        HEAP_OLD.with(|h| {
            for &obj in h.borrow().iter() {
                mark(obj);
            }
        });
    }

    fn mark_i64_if_heap(bits: i64) {
        let p = bits as *mut u8;
        if is_heap_payload(p) {
            mark(header_from_payload(p));
        }
    }

    fn mark_memo_tables() {
        memo::for_each_memo_i64(Self::mark_i64_if_heap);
    }

    fn clear_marks(objs: &[*mut ObjectHeader]) {
        for &obj in objs {
            unsafe {
                (*obj).marked = 0;
            }
        }
    }

    /// Free unmarked objects in `heap`; return freed payload bytes.
    /// When `promote_survivors`, marked objects move into `HEAP_OLD`.
    fn sweep_vec(
        heap: &mut Vec<*mut ObjectHeader>,
        promote_survivors: bool,
    ) -> (usize /*freed*/, usize /*promoted*/) {
        let mut freed = 0usize;
        let mut promoted = 0usize;
        let mut survivors: Vec<*mut ObjectHeader> = Vec::new();
        let mut i = 0;
        while i < heap.len() {
            let obj = heap[i];
            unsafe {
                if (*obj).marked == 0 {
                    freed = freed.saturating_add((*obj).size as usize);
                    HEAP_SET.with(|s| {
                        s.borrow_mut().remove(&obj);
                    });
                    let layout = header_layout((*obj).size as usize);
                    dealloc(obj as *mut u8, layout);
                    heap.swap_remove(i);
                    continue;
                }
                (*obj).marked = 0;
                if promote_survivors {
                    promoted = promoted.saturating_add((*obj).size as usize);
                    survivors.push(obj);
                    heap.swap_remove(i);
                    continue;
                }
            }
            i += 1;
        }
        if promote_survivors && !survivors.is_empty() {
            HEAP_OLD.with(|old| old.borrow_mut().extend(survivors));
        }
        (freed, promoted)
    }

    fn minor_collect() {
        Self::mark_from_roots();
        Self::mark_from_old_generation();
        let (freed, promoted) = HEAP_YOUNG.with(|h| {
            let mut young = h.borrow_mut();
            Self::sweep_vec(&mut young, true)
        });
        // Old objects were marked as roots; clear their mark bits.
        HEAP_OLD.with(|h| Self::clear_marks(&h.borrow()));
        BYTES_YOUNG.with(|y| {
            let mut live = y.borrow_mut();
            *live = live.saturating_sub(freed.saturating_add(promoted));
        });
        BYTES_OLD.with(|o| {
            let mut live = o.borrow_mut();
            *live = live.saturating_add(promoted);
        });
    }

    fn full_collect() {
        Self::mark_from_roots();
        let freed_y = HEAP_YOUNG.with(|h| {
            let mut young = h.borrow_mut();
            let (freed, _) = Self::sweep_vec(&mut young, false);
            freed
        });
        let freed_o = HEAP_OLD.with(|h| {
            let mut old = h.borrow_mut();
            let (freed, _) = Self::sweep_vec(&mut old, false);
            freed
        });
        BYTES_YOUNG.with(|y| {
            let mut live = y.borrow_mut();
            *live = live.saturating_sub(freed_y);
        });
        BYTES_OLD.with(|o| {
            let mut live = o.borrow_mut();
            *live = live.saturating_sub(freed_o);
        });
    }

    fn maybe_collect_on_alloc() {
        let young_limit = *YOUNG_LIMIT.lock().unwrap_or_else(|e| e.into_inner());
        let old_limit = *HEAP_LIMIT.lock().unwrap_or_else(|e| e.into_inner());
        let young = BYTES_YOUNG.with(|y| *y.borrow());
        if young >= young_limit {
            Self::minor_collect();
        }
        let old = BYTES_OLD.with(|o| *o.borrow());
        if old >= old_limit {
            Self::full_collect();
        }
    }
}

pub(crate) fn mark(obj: *mut ObjectHeader) {
    unsafe {
        if obj.is_null() || (*obj).marked != 0 {
            return;
        }
        (*obj).marked = 1;
        let payload = payload_ptr(obj);
        let tid = (*obj).type_id;
        match tid_base(tid) {
            TYPE_LIST => {
                if list_elem_is_float(tid) {
                    // Unboxed Float elems — never heap pointers.
                } else {
                    let n = *(payload as *const i64);
                    let base = payload as *const i64;
                    for i in 0..n as usize {
                        mark_value(*base.add(1 + i));
                    }
                }
            }
            TYPE_LIST_IOTA => {
                // Scalar bounds only — no child pointers.
            }
            TYPE_SET => {
                set_mark_payload(payload, (*obj).size as usize, set_elem_is_float(tid));
            }
            TYPE_MAP => {
                map_mark_payload(
                    payload,
                    (*obj).size as usize,
                    map_key_is_float(tid),
                    map_val_is_float(tid),
                );
            }
            TYPE_ADT => {
                let words = ((*obj).size as usize) / 8;
                let base = payload as *const i64;
                let float_mask = (*obj)._pad;
                // ADT: [tag][fields…] — skip tag; skip unboxed Float fields (layout in `_pad`).
                for i in 1..words {
                    if gc_skip_float_slot(tid, i - 1, float_mask) {
                        continue;
                    }
                    mark_value(*base.add(i));
                }
            }
            TYPE_CLOSURE => {
                let words = ((*obj).size as usize) / 8;
                let base = payload as *const i64;
                // Closure: [fn_ptr][caps…] — skip word0 as non-heap.
                for i in 1..words {
                    mark_value(*base.add(i));
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn mark_value(x: i64) {
    let p = x as *mut u8;
    if is_heap_payload(p) {
        mark(header_from_payload(p));
    }
}

impl MmBackend for MarkSweep {
    fn alloc(&mut self, nbytes: usize, type_id: u32) -> *mut u8 {
        if PAR_WORKER.get() {
            trap_abort(
                "lumia: heap allocation inside parallel map worker \
                 (use scalar Int/Bool/Float callbacks only)",
            );
        }
        let inhibit = GC_INHIBIT.get();
        if inhibit == 0 {
            Self::maybe_collect_on_alloc();
        }
        let layout = header_layout(nbytes);
        unsafe {
            let mem = alloc(layout);
            if mem.is_null() {
                trap_abort("lumia: out of memory");
            }
            finish_alloc(mem, nbytes, type_id)
        }
    }

    fn collect(&mut self) {
        // Explicit collect is always full (API contract / tests).
        Self::full_collect();
    }
}

/// Payload bytes for a HeapList of `len` elements (`[len][e…]`), overflow-safe.
pub(crate) fn list_payload_bytes(len: i64) -> u64 {
    if len < 0 {
        trap_abort("lumia: negative list length");
    }
    (len as u64)
        .checked_add(1)
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as u64)
        .unwrap_or_else(|| trap_abort(&format!("lumia: list too large (len={len})")))
}

pub(crate) unsafe fn finish_alloc(mem: *mut u8, nbytes: usize, type_id: u32) -> *mut u8 {
    if nbytes > u32::MAX as usize {
        trap_abort("lumia: allocation too large (exceeds u32 size field)");
    }
    let header = mem as *mut ObjectHeader;
    (*header).type_id = type_id;
    (*header).size = nbytes as u32;
    (*header).marked = 0;
    // List COW uniqueness: `_pad` is a refcount (ADT uses `_pad` as float mask).
    (*header)._pad = if tid_base(type_id) == TYPE_LIST { 1 } else { 0 };
    HEAP_YOUNG.with(|h| h.borrow_mut().push(header));
    HEAP_SET.with(|s| {
        s.borrow_mut().insert(header);
    });
    BYTES_YOUNG.with(|b| *b.borrow_mut() += nbytes);
    payload_ptr(header)
}

thread_local! {
    pub(crate) static BACKEND: RefCell<MarkSweep> = const { RefCell::new(MarkSweep) };
}

#[no_mangle]
pub extern "C" fn lumia_alloc(nbytes: u64, type_id: u32) -> *mut u8 {
    BACKEND.with(|b| b.borrow_mut().alloc(nbytes as usize, type_id))
}

#[no_mangle]
pub extern "C" fn lumia_root_push(slot: *mut *mut u8) {
    ROOTS.with(|r| r.borrow_mut().push(slot));
}

#[no_mangle]
pub extern "C" fn lumia_root_pop() {
    ROOTS.with(|r| {
        let _ = r.borrow_mut().pop();
    });
}

#[no_mangle]
pub extern "C" fn lumia_write_barrier(obj: *mut u8, field: u32, new_ptr: *mut u8) {
    // Generational collector scans all old objects during minor GC, so a write
    // barrier is not required for correctness yet. Concurrent / card-table
    // collectors must replace this; the ABI stays stable.
    BACKEND.with(|b| b.borrow_mut().write_barrier(obj, field, new_ptr));
}

#[no_mangle]
pub extern "C" fn lumia_gc_collect() {
    BACKEND.with(|b| b.borrow_mut().collect());
}
