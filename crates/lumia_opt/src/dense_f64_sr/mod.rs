//! Rewrite dense `List[Float]` helpers to `lumia_f64_*` foreign calls (before Inline).
//!
//! **Sole owner of nest pattern matching.** Codegen only recognizes the rewritten
//! single-`Call` body and emits a frameless RT trampoline.
//!
//! Name/const/IV peeps share [`lumia_core::name_of`] / [`lumia_core::is_unit_inc`] /
//! [`lumia_core::is_nontrivial_arith`] / [`lumia_core::same_local`] /
//! [`lumia_core::header_lt_bound`] / [`lumia_core::is_list_get`] with codegen domain SRs.
//!
//! Whole-function patterns become a single `Call` so Release inlining places the
//! RT kernel at the call site (same shape as `extras.linalg` wrappers).
//!
//! Covered: gemv/gemvT/addmm/axpy/sub/add/mul/clamp/scale/fill/copy/zeros,
//! plus sumSq/mean/std/l2Norm/l2Normalize/softMax (scalar `sqrtF`/`expF` foreign
//! calls unlock the latter norms).

mod blas_shape;
mod externs;
mod match_blas;
mod match_elem;
mod match_norm;
mod shape_util;

// Re-export for `dense_f64_sr_tests` (`use super::external_sig`).
#[cfg(test)]
#[allow(unused_imports)]
use externs::external_sig;

use externs::{ensure_external, rewrite_body_to_call};
use lumia_core::{collect_leaf_defs, CoreModule};
use match_blas::{
    match_add_fun, match_addmm_fun, match_axpy_fun, match_gemv_fun, match_gemv_t_fun,
    match_mul_fun, match_sub_fun,
};
use match_elem::{
    match_clamp_fun, match_copy_fun, match_fill_fun, match_scale_fun, match_zeros_fun,
};
use match_norm::{
    match_l2_norm_fun, match_l2_normalize_fun, match_mean_fun, match_softmax_fun, match_std_fun,
    match_sum_sq_fun,
};
use rustc_hash::FxHashSet as HashSet;

pub struct DenseF64SrPass;

impl DenseF64SrPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        dense_f64_sr_module(module);
    }
}

fn dense_f64_sr_module(module: &mut CoreModule) {
    let mut rewrites: Vec<(usize, &'static str)> = Vec::new();
    for (i, fun) in module.functions.iter().enumerate() {
        if fun.external.is_some() || fun.is_main || fun.memo.is_some() {
            continue;
        }
        let defs = collect_leaf_defs(&fun.body, true);
        let sym = if match_gemv_fun(fun, &defs).is_some() {
            Some("lumia_f64_gemv")
        } else if match_gemv_t_fun(fun, &defs).is_some() {
            Some("lumia_f64_gemv_t")
        } else if match_addmm_fun(fun, &defs).is_some() {
            Some("lumia_f64_addmm")
        } else if match_axpy_fun(fun, &defs).is_some() {
            Some("lumia_f64_axpy")
        } else if match_sub_fun(fun, &defs).is_some() {
            Some("lumia_f64_sub")
        } else if match_add_fun(fun, &defs).is_some() {
            Some("lumia_f64_add")
        } else if match_mul_fun(fun, &defs).is_some() {
            Some("lumia_f64_mul")
        } else if match_clamp_fun(fun, &defs).is_some() {
            Some("lumia_f64_clamp")
        } else if match_scale_fun(fun, &defs).is_some() {
            Some("lumia_f64_scale")
        } else if match_fill_fun(fun, &defs).is_some() {
            Some("lumia_f64_fill")
        } else if match_copy_fun(fun, &defs).is_some() {
            Some("lumia_f64_copy")
        } else if match_zeros_fun(fun, &defs).is_some() {
            Some("lumia_list_f64_zeros")
        } else if match_l2_normalize_fun(fun, &defs).is_some() {
            Some("lumia_f64_l2_normalize")
        } else if match_softmax_fun(fun, &defs).is_some() {
            Some("lumia_f64_softmax")
        } else if match_l2_norm_fun(fun, &defs).is_some() {
            Some("lumia_f64_l2_norm")
        } else if match_std_fun(fun, &defs).is_some() {
            Some("lumia_f64_std")
        } else if match_sum_sq_fun(fun, &defs).is_some() {
            Some("lumia_f64_sum_sq")
        } else if match_mean_fun(fun, &defs).is_some() {
            Some("lumia_f64_mean")
        } else {
            None
        };
        if let Some(s) = sym {
            rewrites.push((i, s));
        }
    }
    if rewrites.is_empty() {
        return;
    }
    let mut need: HashSet<&'static str> = HashSet::default();
    for &(_, s) in &rewrites {
        need.insert(s);
    }
    for sym in need {
        ensure_external(module, sym);
    }
    for (i, sym) in rewrites {
        rewrite_body_to_call(&mut module.functions[i], sym);
    }
}

#[cfg(test)]
#[path = "../dense_f64_sr_tests.rs"]
mod tests;
