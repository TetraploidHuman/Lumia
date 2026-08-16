//! Mid-end optimization hard caps shared with planners.

/// Call-site const specialization caps (`SpecializeConstPass`).
pub const SPECIALIZE_CONST_MAX_CLONES_PER_FUN: usize = 16;
pub const SPECIALIZE_CONST_MAX_TOTAL_CLONES: usize = 64;
pub const SPECIALIZE_CONST_MAX_OPS: usize = 256;
/// Max nested inline expansions (`InlinePass`).
pub const INLINE_MAX_EXPAND_DEPTH: usize = 8;
