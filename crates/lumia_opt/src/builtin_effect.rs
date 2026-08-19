//! Shared builtin trap/effect classification for memo-family passes.
//!
//! [`builtin_may_trap_or_effect`] is the single source of truth for DCE, LICM,
//! and CSE: anything that may trap, perform IO, or observe concurrency must not
//! be dropped, hoisted past control, or CSE-deduped.

use lumia_hir::Builtin;

/// Whether a builtin call must be treated as trapping or effectful for DCE/LICM/CSE.
pub(crate) fn builtin_may_trap_or_effect(b: &Builtin) -> bool {
    if b.is_io() {
        return true;
    }
    // Keep anything that may trap or has observable concurrency / abort.
    // Pure non-trapping ops (Range*, AdtTag, ListLen, …) stay out so DCE/CSE/LICM
    // can drop or hoist unused calls (see Todo: DCE builtin_may_trap).
    matches!(
        b,
        Builtin::ListGet
            | Builtin::MapRemove
            | Builtin::MatchFail
            | Builtin::Assert
            | Builtin::ListParMap
            | Builtin::ListParFold
            | Builtin::AdtField
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_hir::Builtin;

    #[test]
    fn pure_range_and_len_are_not_trapping() {
        assert!(!builtin_may_trap_or_effect(&Builtin::Range));
        assert!(!builtin_may_trap_or_effect(&Builtin::RangeInclusive));
        assert!(!builtin_may_trap_or_effect(&Builtin::AdtTag));
        assert!(!builtin_may_trap_or_effect(&Builtin::ListLen));
    }

    #[test]
    fn list_get_and_par_map_trap_or_effect() {
        assert!(builtin_may_trap_or_effect(&Builtin::ListGet));
        assert!(builtin_may_trap_or_effect(&Builtin::ListParMap));
    }
}
