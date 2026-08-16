//! Generational mark-sweep GC backend and allocation ABI.
//!
//! Young allocations are tracked in a **young generation list** (`Heap.young`) —
//! not a bump-pointer nursery. Soft live-byte threshold → **minor** STW: mark
//! only young objects; old→young edges come from the **remembered set**
//! (`lumia_write_barrier`) plus rooted/permanent old objects. Survivors promote.
//!
//! Old-generation pressure → **incremental concurrent full mark** (Dijkstra-style
//! shade on the write barrier + black allocation), with a final remark before
//! sweep. `lumia_gc_collect` drains the mark to completion. Minor GC stays STW.

use std::alloc::{alloc, dealloc};

use crate::common::{
    header_from_payload, header_layout, is_heap_payload, is_young_payload,
    may_be_heap_payload_bits, payload_ptr, trap_abort, MarkSweep, ObjectHeader, PAR_WORKER,
    TYPE_ADT, TYPE_CHANNEL, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_IOTA, TYPE_MAP, TYPE_SET,
    TYPE_TASK,
};
use crate::heap::{with_heap, Heap};
use crate::mutator::for_each_mutator_root;
use crate::map_set::{map_mark_payload, set_mark_payload};
use crate::memo;
use lumia_abi::{
    list_elem_is_float, map_key_is_float, map_val_is_float, set_elem_is_float, tid_base,
};



fn incremental_full_enabled() -> bool {
    match std::env::var("LUMIA_GC_INCREMENTAL") {
        Ok(v) => {
            let v = v.trim();
            !(v.eq_ignore_ascii_case("0")
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("stw"))
        }
        Err(_) => with_heap(|h| h.incremental_full),
    }
}

impl MarkSweep {
    fn mark_from_roots_full() {
        // Mutex STW: hold heap for the whole root scan; nest sched snapshot (heap→sched).
        with_heap(|h| {
            h.mark_minor = false;
            Self::shade_all_roots_locked(h);
        });
    }

    /// Re-seed mark work from mutator / sched / memo roots (caller holds heap).
    fn shade_all_roots_locked(h: &mut Heap) {
        let (parked_vals, task_vals) = crate::task::snapshot_sched_gc_roots();
        for i in 0..h.perm.len() {
            let obj = h.perm[i];
            if may_be_heap_payload_bits(obj as i64)
                && h.contains_header(header_from_payload(obj))
            {
                mark_on(h, header_from_payload(obj));
            }
        }
        for_each_mutator_root(|root| unsafe {
            let p = *root;
            // Nested with_heap (reentrant): root walk holds heap + roots mutex.
            if is_heap_payload(p) {
                mark(header_from_payload(p));
            }
        });
        memo::for_each_memo_i64(|bits| {
            mark_value(bits);
        });
        for v in parked_vals.into_iter().chain(task_vals) {
            mark_value_on(h, v);
        }
    }

    fn clear_marks(objs: &[*mut ObjectHeader]) {
        for &obj in objs {
            unsafe {
                (*obj).marked = 0;
            }
        }
    }

    fn sweep_vec(
        objects: &mut Vec<*mut ObjectHeader>,
        heap_set: &mut rustc_hash::FxHashSet<*mut ObjectHeader>,
        old_set: &mut rustc_hash::FxHashSet<*mut ObjectHeader>,
        remembered: &mut rustc_hash::FxHashSet<*mut ObjectHeader>,
        promote_survivors: bool,
        from_old: bool,
    ) -> (usize /*freed*/, usize /*promoted*/, Vec<*mut ObjectHeader>) {
        let mut freed = 0usize;
        let mut promoted = 0usize;
        let mut survivors: Vec<*mut ObjectHeader> = Vec::new();
        let mut i = 0;
        while i < objects.len() {
            let obj = objects[i];
            unsafe {
                if (*obj).marked == 0 {
                    freed = freed.saturating_add((*obj).size as usize);
                    heap_set.remove(&obj);
                    if from_old {
                        old_set.remove(&obj);
                    }
                    remembered.remove(&obj);
                    // Notify sched before free so Task/Channel maps drop dangling ptrs
                    // (heap → sched lock order).
                    let base = tid_base((*obj).type_id);
                    if base == TYPE_TASK || base == TYPE_CHANNEL {
                        let id = *(payload_ptr(obj) as *const i64) as u64;
                        if base == TYPE_TASK {
                            crate::task::scheduler::on_task_handle_swept(id);
                        } else {
                            crate::task::scheduler::on_channel_handle_swept(id);
                        }
                    }
                    let layout = header_layout((*obj).size as usize);
                    dealloc(obj as *mut u8, layout);
                    objects.swap_remove(i);
                    continue;
                }
                (*obj).marked = 0;
                if promote_survivors {
                    promoted = promoted.saturating_add((*obj).size as usize);
                    survivors.push(obj);
                    objects.swap_remove(i);
                    continue;
                }
            }
            i += 1;
        }
        (freed, promoted, survivors)
    }

    fn minor_collect() {
        if with_heap(|h| h.full_marking) {
            if !Self::drain_full_mark() {
                return;
            }
        }
        // Mutex STW: mark + sweep under one heap lock (alloc/root push blocked).
        with_heap(|h| {
            if h.gc_inhibit > 0 {
                return;
            }
            h.mark_minor = true;
            let (parked_vals, task_vals) = crate::task::snapshot_sched_gc_roots();
            for_each_mutator_root(|root| unsafe {
                let p = *root;
                if is_young_payload(p) {
                    mark(header_from_payload(p));
                } else if is_heap_payload(p) {
                    scan_old_for_young(header_from_payload(p));
                }
            });
            for i in 0..h.perm.len() {
                let obj = h.perm[i];
                if is_young_payload(obj) {
                    mark_on(h, header_from_payload(obj));
                } else if is_heap_payload(obj) {
                    scan_old_for_young_on(h, header_from_payload(obj));
                }
            }
            memo::for_each_memo_i64(|bits| {
                let p = bits as *mut u8;
                if is_young_payload(p) {
                    mark(header_from_payload(p));
                } else if is_heap_payload(p) {
                    scan_old_for_young(header_from_payload(p));
                }
            });
            for v in parked_vals.into_iter().chain(task_vals) {
                let p = v as *mut u8;
                if is_young_payload(p) {
                    mark_on(h, header_from_payload(p));
                } else if is_heap_payload(p) {
                    scan_old_for_young_on(h, header_from_payload(p));
                }
            }
            let remembered: Vec<*mut ObjectHeader> = h.remembered.iter().copied().collect();
            for obj in remembered {
                scan_old_for_young_on(h, obj);
            }
            h.mark_minor = false;

            let (freed, promoted, survivors) = Self::sweep_vec(
                &mut h.young,
                &mut h.heap_set,
                &mut h.old_set,
                &mut h.remembered,
                true,
                false,
            );
            Self::clear_marks(&h.old);
            h.remembered.clear();
            h.bytes_young = h.bytes_young.saturating_sub(freed.saturating_add(promoted));
            h.bytes_old = h.bytes_old.saturating_add(promoted);
            for &obj in &survivors {
                h.old_set.insert(obj);
            }
            h.old.extend(survivors);
            h.refresh_alloc_pressure_fast();
        });
    }

    fn begin_full_mark() -> bool {
        // Clear marks + raise `full_marking` under one heap lock so mutators
        // cannot allocate white objects in the gap. Respect `gc_inhibit`.
        let started = with_heap(|h| {
            if h.gc_inhibit > 0 {
                return false;
            }
            Self::clear_marks(&h.young);
            Self::clear_marks(&h.old);
            h.mark_work.clear();
            h.full_marking = true;
            crate::heap::set_full_marking_fast(true);
            h.mark_minor = false;
            true
        });
        if started {
            Self::mark_from_roots_full();
        }
        started
    }

    fn full_collect_stw() -> bool {
        // Mutex STW across clear + mark + sweep.
        with_heap(|h| {
            if h.gc_inhibit > 0 {
                return false;
            }
            Self::clear_marks(&h.young);
            Self::clear_marks(&h.old);
            h.mark_work.clear();
            h.mark_minor = false;
            h.full_marking = false;
            crate::heap::set_full_marking_fast(false);
            Self::shade_all_roots_locked(h);
            let (freed_y, _, _) = Self::sweep_vec(
                &mut h.young,
                &mut h.heap_set,
                &mut h.old_set,
                &mut h.remembered,
                false,
                false,
            );
            let (freed_o, _, _) = Self::sweep_vec(
                &mut h.old,
                &mut h.heap_set,
                &mut h.old_set,
                &mut h.remembered,
                false,
                true,
            );
            h.remembered.clear();
            h.bytes_young = h.bytes_young.saturating_sub(freed_y);
            h.bytes_old = h.bytes_old.saturating_sub(freed_o);
            h.refresh_alloc_pressure_fast();
            true
        })
    }

    fn full_collect() -> bool {
        if with_heap(|h| h.full_marking) {
            return Self::drain_full_mark();
        }
        if incremental_full_enabled() {
            if !Self::begin_full_mark() {
                return false;
            }
            Self::drain_full_mark()
        } else {
            Self::full_collect_stw()
        }
    }

    fn mark_quantum(budget: usize) -> bool {
        with_heap(|h| {
            if !h.full_marking {
                return false;
            }
            let mut n = 0usize;
            while n < budget {
                let Some(obj) = h.mark_work.pop() else {
                    break;
                };
                // Scan under the same heap borrow (no per-edge Mutex).
                scan_fields_on(h, obj);
                n += 1;
            }
            if !h.mark_work.is_empty() {
                return true;
            }
            // Wavefront empty: re-shade roots so mid-mark mutator publishes
            // (channel/task/TLS) become grey before terminal sweep.
            Self::shade_all_roots_locked(h);
            if !h.mark_work.is_empty() {
                return true;
            }
            // Terminal mutex STW: wait out inhibit (same rule as begin_full / minor).
            if h.gc_inhibit > 0 {
                return true;
            }
            h.full_marking = false;
            crate::heap::set_full_marking_fast(false);
            h.mark_minor = false;
            h.mark_work.clear();
            Self::shade_all_roots_locked(h);
            // Rescan already-black objects for late edges.
            let blacks: Vec<*mut ObjectHeader> =
                h.young.iter().chain(h.old.iter()).copied().collect();
            for obj in blacks {
                unsafe {
                    if (*obj).marked != 0 {
                        scan_fields_on(h, obj);
                    }
                }
            }
            let (freed_y, _, _) = Self::sweep_vec(
                &mut h.young,
                &mut h.heap_set,
                &mut h.old_set,
                &mut h.remembered,
                false,
                false,
            );
            let (freed_o, _, _) = Self::sweep_vec(
                &mut h.old,
                &mut h.heap_set,
                &mut h.old_set,
                &mut h.remembered,
                false,
                true,
            );
            h.remembered.clear();
            h.bytes_young = h.bytes_young.saturating_sub(freed_y);
            h.bytes_old = h.bytes_old.saturating_sub(freed_o);
            h.refresh_alloc_pressure_fast();
            false
        })
    }

    /// Drain incremental mark to completion. Returns `false` if blocked on `gc_inhibit`
    /// before the terminal remark/sweep.
    fn drain_full_mark() -> bool {
        if !with_heap(|h| h.full_marking) {
            return true;
        }
        for _ in 0..1_000_000 {
            if with_heap(|h| h.gc_inhibit > 0 && h.mark_work.is_empty()) {
                return false;
            }
            if !Self::mark_quantum(usize::MAX / 4) {
                return true;
            }
        }
        false
    }

    fn maybe_collect_on_alloc() {
        let (full_marking, mark_quantum, young_limit, old_limit, young, old) = with_heap(|h| {
            (
                h.full_marking,
                h.mark_quantum,
                h.young_limit,
                h.old_limit,
                h.bytes_young,
                h.bytes_old,
            )
        });
        if full_marking {
            Self::mark_quantum(mark_quantum);
            let (young_now, still_full) =
                with_heap(|h| (h.bytes_young, h.full_marking));
            if still_full && young_now >= young_limit {
                Self::drain_full_mark();
            }
            if !with_heap(|h| h.full_marking) && young_now >= young_limit {
                Self::minor_collect();
            }
            return;
        }
        if young >= young_limit {
            Self::minor_collect();
        }
        let old_now = if young >= young_limit {
            with_heap(|h| h.bytes_old)
        } else {
            old
        };
        if old_now >= old_limit {
            if incremental_full_enabled() {
                if Self::begin_full_mark() {
                    let q = with_heap(|h| h.mark_quantum);
                    Self::mark_quantum(q);
                }
            } else {
                let _ = Self::full_collect_stw();
            }
        }
    }
}

/// Shade a heap object grey for the incremental full mark (Dijkstra).
fn shade_on(h: &mut Heap, obj: *mut ObjectHeader) {
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

fn shade(obj: *mut ObjectHeader) {
    with_heap(|h| shade_on(h, obj));
}

fn shade_payload(payload: *mut u8) {
    if payload.is_null() || !is_heap_payload(payload) {
        return;
    }
    shade(header_from_payload(payload));
}

/// Scan fields of an already-black object; shade (or recursively mark) children.
/// Caller must hold the process heap lock (`h`).
fn scan_fields_on(h: &mut Heap, obj: *mut ObjectHeader) {
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
                let mask = (*obj)._pad;
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

fn scan_old_for_young_on(h: &mut Heap, obj: *mut ObjectHeader) {
    unsafe {
        if obj.is_null() || (*obj).marked != 0 {
            return;
        }
        (*obj).marked = 1;
        scan_fields_on(h, obj);
    }
}

fn scan_old_for_young(obj: *mut ObjectHeader) {
    with_heap(|h| scan_old_for_young_on(h, obj));
}

impl MarkSweep {
    /// Allocate `nbytes` of payload (plus header) into the young generation.
    pub fn alloc(&mut self, nbytes: usize, type_id: u32) -> *mut u8 {
        if PAR_WORKER.get() {
            trap_abort(
                "lumia: heap allocation inside parallel map worker \
                 (use scalar Int/Bool/Float callbacks only)",
            );
        }
        // Skip heap Mutex when soft pressure is idle (flag refreshed under lock).
        let try_gc = crate::heap::alloc_pressure_fast()
            && with_heap(|h| {
                h.gc_inhibit == 0
                    && (h.full_marking
                        || h.bytes_young >= h.young_limit
                        || h.bytes_old >= h.old_limit)
            });
        if try_gc {
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

    pub fn collect(&mut self) {
        // Start collect only while `gc_inhibit == 0` (checked inside begin/STW).
        for _ in 0..1_000_000 {
            if Self::full_collect() {
                return;
            }
            std::thread::yield_now();
        }
        trap_abort("lumia: gc_collect blocked by gc_inhibit");
    }

    pub fn write_barrier(&mut self, obj: *mut u8, _field: u32, new_ptr: *mut u8) {
        if obj.is_null()
            || !may_be_heap_payload_bits(obj as i64)
            || !is_heap_payload(obj)
        {
            return;
        }
        let obj_h = header_from_payload(obj);
        // Int/Bool/FunRef / null cannot be young targets — skip second probe.
        let new_h = if !may_be_heap_payload_bits(new_ptr as i64) || !is_heap_payload(new_ptr)
        {
            None
        } else {
            Some(header_from_payload(new_ptr))
        };
        with_heap(|heap| {
            if heap.is_old_header(obj_h) {
                if let Some(nh) = new_h {
                    if heap.contains_header(nh) && !heap.is_old_header(nh) {
                        heap.remembered.insert(obj_h);
                    }
                }
            }
            if heap.full_marking {
                if let Some(nh) = new_h {
                    if heap.contains_header(nh) {
                        unsafe {
                            if (*nh).marked == 0 {
                                (*nh).marked = 1;
                                heap.mark_work.push(nh);
                            }
                        }
                    }
                }
            }
        });
    }
}

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
    (*header).rc = if matches!(tid_base(type_id), TYPE_LIST | TYPE_ADT) {
        1
    } else {
        0
    };
    (*header)._pad = 0;
    with_heap(|h| {
        (*header).marked = if h.full_marking { 1 } else { 0 };
        h.young.push(header);
        h.heap_set.insert(header);
        h.bytes_young += nbytes;
        h.refresh_alloc_pressure_fast();
    });
    payload_ptr(header)
}

#[no_mangle]
pub extern "C" fn lumia_alloc(nbytes: u64, type_id: u32) -> *mut u8 {
    MarkSweep.alloc(nbytes as usize, type_id)
}

#[no_mangle]
pub extern "C" fn lumia_root_push(slot: *mut *mut u8) {
    crate::mutator::ensure_mutator_registered();
    crate::mutator::push_root(slot);
    // Dijkstra: shade new roots during incremental full mark without taking
    // the heap Mutex solely to read the flag (see `full_marking_fast`).
    if crate::heap::full_marking_fast() {
        unsafe {
            shade_payload(*slot);
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_root_pop() {
    crate::mutator::pop_root();
}

#[no_mangle]
pub extern "C" fn lumia_write_barrier(obj: *mut u8, field: u32, new_ptr: *mut u8) {
    MarkSweep.write_barrier(obj, field, new_ptr);
}

#[no_mangle]
pub extern "C" fn lumia_gc_collect() {
    MarkSweep.collect();
}

#[cfg(test)]
pub(crate) fn gc_full_marking_for_test() -> bool {
    with_heap(|h| h.full_marking)
}

#[cfg(test)]
pub(crate) fn gc_set_incremental_full_for_test(on: bool) {
    with_heap(|h| h.incremental_full = on);
}

#[cfg(test)]
pub(crate) fn gc_set_mark_quantum_for_test(n: usize) {
    with_heap(|h| h.mark_quantum = n.max(1));
}
