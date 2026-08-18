//! High-level IR — named bindings after light desugaring from syntax AST.

mod adt_classify;
mod ast;
mod builtin_info;
mod builtin_surface;
mod desugar_slots;
mod langitem;
mod list_hof;
mod lower;
mod mangle;
mod match_check;
mod visit;

pub use adt_classify::{classify_sum_field_recursive, sum_parametric_arity};
pub use ast::{
    AdtDef, AdtVariant, Builtin, BuiltinFamily, CtorInfo, Expr, Fun, Item, Module, ProductDef,
};
pub use builtin_info::{BuiltinEffect, BuiltinEmit, BuiltinInfo, ResultHeap};
pub use builtin_surface::{surface_names, SurfaceName, SurfaceRole, PRELUDE_CTORS};
pub use desugar_slots::{
    is_list_builder_acc_slot, is_scalar_fold_acc_slot, FOLD_ELEM_PREFIX, FOR_INDEX_PREFIX,
    FUSE_ACC_PREFIX, LIST_BUILDER_ACC_PREFIXES,
};
pub use langitem::{
    is_option, is_option_or_result, is_result, prelude_adt, prelude_type_param_count, PreludeAdt,
    PreludeVariant, OPTION, PRELUDE_ADTS, RESULT,
};
pub use list_hof::{desugar_list_fold_sequential, desugar_list_map_sequential};
pub use lower::{expand_with_known, lower_module, lower_module_recovering, LowerCtx, LowerError};
pub use mangle::mangle_trait_method;
pub use visit::{
    all_free_vars, fold, for_each_expr, for_each_expr_mut, for_each_expr_skipping_lambdas,
    free_vars_expr,
};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
