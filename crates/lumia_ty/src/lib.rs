//! Hindley-Milner style type inference + effect sets.

mod alt;
mod display;
mod effects;
mod infer;
mod parallel;
mod traits;
mod typecheck;
mod types;

pub use display::{display_type, pretty_type_with, subst_num_vars, var_names_for};
pub use effects::check_effect_boundaries;
pub use infer::{
    infer_module, infer_module_recovering, infer_module_with_options, infer_module_with_visibility,
    InferOptions,
};
pub use parallel::finalize_auto_parallel;
pub use typecheck::{typecheck_hir, typecheck_hir_recovering, TypecheckOptions};
pub use types::{expr_span, Effect, NameVisibility, Scheme, Type, TypeError, TypedModule};

#[cfg(test)]
mod tests;
