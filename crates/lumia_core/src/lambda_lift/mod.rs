//! Lambda lifting and capture analysis.

mod captures;
mod float_abi;
mod heap;
mod rewrite;

pub(crate) use rewrite::lift_lambdas;
