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
    TYPE_CHANNEL, TYPE_TASK,
};
use crate::heap::{with_heap, Heap};
use crate::mutator::for_each_mutator_root;
use crate::memo;
use lumia_abi::tid_base;

mod alloc_ffi;
mod limits;
mod pressure;
mod shade;

pub use alloc_ffi::{
    lumia_alloc, lumia_gc_collect, lumia_root_pop, lumia_root_push, lumia_write_barrier,
};
pub(crate) use alloc_ffi::{
    init_alloc_header, insert_young, list_payload_bytes, soft_gc_needed,
};
pub(crate) use pressure::{
    alloc_pressure_fast, full_marking_fast, refresh_from_heap as refresh_alloc_pressure_fast,
    set_full_marking_fast,
};
pub(crate) use shade::{mark, mark_on, mark_value, mark_value_on};

#[cfg(test)]
pub(crate) use limits::{
    gc_set_incremental_full_for_test, gc_set_mark_quantum_for_test, set_gc_limits_for_test,
};

use limits::incremental_full_enabled;
use shade::{scan_fields_on, scan_old_for_young, scan_old_for_young_on};

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
        let (parked_vals, task_vals) = crate::concurrency_policy::snapshot_sched_gc_roots();
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
        if with_heap(|h| h.full_marking) && !Self::drain_full_mark() {
            return;
        }
        // Mutex STW: mark + sweep under one heap lock (alloc/root push blocked).
        with_heap(|h| {
            if h.gc_inhibit > 0 {
                return;
            }
            h.mark_minor = true;
            let (parked_vals, task_vals) = crate::concurrency_policy::snapshot_sched_gc_roots();
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
            crate::gc::set_full_marking_fast(true);
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
            crate::gc::set_full_marking_fast(false);
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
            crate::gc::set_full_marking_fast(false);
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

impl MarkSweep {
    /// Allocate `nbytes` of payload (plus header) into the young generation.
    pub fn alloc(&mut self, nbytes: usize, type_id: u32) -> *mut u8 {
        if PAR_WORKER.get() {
            trap_abort(
                "lumia: heap allocation inside parallel map worker \
                 (use scalar Int/Bool/Float callbacks only)",
            );
        }
        let layout = header_layout(nbytes);
        unsafe {
            let mem = alloc(layout);
            if mem.is_null() {
                trap_abort("lumia: out of memory");
            }
            let header = init_alloc_header(mem, nbytes, type_id);
            // Idle soft-pressure: one heap lock (insert only).
            // Soft pressure: one lock that either inserts or signals collect —
            // avoids the old peek+insert double lock when no collect runs.
            if !alloc_pressure_fast() {
                with_heap(|h| insert_young(h, header, nbytes));
                return payload_ptr(header);
            }
            enum Outcome {
                Done,
                NeedCollect,
            }
            let outcome = with_heap(|h| {
                if soft_gc_needed(h) {
                    Outcome::NeedCollect
                } else {
                    insert_young(h, header, nbytes);
                    Outcome::Done
                }
            });
            match outcome {
                Outcome::Done => payload_ptr(header),
                Outcome::NeedCollect => {
                    Self::maybe_collect_on_alloc();
                    with_heap(|h| insert_young(h, header, nbytes));
                    payload_ptr(header)
                }
            }
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

#[cfg(test)]
pub(crate) fn gc_full_marking_for_test() -> bool {
    with_heap(|h| h.full_marking)
}
