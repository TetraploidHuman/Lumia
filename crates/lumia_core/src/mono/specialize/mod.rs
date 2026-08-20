mod collect;
mod forwarders;
mod funref;
mod ret_refresh;
mod rewrite;

use crate::ir::CoreModule;

pub(crate) use collect::mono_value_ty;

/// Scheme-driven monomorphization:
/// 1. **Collect clones** until fixed point (scan → clone worklist).
/// 2. **Rewrite** call sites to mangled clones (single pass).
/// 3. **Refresh** erased HOF return types from final bodies (single pass).
pub(crate) fn specialize_mono_calls(module: &mut CoreModule) -> bool {
    let renames = collect::collect_mono_clones_until_fixed_point(module);
    if renames.is_empty() {
        return false;
    }
    rewrite::rewrite_all_mono_call_sites(module, &renames);
    // Residual `Call(generic, …)` (missed rewrite) must not emit Int `*` on IEEE
    // bits — upgrade erased formals from Float/List[Float] clones.
    ret_refresh::upgrade_generic_param_tys_from_clones(module);
    // After all clones exist, upgrade erased Int rets on HOF wrappers whose
    // bodies now `Call(dbl$Float, …)` (directize order within a round varies).
    ret_refresh::refresh_erased_mono_return_types(module);
    // Toehold: thin FunRef wrappers that only forward to a concrete Call share
    // that target at call sites (avoid an extra frame / duplicate body emit).
    forwarders::elide_trivial_mono_forwarders(module);
    true
}
