//! GC test helpers (extracted from `common` — Todo: RT 测例半迁).

use crate::gc::set_gc_limits_for_test;
use crate::heap::with_heap;

pub(super) fn gc_live_bytes_for_test() -> (usize, usize) {
    with_heap(|heap| (heap.bytes_young, heap.bytes_old))
}

pub(super) fn gc_heap_lens_for_test() -> (usize, usize) {
    with_heap(|heap| (heap.young.len(), heap.old.len()))
}

pub(super) fn gc_remembered_len_for_test() -> usize {
    with_heap(|heap| heap.remembered.len())
}

pub(super) struct GcLimitGuard {
    young: usize,
    old: usize,
}

impl GcLimitGuard {
    pub(super) fn set(young: usize, old: usize) -> Self {
        // Flush TLS LABs so pending bytes hit `bytes_young` before the new soft limit.
        let (y, o) = with_heap(|h| {
            crate::mutator::flush_all_labs(h);
            (h.young_limit, h.old_limit)
        });
        set_gc_limits_for_test(young, old);
        Self { young: y, old: o }
    }
}

impl Drop for GcLimitGuard {
    fn drop(&mut self) {
        set_gc_limits_for_test(self.young, self.old);
    }
}
