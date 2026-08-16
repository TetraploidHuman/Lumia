//! Runtime integration tests (GC, map/set, memo, float eq, task/channel).

use super::*;
use crate::common::{
    gc_heap_lens_for_test, gc_live_bytes_for_test, header_from_payload, set_gc_limits_for_test,
    trap_abort, PAR_WORKER,
};
use crate::gc::list_payload_bytes;
use crate::list::force_heap_list;
use crate::map_set::{
    map_count, map_is_assoc, map_is_hash, map_is_overlay, map_overlay_dn, set_elem_at, set_is_hash,
};
use crate::string_io::with_str_bytes;
use crate::MmBackend;
use std::ptr;

struct GcLimitGuard {
    young: usize,
    old: usize,
}
impl GcLimitGuard {
    fn set(young: usize, old: usize) -> Self {
        let (y, o) = crate::heap::with_heap(|h| (h.young_limit, h.old_limit));
        set_gc_limits_for_test(young, old);
        Self { young: y, old: o }
    }
}
impl Drop for GcLimitGuard {
    fn drop(&mut self) {
        set_gc_limits_for_test(self.young, self.old);
    }
}

mod eq;
mod gc;
mod list;
mod map_set;
mod memo;
mod task;
