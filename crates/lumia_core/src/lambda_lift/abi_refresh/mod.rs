//! After mono, refresh Float ABI for closure captures via typed local inference.
//!
//! Lift runs before mono, so nested `{ x -> x + k }` inside `make(k)` still
//! sees a generic `k`. Once `make$Float` exists, capture slots are Float;
//! codegen loads via typed `closure_cap_tys`.

mod float_caps;
mod fold_list;
mod local_lookup;
mod ret_refresh;

use float_caps::{scan_alloc_closure_caps, seed_float_locals_from_cap_indices};
use fold_list::upgrade_captured_list_fold_float;
use ret_refresh::{refresh_alloc_closure_fun_rets, refresh_lifted_lambda_rets};

use crate::ir::CoreModule;
use lumia_syntax::Sym;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(crate) fn fixup_closure_float_caps(module: &mut CoreModule) {
    let lifted = super::lifted_lambda_names(module);
    super::with_lifted_lambda_names(lifted, || fixup_closure_float_caps_inner(module));
}

fn fixup_closure_float_caps_inner(module: &mut CoreModule) {
    let tables = crate::ModuleTables::from_module(module);
    let fun_ret_tys = &tables.fun_ret_tys;
    let fun_param_tys = &tables.fun_param_tys;

    // (lifted_fun, capture_index) → must be float (from AllocClosure-site tys)
    let mut need_float: HashSet<(String, u32)> = HashSet::default();
    for fun in &module.functions {
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        for (p, ty) in fun.params.iter().zip(fun.param_tys.iter()) {
            local_tys.insert(p.0, ty.clone());
        }
        let mut slot_tys: HashMap<Sym, Type> = HashMap::default();
        scan_alloc_closure_caps(
            &fun.body,
            &mut local_tys,
            &mut slot_tys,
            fun_ret_tys,
            fun_param_tys,
            &mut need_float,
        );
    }

    if !need_float.is_empty() {
        for fun in &mut module.functions {
            let indices: HashSet<u32> = need_float
                .iter()
                .filter(|(n, _)| n == &fun.name)
                .map(|(_, i)| *i)
                .collect();
            if indices.is_empty() {
                continue;
            }
            // Seed float locals from typed float cap indices.
            let seed = seed_float_locals_from_cap_indices(&fun.body, &indices);
            if fun.params.len() > 1 {
                let user: Vec<_> = fun.params[1..].to_vec();
                let float_ps =
                    super::float_abi::params_used_as_float_seeded(&fun.body, &user, &seed);
                for (i, p) in user.iter().enumerate() {
                    if float_ps.contains(&p.0) {
                        fun.param_tys[i + 1] = Type::Float;
                    }
                }
            }
            if super::float_abi::block_result_is_float_seeded(&fun.body, fun_ret_tys, &seed) {
                fun.ret_ty = Type::Float;
            }
        }
    }

    // Always refresh Fun/spawn rets after directize/mono — even when no
    // float env caps (`var f = {…}; f(1.5)` has no float env caps).
    refresh_alloc_closure_fun_rets(module);
    refresh_lifted_lambda_rets(module);
    upgrade_captured_list_fold_float(module);
}

#[cfg(test)]
#[path = "../float_cap_fixup_tests.rs"]
mod float_cap_fixup_tests;
