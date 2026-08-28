//! Generational mark-sweep GC backend and allocation ABI.
//!
//! Young allocations land in a nursery. Soft threshold → **minor** STW:
//! mark only young objects; old→young edges come from the **remembered set**
//! (`lumi_write_barrier`) plus rooted/permanent old objects. Survivors promote.
//!
//! Old-generation pressure → **incremental concurrent full mark** (Dijkstra-style
//! shade on the write barrier + black allocation), with a final remark before
//! sweep. `lumi_gc_collect` drains the mark to completion. Minor GC stays STW.

use std::alloc::{alloc, dealloc};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::common::{
    header_from_payload, header_layout, is_heap_payload, is_old_header, is_young_payload,
    payload_ptr, remember_old_to_young, trap_abort, MarkSweep, MmBackend, ObjectHeader, BYTES_OLD,
    BYTES_YOUNG, GC_INHIBIT, HEAP_LIMIT, HEAP_OLD, HEAP_OLD_SET, HEAP_SET, HEAP_YOUNG, PAR_WORKER,
    PERM_OBJECTS, REMEMBERED, ROOTS, TYPE_ADT, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_IOTA,
    TYPE_LIST_SLICE, TYPE_MAP, TYPE_SET, YOUNG_LIMIT,
};
use crate::map_set::{map_mark_payload, set_mark_payload};
#[cfg(feature = "opt-memo")]
use crate::memo;
use lumi_abi::{
    list_elem_is_float, map_key_is_float, map_val_is_float, set_elem_is_float, tid_base,
};
use rustc_hash::FxHashSet;

thread_local! {
    /// When true, [`mark_value`] / map-set markers only follow young payloads.
    static MARK_MINOR: Cell<bool> = const { Cell::new(false) };
    /// Incremental full-heap mark in progress (mutator may run between quanta).
    static FULL_MARKING: Cell<bool> = const { Cell::new(false) };
    /// Grey objects for the concurrent/incremental full mark.
    static MARK_WORK: RefCell<Vec<*mut ObjectHeader>> = const { RefCell::new(Vec::new()) };
    /// Objects processed per alloc-triggered quantum.
    static MARK_QUANTUM: Cell<usize> = const { Cell::new(256) };
    /// When false, old pressure still does a classic STW full collect.
    /// Overridden by `LUMI_GC_INCREMENTAL=0|false|off|stw`.
    static INCREMENTAL_FULL: Cell<bool> = const { Cell::new(true) };
    /// Observability: collection counts / bytes reclaimed.
    static GC_MINOR_COUNT: Cell<u64> = const { Cell::new(0) };
    static GC_FULL_COUNT: Cell<u64> = const { Cell::new(0) };
    static GC_BYTES_FREED: Cell<u64> = const { Cell::new(0) };
    /// Parallel mark worker count (1 = sequential). Set via `LUMI_GC_MARK_THREADS`.
    static MARK_THREADS: Cell<usize> = const { Cell::new(1) };
    /// When set, [`mark_value`] / [`shade`] use a shared heap snapshot (parallel drain).
    static PAR_MARK_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static PAR_MARK_SNAP: RefCell<Option<Arc<SendHdrSet>>> = const { RefCell::new(None) };
    static PAR_MARK_GREYS: RefCell<Vec<SendHdr>> = const { RefCell::new(Vec::new()) };
}

/// Raw header pointer for STW parallel mark (mutator stopped; workers only shade).
struct SendHdr(*mut ObjectHeader);
// SAFETY: drain is STW; workers only touch `marked` via CAS and read object fields.
unsafe impl Send for SendHdr {}

struct SendHdrSet(FxHashSet<*mut ObjectHeader>);
// SAFETY: snapshot is immutable after construction during STW parallel drain.
unsafe impl Send for SendHdrSet {}
unsafe impl Sync for SendHdrSet {}

impl SendHdrSet {
    fn contains(&self, h: *mut ObjectHeader) -> bool {
        self.0.contains(&h)
    }
}

fn incremental_full_enabled() -> bool {
    match std::env::var("LUMI_GC_INCREMENTAL") {
        Ok(v) => {
            let v = v.trim();
            !(v.eq_ignore_ascii_case("0")
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("stw"))
        }
        Err(_) => INCREMENTAL_FULL.get(),
    }
}

/// Scale incremental mark quantum / worker count when `LUMI_GC_MARK_THREADS>1`.
pub(crate) fn configure_mark_parallelism(threads: usize) {
    let n = threads.max(1);
    MARK_THREADS.with(|c| c.set(n));
    if n > 1 {
        MARK_QUANTUM.with(|c| c.set(c.get().saturating_mul(n)));
    }
}

fn note_gc_freed(bytes: usize) {
    GC_BYTES_FREED.with(|c| c.set(c.get().saturating_add(bytes as u64)));
}

impl MarkSweep {
    fn mark_from_roots_full() {
        MARK_MINOR.set(false);
        ROOTS.with(|r| {
            for root in r.borrow().iter() {
                unsafe {
                    let p = **root;
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
        #[cfg(feature = "opt-memo")]
        memo::for_each_memo_i64(|bits| {
            let p = bits as *mut u8;
            if is_heap_payload(p) {
                mark(header_from_payload(p));
            }
        });
    }

    fn mark_from_roots_minor() {
        MARK_MINOR.set(true);
        ROOTS.with(|r| {
            for root in r.borrow().iter() {
                unsafe {
                    let p = **root;
                    if is_young_payload(p) {
                        mark(header_from_payload(p));
                    } else if is_heap_payload(p) {
                        scan_old_for_young(header_from_payload(p));
                    }
                }
            }
        });
        PERM_OBJECTS.with(|p| {
            for obj in p.borrow().iter() {
                if is_young_payload(*obj) {
                    mark(header_from_payload(*obj));
                } else if is_heap_payload(*obj) {
                    scan_old_for_young(header_from_payload(*obj));
                }
            }
        });
        #[cfg(feature = "opt-memo")]
        memo::for_each_memo_i64(|bits| {
            let p = bits as *mut u8;
            if is_young_payload(p) {
                mark(header_from_payload(p));
            }
        });
        REMEMBERED.with(|r| {
            for &obj in r.borrow().iter() {
                scan_old_for_young(obj);
            }
        });
        MARK_MINOR.set(false);
    }

    fn clear_marks(objs: &[*mut ObjectHeader]) {
        for &obj in objs {
            unsafe {
                (*obj).marked = 0;
            }
        }
    }

    fn clear_all_marks() {
        HEAP_YOUNG.with(|h| Self::clear_marks(&h.borrow()));
        HEAP_OLD.with(|h| Self::clear_marks(&h.borrow()));
    }

    fn sweep_vec(
        heap: &mut Vec<*mut ObjectHeader>,
        promote_survivors: bool,
        // When sweeping the old generation, also drop `HEAP_OLD_SET` entries.
        from_old: bool,
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
                    // Slice views bump the parent's COW RC; drop that alias on free.
                    if tid_base((*obj).type_id) == TYPE_LIST_SLICE {
                        let parent = *(payload_ptr(obj) as *const i64) as *mut u8;
                        crate::common::list_rc_release(parent);
                    }
                    HEAP_SET.with(|s| {
                        s.borrow_mut().remove(&obj);
                    });
                    if from_old {
                        HEAP_OLD_SET.with(|s| {
                            s.borrow_mut().remove(&obj);
                        });
                    }
                    REMEMBERED.with(|r| {
                        r.borrow_mut().remove(&obj);
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
            HEAP_OLD_SET.with(|s| {
                let mut set = s.borrow_mut();
                for &obj in &survivors {
                    set.insert(obj);
                }
            });
            HEAP_OLD.with(|old| old.borrow_mut().extend(survivors));
        }
        (freed, promoted)
    }

    fn minor_collect() {
        // Never interleave minor with an in-flight full mark.
        if FULL_MARKING.get() {
            Self::drain_full_mark();
        }
        Self::mark_from_roots_minor();
        let (freed, promoted) = HEAP_YOUNG.with(|h| {
            let mut young = h.borrow_mut();
            Self::sweep_vec(&mut young, true, false)
        });
        HEAP_OLD.with(|h| Self::clear_marks(&h.borrow()));
        REMEMBERED.with(|r| r.borrow_mut().clear());
        BYTES_YOUNG.with(|y| {
            let mut live = y.borrow_mut();
            *live = live.saturating_sub(freed.saturating_add(promoted));
        });
        BYTES_OLD.with(|o| {
            let mut live = o.borrow_mut();
            *live = live.saturating_add(promoted);
        });
        note_gc_freed(freed);
        GC_MINOR_COUNT.with(|c| c.set(c.get() + 1));
    }

    fn full_collect_stw() {
        Self::clear_all_marks();
        MARK_WORK.with(|w| w.borrow_mut().clear());
        Self::mark_from_roots_full();
        Self::sweep_after_full_mark();
    }

    fn sweep_after_full_mark() {
        let freed_y = HEAP_YOUNG.with(|h| {
            let mut young = h.borrow_mut();
            let (freed, _) = Self::sweep_vec(&mut young, false, false);
            freed
        });
        let freed_o = HEAP_OLD.with(|h| {
            let mut old = h.borrow_mut();
            let (freed, _) = Self::sweep_vec(&mut old, false, true);
            freed
        });
        REMEMBERED.with(|r| r.borrow_mut().clear());
        BYTES_YOUNG.with(|y| {
            let mut live = y.borrow_mut();
            *live = live.saturating_sub(freed_y);
        });
        BYTES_OLD.with(|o| {
            let mut live = o.borrow_mut();
            *live = live.saturating_sub(freed_o);
        });
        note_gc_freed(freed_y.saturating_add(freed_o));
        GC_FULL_COUNT.with(|c| c.set(c.get() + 1));
    }

    fn begin_full_mark() {
        Self::clear_all_marks();
        MARK_WORK.with(|w| w.borrow_mut().clear());
        FULL_MARKING.set(true);
        MARK_MINOR.set(false);
        // Seed greys from roots (worklist-based; `mark` shades under FULL_MARKING).
        Self::mark_from_roots_full();
    }

    /// Process up to `budget` grey objects. Returns true if still marking.
    fn mark_quantum(budget: usize) -> bool {
        if !FULL_MARKING.get() {
            return false;
        }
        let mut n = 0usize;
        while n < budget {
            let obj = MARK_WORK.with(|w| w.borrow_mut().pop());
            let Some(obj) = obj else {
                break;
            };
            scan_fields(obj);
            n += 1;
        }
        if !MARK_WORK.with(|w| w.borrow().is_empty()) {
            return true;
        }
        // Worklist drained: re-shade roots + remark black objects (covers alloc-init
        // stores that skip the write barrier).
        Self::mark_from_roots_full();
        remark_black_objects();
        if !MARK_WORK.with(|w| w.borrow().is_empty()) {
            return true;
        }
        FULL_MARKING.set(false);
        Self::sweep_after_full_mark();
        false
    }

    fn drain_full_mark() {
        if !FULL_MARKING.get() {
            return;
        }
        let threads = MARK_THREADS.with(|c| c.get());
        if threads > 1 {
            Self::drain_full_mark_parallel(threads);
        } else {
            while Self::mark_quantum(usize::MAX / 4) {}
        }
    }

    /// STW parallel drain: snapshot `HEAP_SET`, scan greys on worker threads, merge
    /// newly shaded children on the collector thread. Mutator is stopped.
    fn drain_full_mark_parallel(threads: usize) {
        let n_workers = threads.min(8).max(2);
        loop {
            if !FULL_MARKING.get() {
                return;
            }
            let batch = MARK_WORK.with(|w| std::mem::take(&mut *w.borrow_mut()));
            if batch.is_empty() {
                Self::mark_from_roots_full();
                remark_black_objects();
                if MARK_WORK.with(|w| w.borrow().is_empty()) {
                    FULL_MARKING.set(false);
                    Self::sweep_after_full_mark();
                    return;
                }
                continue;
            }
            let snap = HEAP_SET.with(|s| Arc::new(SendHdrSet(s.borrow().clone())));
            let chunk_size = (batch.len() / n_workers).max(1);
            let chunks: Vec<Vec<SendHdr>> = batch
                .chunks(chunk_size)
                .map(|c| c.iter().copied().map(SendHdr).collect())
                .collect();
            std::thread::scope(|scope| {
                let mut joins = Vec::with_capacity(chunks.len());
                for chunk in chunks {
                    let snap = Arc::clone(&snap);
                    joins.push(scope.spawn(move || {
                        PAR_MARK_SNAP.with(|s| *s.borrow_mut() = Some(snap));
                        PAR_MARK_ACTIVE.set(true);
                        for SendHdr(obj) in chunk {
                            scan_fields(obj);
                        }
                        PAR_MARK_ACTIVE.set(false);
                        let greys = PAR_MARK_GREYS.with(|g| std::mem::take(&mut *g.borrow_mut()));
                        PAR_MARK_SNAP.with(|s| *s.borrow_mut() = None);
                        greys
                    }));
                }
                for j in joins {
                    if let Ok(greys) = j.join() {
                        MARK_WORK.with(|w| {
                            w.borrow_mut()
                                .extend(greys.into_iter().map(|SendHdr(p)| p));
                        });
                    }
                }
            });
        }
    }

    fn full_collect() {
        if FULL_MARKING.get() {
            Self::drain_full_mark();
            return;
        }
        if incremental_full_enabled() {
            Self::begin_full_mark();
            Self::drain_full_mark();
        } else {
            Self::full_collect_stw();
        }
    }

    fn maybe_collect_on_alloc() {
        if FULL_MARKING.get() {
            let q = MARK_QUANTUM.with(|c| c.get());
            Self::mark_quantum(q);
            // Nursery pressure during concurrent mark: finish full, then minor.
            let young_limit = YOUNG_LIMIT.with(|c| c.get());
            let young = BYTES_YOUNG.with(|y| *y.borrow());
            if FULL_MARKING.get() && young >= young_limit {
                Self::drain_full_mark();
            }
            if !FULL_MARKING.get() && young >= young_limit {
                Self::minor_collect();
            }
            return;
        }
        let young_limit = YOUNG_LIMIT.with(|c| c.get());
        let old_limit = HEAP_LIMIT.with(|c| c.get());
        let young = BYTES_YOUNG.with(|y| *y.borrow());
        if young >= young_limit {
            Self::minor_collect();
        }
        let old = BYTES_OLD.with(|o| *o.borrow());
        if old >= old_limit {
            if incremental_full_enabled() {
                Self::begin_full_mark();
                let q = MARK_QUANTUM.with(|c| c.get());
                Self::mark_quantum(q);
            } else {
                Self::full_collect_stw();
            }
        }
    }
}

/// Shade a heap object grey for the incremental full mark (Dijkstra).
fn shade(obj: *mut ObjectHeader) {
    if obj.is_null() {
        return;
    }
    unsafe {
        // Atomic CAS so parallel drain workers do not race on `marked`.
        let marked_ptr = std::ptr::addr_of_mut!((*obj).marked);
        // SAFETY: `marked` is a plain `u32` field; exclusive access during STW drain
        // except for concurrent CAS among mark workers (intended).
        let atom = AtomicU32::from_ptr(marked_ptr);
        if atom
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        if PAR_MARK_ACTIVE.get() {
            PAR_MARK_GREYS.with(|g| g.borrow_mut().push(SendHdr(obj)));
        } else {
            MARK_WORK.with(|w| w.borrow_mut().push(obj));
        }
    }
}

fn shade_payload(payload: *mut u8) {
    if payload.is_null() || !is_heap_payload(payload) {
        return;
    }
    shade(header_from_payload(payload));
}

/// Scan fields of an already-black object; shade (or recursively mark) children.
fn scan_fields(obj: *mut ObjectHeader) {
    unsafe {
        let payload = payload_ptr(obj);
        let tid = (*obj).type_id;
        match tid_base(tid) {
            TYPE_LIST => {
                if !list_elem_is_float(tid) {
                    let n = *(payload as *const i64);
                    let base = payload as *const i64;
                    // Clamp to payload size: corrupted / negative `len` must not OOB.
                    let max_elems = ((*obj).size as usize).saturating_sub(8) / 8;
                    if n > 0 {
                        let n = (n as usize).min(max_elems);
                        for i in 0..n {
                            mark_value(*base.add(1 + i));
                        }
                    }
                }
            }
            TYPE_LIST_IOTA => {}
            TYPE_LIST_SLICE => {
                // payload: [parent][offset][len] — keep parent alive.
                let base = payload as *const i64;
                mark_value(*base);
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
                // Do **not** trust `_pad` float bits for GC skip: product mono can
                // over-tag List/ADT fields as Float (UAF). `mark_value` already
                // no-ops on non-heap bit patterns, so true Float slots are safe.
                let words = ((*obj).size as usize) / 8;
                let base = payload as *const i64;
                for i in 1..words {
                    mark_value(*base.add(i));
                }
            }
            TYPE_CLOSURE => {
                let words = ((*obj).size as usize) / 8;
                let base = payload as *const i64;
                for i in 1..words {
                    mark_value(*base.add(i));
                }
            }
            _ => {}
        }
    }
}

fn remark_black_objects() {
    let mut blacks: Vec<*mut ObjectHeader> = Vec::new();
    HEAP_YOUNG.with(|h| blacks.extend(h.borrow().iter().copied()));
    HEAP_OLD.with(|h| blacks.extend(h.borrow().iter().copied()));
    for obj in blacks {
        unsafe {
            if (*obj).marked != 0 {
                scan_fields(obj);
            }
        }
    }
}

/// Used by `map_set` mark helpers; respects [`MARK_MINOR`] / [`FULL_MARKING`].
pub(crate) fn mark_value(x: i64) {
    let p = x as *mut u8;
    if PAR_MARK_ACTIVE.get() {
        if p.is_null() {
            return;
        }
        let h = header_from_payload(p);
        let ok = PAR_MARK_SNAP.with(|s| {
            s.borrow()
                .as_ref()
                .map(|snap| snap.contains(h))
                .unwrap_or(false)
        });
        if ok {
            mark(h);
        }
        return;
    }
    if MARK_MINOR.get() {
        if is_young_payload(p) {
            mark(header_from_payload(p));
        }
    } else if is_heap_payload(p) {
        mark(header_from_payload(p));
    }
}

pub(crate) fn mark(obj: *mut ObjectHeader) {
    unsafe {
        if obj.is_null() {
            return;
        }
        // Parallel workers use shade (CAS) even though FULL_MARKING is TLS-local.
        if PAR_MARK_ACTIVE.get() || FULL_MARKING.get() {
            shade(obj);
            return;
        }
        if (*obj).marked != 0 {
            return;
        }
        if MARK_MINOR.get() && is_old_header(obj) {
            return;
        }
        (*obj).marked = 1;
        scan_fields(obj);
    }
}

/// Scan old object fields for young pointers (rooted old / remembered card).
fn scan_old_for_young(obj: *mut ObjectHeader) {
    unsafe {
        if obj.is_null() || (*obj).marked != 0 {
            return;
        }
        (*obj).marked = 1; // "scanned this minor"
        scan_fields(obj);
    }
}

impl MmBackend for MarkSweep {
    fn alloc(&mut self, nbytes: usize, type_id: u32) -> *mut u8 {
        if PAR_WORKER.get() {
            trap_abort(
                "lumi: heap allocation inside parallel map worker \
                 (use scalar Int/Bool/Float callbacks only)",
            );
        }
        if GC_INHIBIT.get() == 0 {
            Self::maybe_collect_on_alloc();
        }
        let layout = header_layout(nbytes);
        unsafe {
            let mem = alloc(layout);
            if mem.is_null() {
                trap_abort("lumi: out of memory");
            }
            finish_alloc(mem, nbytes, type_id)
        }
    }

    fn collect(&mut self) {
        Self::full_collect();
    }

    fn write_barrier(&mut self, obj: *mut u8, _field: u32, new_ptr: *mut u8) {
        remember_old_to_young(obj, new_ptr as i64);
        // Dijkstra incremental-update: shade the installed pointer during full mark.
        if FULL_MARKING.get() {
            shade_payload(new_ptr);
        }
    }
}

pub(crate) fn list_payload_bytes(len: i64) -> u64 {
    if len < 0 {
        trap_abort("lumi: negative list length");
    }
    (len as u64)
        .checked_add(1)
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as u64)
        .unwrap_or_else(|| trap_abort(&format!("lumi: list too large (len={len})")))
}

pub(crate) unsafe fn finish_alloc(mem: *mut u8, nbytes: usize, type_id: u32) -> *mut u8 {
    if nbytes > u32::MAX as usize {
        trap_abort("lumi: allocation too large (exceeds u32 size field)");
    }
    let header = mem as *mut ObjectHeader;
    (*header).type_id = type_id;
    (*header).size = nbytes as u32;
    // Black allocation during concurrent mark: object is live; final remark
    // picks up fields filled without a write barrier (codegen alloc-init).
    (*header).marked = if FULL_MARKING.get() { 1 } else { 0 };
    (*header).rc = if matches!(
        tid_base(type_id),
        TYPE_LIST | TYPE_LIST_SLICE | TYPE_ADT | TYPE_MAP | TYPE_SET
    ) {
        1
    } else {
        0
    };
    (*header)._pad = 0;
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
pub extern "C" fn lumi_alloc(nbytes: u64, type_id: u32) -> *mut u8 {
    let _ = crate::mm::current_mm_mode();
    BACKEND.with(|b| b.borrow_mut().alloc(nbytes as usize, type_id))
}

#[no_mangle]
pub extern "C" fn lumi_root_push(slot: *mut *mut u8) {
    ROOTS.with(|r| r.borrow_mut().push(slot));
    if FULL_MARKING.get() {
        unsafe {
            shade_payload(*slot);
        }
    }
}

#[no_mangle]
pub extern "C" fn lumi_root_pop() {
    ROOTS.with(|r| {
        let _ = r.borrow_mut().pop();
    });
}

#[no_mangle]
pub extern "C" fn lumi_write_barrier(obj: *mut u8, field: u32, new_ptr: *mut u8) {
    BACKEND.with(|b| b.borrow_mut().write_barrier(obj, field, new_ptr));
}

#[no_mangle]
pub extern "C" fn lumi_gc_collect() {
    BACKEND.with(|b| b.borrow_mut().collect());
}

fn gc_stats_wanted(force: bool) -> bool {
    if force {
        return true;
    }
    match std::env::var("LUMI_GC_STATS") {
        Ok(v) => matches!(v.trim(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

/// Print GC collection counters to stderr when `force != 0` or `LUMI_GC_STATS=1`.
#[no_mangle]
pub extern "C" fn lumi_gc_print_stats(force: i64) {
    if !gc_stats_wanted(force != 0) {
        return;
    }
    eprintln!(
        "lumi gc: minor={} full={} bytes_freed={} mark_threads={}",
        GC_MINOR_COUNT.with(|c| c.get()),
        GC_FULL_COUNT.with(|c| c.get()),
        GC_BYTES_FREED.with(|c| c.get()),
        MARK_THREADS.with(|c| c.get()),
    );
}

#[no_mangle]
pub extern "C" fn lumi_gc_minor_count() -> i64 {
    GC_MINOR_COUNT.with(|c| c.get() as i64)
}

#[no_mangle]
pub extern "C" fn lumi_gc_full_count() -> i64 {
    GC_FULL_COUNT.with(|c| c.get() as i64)
}

#[cfg(test)]
pub(crate) fn gc_reset_stats_for_test() {
    GC_MINOR_COUNT.with(|c| c.set(0));
    GC_FULL_COUNT.with(|c| c.set(0));
    GC_BYTES_FREED.with(|c| c.set(0));
}

#[cfg(test)]
pub(crate) fn gc_full_marking_for_test() -> bool {
    FULL_MARKING.with(|c| c.get())
}

#[cfg(test)]
pub(crate) fn gc_set_incremental_full_for_test(on: bool) {
    INCREMENTAL_FULL.with(|c| c.set(on));
}

#[cfg(test)]
pub(crate) fn gc_set_mark_quantum_for_test(n: usize) {
    MARK_QUANTUM.with(|c| c.set(n.max(1)));
}
