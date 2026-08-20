//! Map and Set collections.

mod hash_probe;
mod map_core;
mod map_ops;
mod overlay;
mod set;
mod tid;

pub(crate) use hash_probe::{
    compact_linear_entries, finish_linear_container, open_hash_claim_slot_or_trap,
    open_hash_demote_linear_in_place, open_hash_find_slot, open_hash_from_linear,
    open_hash_remove_slot, OPEN_HASH_ST_EMPTY, OPEN_HASH_ST_FULL, OPEN_HASH_ST_TOMB,
};
pub(crate) use map_core::*;
pub use map_ops::{
    lumia_map_contains, lumia_map_empty, lumia_map_finish, lumia_map_get, lumia_map_items,
    lumia_map_keys, lumia_map_remove, lumia_map_set, lumia_map_values,
};
pub(crate) use overlay::{overlay_delta_len, MAP_OVERLAY_MARK, SET_OVERLAY_MARK};
pub(crate) use set::*;
pub use set::{
    lumia_set_contains, lumia_set_diff, lumia_set_empty, lumia_set_finish, lumia_set_insert,
    lumia_set_intersect, lumia_set_remove, lumia_set_union,
};
pub(crate) use tid::*;
pub use tid::{
    lumia_ensure_map_bool, lumia_ensure_map_f64, lumia_ensure_map_vbool, lumia_ensure_map_vf64,
    lumia_ensure_set_bool, lumia_ensure_set_f64,
};
