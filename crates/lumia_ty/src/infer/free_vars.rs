//! Free-variable names in HIR expressions (for spawn capture checks).

use lumia_hir::{all_free_vars, Expr};
use rustc_hash::FxHashSet as HashSet;

/// Collect names referenced free in `expr` (not bound by nested lets/lambdas).
///
/// Delegates to [`lumia_hir::all_free_vars`] (incl. Assign LHS as a use).
pub(crate) fn free_var_names(expr: &Expr) -> HashSet<String> {
    all_free_vars(expr)
}
