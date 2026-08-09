//! Syntax AST → HIR lowering.

mod collections;
mod ctx;
mod expr;
mod for_loops;
mod hof_fuse;
mod items;
mod match_arms;

pub use ctx::{LowerCtx, LowerError};
pub use items::lower_module;

pub(crate) use for_loops::{counter_for_in, empty_list, for_each_elem};
