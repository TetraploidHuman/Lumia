//! List operations and ranges.

mod core;
mod ops;
mod par;
mod tid;

pub(crate) use core::{force_heap_list, list_get_of, list_len_of};
pub use core::{
    lumi_list_append, lumi_list_empty, lumi_list_get, lumi_list_len, lumi_list_promote,
    lumi_list_release, lumi_list_retain, lumi_ptr_eq,
};
pub use ops::*;
pub use par::*;
#[cfg(test)]
pub(crate) use tid::ensure_list_f64;
pub(crate) use tid::list_float_elems;
pub use tid::lumi_ensure_list_f64;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::header_from_payload;
    use crate::TYPE_LIST_IOTA;

    #[test]
    fn range_empty_and_inverted() {
        let empty = lumi_range(5, 5);
        assert_eq!(list_len_of(empty), 0);
        let inv = lumi_range(10, 3);
        assert_eq!(list_len_of(inv), 0);
        let incl = lumi_range_inclusive(2, 4);
        assert_eq!(list_len_of(incl), 3);
        assert_eq!(list_get_of(incl, 2), 4);
    }

    #[test]
    fn iota_take_preserves_iota_tag() {
        let r = lumi_range(10, 20);
        let t = lumi_list_take(r, 3);
        unsafe {
            assert_eq!((*header_from_payload(t)).type_id, TYPE_LIST_IOTA);
        }
        assert_eq!(list_len_of(t), 3);
        assert_eq!(list_get_of(t, 0), 10);
        assert_eq!(list_get_of(t, 2), 12);
        // Negative / oversized take clamps.
        assert_eq!(list_len_of(lumi_list_take(r, -1)), 0);
        assert_eq!(list_len_of(lumi_list_take(r, 100)), 10);
    }

    #[test]
    fn reverse_and_sort_heap_list() {
        let mut xs = lumi_list_empty();
        for v in [3, 1, 2] {
            xs = lumi_list_append(xs, v);
        }
        let rev = lumi_list_reverse(xs);
        assert_eq!(list_get_of(rev, 0), 2);
        assert_eq!(list_get_of(rev, 2), 3);
        let sorted = lumi_list_sort(xs);
        assert_eq!(list_get_of(sorted, 0), 1);
        assert_eq!(list_get_of(sorted, 1), 2);
        assert_eq!(list_get_of(sorted, 2), 3);
    }
}
