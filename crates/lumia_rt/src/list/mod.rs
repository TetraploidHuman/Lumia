//! List operations and ranges.

mod core;
mod f64_view;
mod ops;
mod par;
mod tid;

pub(crate) use core::{force_heap_list, list_get_of, list_len_of};
pub use core::{
    lumia_list_append, lumia_list_empty, lumia_list_get, lumia_list_len, lumia_list_promote,
    lumia_list_release, lumia_list_retain, lumia_ptr_eq,
};
pub(crate) use f64_view::{f64_elems, f64_elems_mut, require_len};
pub use ops::{
    lumia_list_concat, lumia_list_join, lumia_list_reverse, lumia_list_set, lumia_list_slice,
    lumia_list_sort, lumia_list_sort_by_keys, lumia_list_take, lumia_range, lumia_range_inclusive,
};
pub use par::{lumia_list_par_fold, lumia_list_par_map};
#[cfg(test)]
pub(crate) use tid::ensure_list_f64;
pub(crate) use tid::list_bool_elems;
pub(crate) use tid::list_float_elems;
pub use tid::{lumia_ensure_list_bool, lumia_ensure_list_f64};

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
