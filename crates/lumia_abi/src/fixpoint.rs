//! Shared mid-end / codegen fixpoint iteration caps.
//!
//! Touching the ceiling is currently a **silent stop** (keep going with the
//! last stable state). Prefer diagnosing via debug asserts / future ICE policy
//! rather than scattering fresh magic numbers.

use crate::{INLINE_MAX_EXPAND_DEPTH, SPECIALIZE_CONST_MAX_CLONES_PER_FUN};

/// Float-ABI × mono clone alternation after lambda lift (`lower_hir`).
pub const FLOAT_MONO_ROUNDS: usize = 8;
/// Mono clone worklist rounds (`specialize_mono_calls`).
pub const MONO_CLONE_ROUNDS: usize = 8;
/// Float ABI / escape / similar change-flag loops.
pub const CHANGE_FLAG_ROUNDS: usize = 32;
/// Closure capture-type collection fixpoint (`closure_cap_tys`).
pub const CLOSURE_CAP_TY_ROUNDS: usize = 8;

/// Named bundle for audits / docs (values mirror the consts above).
pub struct FixpointCaps;

impl FixpointCaps {
    pub const FLOAT_MONO: usize = FLOAT_MONO_ROUNDS;
    pub const MONO_CLONE: usize = MONO_CLONE_ROUNDS;
    pub const CHANGE_FLAG: usize = CHANGE_FLAG_ROUNDS;
    pub const CLOSURE_CAP_TY: usize = CLOSURE_CAP_TY_ROUNDS;
}

const _: () = {
    assert!(FLOAT_MONO_ROUNDS > 0);
    assert!(MONO_CLONE_ROUNDS > 0);
    assert!(CHANGE_FLAG_ROUNDS >= FLOAT_MONO_ROUNDS);
    assert!(CLOSURE_CAP_TY_ROUNDS > 0);
    // Keep specialize/inline caps in the same policy neighborhood.
    assert!(SPECIALIZE_CONST_MAX_CLONES_PER_FUN >= MONO_CLONE_ROUNDS);
    assert!(INLINE_MAX_EXPAND_DEPTH >= 1);
};
