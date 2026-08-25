//! Monomorphization, FunRef directization, and trait-method resolve.

mod fun_index;
mod funref_env;
mod key;
mod ret_ty;
mod specialize;
mod traits;

pub(crate) use specialize::specialize_mono_calls;
pub(crate) use traits::{
    directize_funref_calls, ensure_trait_method_stubs, resolve_trait_method_calls,
};

#[cfg(test)]
mod tests;
