//! Runtime integration tests (GC, map/set, memo, float eq, task/channel).

use super::*;
use crate::common::{header_from_payload, trap_abort, PAR_WORKER};
use crate::gc::list_payload_bytes;
use crate::list::force_heap_list;
use crate::map_set::{
    map_count, map_is_assoc, map_is_hash, map_is_overlay, map_overlay_dn, set_count, set_elem_at,
    set_is_hash, set_is_overlay, set_overlay_dn,
};
use crate::string_io::with_str_bytes;
use std::ptr;

mod gc_helpers;
use gc_helpers::{
    gc_heap_lens_for_test, gc_live_bytes_for_test, gc_remembered_len_for_test, GcLimitGuard,
};

mod eq;
mod gc;
mod list;
mod map_set;
mod memo;
mod task;
