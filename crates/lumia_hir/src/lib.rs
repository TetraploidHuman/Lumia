//! High-level IR — named bindings after light desugaring from syntax AST.

mod ast;
mod adt_classify;
mod builtin_info;
mod builtin_surface;
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
pub use langitem::{
    is_option, is_option_or_result, is_result, prelude_adt, prelude_type_param_count, PreludeAdt,
    PreludeVariant, OPTION, PRELUDE_ADTS, RESULT,
};
pub use list_hof::{desugar_list_fold_sequential, desugar_list_map_sequential};
pub use lower::{expand_with_known, lower_module, LowerCtx, LowerError};
pub use mangle::mangle_trait_method;
pub use visit::{all_free_vars, fold, for_each_expr, for_each_expr_mut, free_vars_expr};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
