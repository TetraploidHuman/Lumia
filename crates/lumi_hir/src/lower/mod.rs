//! Syntax AST → HIR lowering.

mod collections;
mod ctx;
mod expr;
mod for_loops;
mod hof_fuse;
mod items;
mod match_arms;

pub use ctx::{LowerCtx, LowerError, LowerOptions};
pub use expr::product::expand_with_known;
pub use items::{lower_module, lower_module_with_options};

pub(crate) use for_loops::{counter_for_in, empty_list, for_each_elem};
