//! List operations and ranges.

mod core;
mod f64_view;
mod ops;
mod par;
mod tid;

pub(crate) use core::{force_heap_list, list_get_of, list_len_of};
pub(crate) use f64_view::{f64_elems, f64_elems_mut, require_len};
pub use core::{
    lumia_list_append, lumia_list_empty, lumia_list_get, lumia_list_len, lumia_list_promote,
    lumia_list_release, lumia_list_retain, lumia_ptr_eq,
};
pub use ops::{
    lumia_list_concat, lumia_list_join, lumia_list_reverse, lumia_list_set, lumia_list_slice,
    lumia_list_sort, lumia_list_sort_by_keys, lumia_list_take, lumia_range, lumia_range_inclusive,
};
pub use par::{lumia_list_par_fold, lumia_list_par_map};
#[cfg(test)]
pub(crate) use tid::ensure_list_f64;
pub(crate) use tid::list_float_elems;
pub use tid::lumia_ensure_list_f64;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::header_from_payload;
    use crate::TYPE_LIST_IOTA;

    #[test]
    fn range_empty_and_inverted() {
        let empty = lumia_range(5, 5);
        assert_eq!(list_len_of(empty), 0);
        let inv = lumia_range(10, 3);
        assert_eq!(list_len_of(inv), 0);
        let incl = lumia_range_inclusive(2, 4);
        assert_eq!(list_len_of(incl), 3);
        assert_eq!(list_get_of(incl, 2), 4);
    }

    #[test]
    fn iota_take_preserves_iota_tag() {
        let r = lumia_range(10, 20);
        let t = lumia_list_take(r, 3);
        unsafe {
            assert_eq!((*header_from_payload(t)).type_id, TYPE_LIST_IOTA);
        }
        assert_eq!(list_len_of(t), 3);
        assert_eq!(list_get_of(t, 0), 10);
        assert_eq!(list_get_of(t, 2), 12);
        // Negative / oversized take clamps.
        assert_eq!(list_len_of(lumia_list_take(r, -1)), 0);
        assert_eq!(list_len_of(lumia_list_take(r, 100)), 10);
    }

    #[test]
    fn reverse_and_sort_heap_list() {
        let mut xs = lumia_list_empty();
        for v in [3, 1, 2] {
            xs = lumia_list_append(xs, v);
        }
        let rev = lumia_list_reverse(xs);
        assert_eq!(list_get_of(rev, 0), 2);
        assert_eq!(list_get_of(rev, 2), 3);
        let sorted = lumia_list_sort(xs);
        assert_eq!(list_get_of(sorted, 0), 1);
        assert_eq!(list_get_of(sorted, 1), 2);
        assert_eq!(list_get_of(sorted, 2), 3);
    }
}
