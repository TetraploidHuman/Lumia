//! Monomorphization, FunRef directization, and trait-method resolve.

mod directize;
mod fun_index;
mod key;
mod ret_ty;
mod specialize;
mod traits;

pub(crate) use directize::directize_funref_calls;
pub(crate) use specialize::specialize_mono_calls;
pub(crate) use traits::{ensure_trait_method_stubs, resolve_trait_method_calls};

#[cfg(test)]
mod tests;
