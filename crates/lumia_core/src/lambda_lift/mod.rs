//! Lambda lifting and capture analysis.

mod captures;
mod channel_hint;
pub(crate) mod float_abi;
mod float_cap_fixup;
mod heap;
mod rewrite;

pub(crate) use channel_hint::refine_channel_elem_hint;
pub(crate) use float_cap_fixup::fixup_closure_float_caps;
pub(crate) use rewrite::lift_lambdas;
