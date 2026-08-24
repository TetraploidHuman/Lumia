//! Lambda lifting and capture analysis.

mod captures;
mod float_abi;
mod float_cap_fixup;
mod heap;
mod rewrite;

pub(crate) use float_cap_fixup::fixup_closure_float_caps;
pub(crate) use rewrite::lift_lambdas;
