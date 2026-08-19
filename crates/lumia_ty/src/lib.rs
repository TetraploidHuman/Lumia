//! Hindley-Milner style type inference + effect sets.
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::collapsible_match)]

mod alt;
mod core_ty;
mod display;
mod effects;
mod infer;
mod parallel;
mod product_resolve;
mod span_facts;
mod traits;
mod typecheck;
mod types;

pub use core_ty::{close_type, is_closed as core_ty_is_closed, CoreTy};
pub use display::{display_type, pretty_type_with, subst_num_vars, var_names_for};
pub use effects::check_effect_boundaries;
pub use infer::{
    infer_module, infer_module_recovering, infer_module_with_options, infer_module_with_visibility,
    InferOptions,
};
pub use parallel::{finalize_auto_parallel, type_at_span};
pub use typecheck::{typecheck_hir, typecheck_hir_recovering, TypecheckOptions};
pub use types::{expr_span, Effect, NameVisibility, Scheme, Type, TypeError, TypedModule};

#[cfg(test)]
mod tests;
