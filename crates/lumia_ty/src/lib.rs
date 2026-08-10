//! Hindley-Milner style type inference + effect sets.

mod alt;
mod effects;
mod infer;
mod parallel;
mod traits;
mod typecheck;
mod types;

pub use effects::check_effect_boundaries;
pub use infer::{
    infer_module, infer_module_with_options, infer_module_with_visibility, InferOptions,
};
pub use parallel::finalize_auto_parallel;
pub use typecheck::{typecheck_hir, TypecheckOptions};
pub use types::{Effect, NameVisibility, Scheme, Type, TypeError, TypedModule};

#[cfg(test)]
mod tests;
