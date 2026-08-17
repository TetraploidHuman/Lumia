//! Acc / index slot name prefixes emitted by HIR desugar.
//!
//! Core `float_cap_fixup` (and related ABI walks) classify slots by these
//! prefixes. Inventing a new prefix in a desugarer without updating
//! [`LIST_BUILDER_ACC_PREFIXES`] (or the fuse / index exceptions) silently
//! changes Float ABI.

/// Fused fold accumulator (`hof_fuse`).
pub const FUSE_ACC_PREFIX: &str = "__fuse_acc";

/// `for` loop index (`for_loops`). Includes the trailing `_` before the span id.
pub const FOR_INDEX_PREFIX: &str = "__i_";

/// Accumulators that build a **list / set / map** (must not upgrade to scalar Float).
pub const MAP_ACC_PREFIX: &str = "__map_acc";
pub const FMAP_ACC_PREFIX: &str = "__fmap_acc";
pub const TOLIST_ACC_PREFIX: &str = "__tolist_acc";
pub const TOSET_ACC_PREFIX: &str = "__toset_acc";
pub const TOMAP_ACC_PREFIX: &str = "__tomap_acc";
pub const UNION_ACC_PREFIX: &str = "__union_acc";
pub const ISECT_ACC_PREFIX: &str = "__isect_acc";
pub const DIFF_ACC_PREFIX: &str = "__diff_acc";
pub const FLT_ACC_PREFIX: &str = "__flt_acc";
/// Legacy / defensive — filter emits [`FLT_ACC_PREFIX`].
pub const FILTER_ACC_LEGACY_PREFIX: &str = "__filter_acc";

pub const LIST_BUILDER_ACC_PREFIXES: &[&str] = &[
    MAP_ACC_PREFIX,
    FMAP_ACC_PREFIX,
    TOLIST_ACC_PREFIX,
    TOSET_ACC_PREFIX,
    TOMAP_ACC_PREFIX,
    UNION_ACC_PREFIX,
    ISECT_ACC_PREFIX,
    DIFF_ACC_PREFIX,
    FLT_ACC_PREFIX,
    FILTER_ACC_LEGACY_PREFIX,
];

/// Fold element binder (`list_hof/fold`). Includes trailing `_` before the span id.
pub const FOLD_ELEM_PREFIX: &str = "__fold_x_";

#[inline]
pub fn is_list_builder_acc_slot(name: &str) -> bool {
    LIST_BUILDER_ACC_PREFIXES
        .iter()
        .any(|p| name.starts_with(p))
}

/// Sequential / fused fold slots (`a`, `__fuse_acc_*`). Exclude list builders
/// so map/filter/… → `List[…]` rets stay lists (not upgraded to scalar Float).
#[inline]
pub fn is_scalar_fold_acc_slot(name: &str) -> bool {
    if name.starts_with(FUSE_ACC_PREFIX) {
        return true;
    }
    !is_list_builder_acc_slot(name) && !name.starts_with(FOR_INDEX_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_fold_classifies_desugar_prefixes() {
        assert!(is_scalar_fold_acc_slot("acc"));
        assert!(is_scalar_fold_acc_slot(&format!("{FUSE_ACC_PREFIX}_1")));
        assert!(!is_scalar_fold_acc_slot(&format!("{MAP_ACC_PREFIX}_1")));
        assert!(!is_scalar_fold_acc_slot(&format!("{FLT_ACC_PREFIX}_9")));
        assert!(!is_scalar_fold_acc_slot(&format!("{FOR_INDEX_PREFIX}3")));
        assert!(!is_scalar_fold_acc_slot(&format!("{TOSET_ACC_PREFIX}_0")));
    }

    #[test]
    fn list_builder_table_covers_known_desugar_emitters() {
        // Lock-in: every non-legacy builder prefix used by list_hof / collections.
        for p in [
            MAP_ACC_PREFIX,
            FMAP_ACC_PREFIX,
            TOLIST_ACC_PREFIX,
            TOSET_ACC_PREFIX,
            TOMAP_ACC_PREFIX,
            UNION_ACC_PREFIX,
            ISECT_ACC_PREFIX,
            DIFF_ACC_PREFIX,
            FLT_ACC_PREFIX,
        ] {
            assert!(
                LIST_BUILDER_ACC_PREFIXES.contains(&p),
                "{p} missing from LIST_BUILDER_ACC_PREFIXES"
            );
        }
    }
}
