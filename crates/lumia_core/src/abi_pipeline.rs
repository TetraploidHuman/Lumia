//! Post-lower Core ABI pipeline (not HIR translation).
//!
//! Lift → channel hint → directize → traits → float fixup × mono fixpoint →
//! trait stubs. [`lower_hir_with_schemes`](crate::lower_hir_with_schemes) stops
//! at raw Core; public compile entries call this next.

use crate::ir::CoreModule;
use crate::lambda_lift::{fixup_closure_float_caps, lift_lambdas, refine_channel_elem_hint};
use crate::mono::{
    directize_funref_calls, ensure_trait_method_stubs, resolve_trait_method_calls,
    specialize_mono_calls,
};

/// Post-lower Core ABI pipeline: lift → channel hint → directize → traits →
/// float fixup × mono fixpoint → trait stubs.
pub fn run_core_abi_pipeline(core: &mut CoreModule) {
    lift_lambdas(core);
    refine_channel_elem_hint(core);
    directize_funref_calls(core);
    // Num `a + b` is still Binary until here — rewrite to `__Num_T_add` Call
    // before mono so Float field products get `$…Float…` clones (codegen
    // override alone hits the unspecialized Int-body instance).
    resolve_trait_method_calls(core);
    // Fixpoint: fixup lifts Float/Bool/String/Fun ABI on `__lam_*`; mono clones
    // HOF consumers (`unwrapOr` after `optionMap`, spawn join, …). Change-flag
    // until specialize reports no new clones (capped). One more fixup after the
    // last mono pass patches caps once `$Float` clones exist.
    for _ in 0..lumia_abi::FLOAT_MONO_ROUNDS {
        fixup_closure_float_caps(core);
        if !specialize_mono_calls(core) {
            break;
        }
    }
    fixup_closure_float_caps(core);
    ensure_trait_method_stubs(core);
}
