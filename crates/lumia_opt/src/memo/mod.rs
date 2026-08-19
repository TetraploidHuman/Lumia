//! Transparent result reuse family (DESIGN §7.5) — not a single pass.
//!
//! This module groups the §7.5 "reuse" pipeline pieces that share PE / scalar
//! environments and the `T_f` planner:
//!
//! - **§7.5.1-A local**: [`cse`], const-fold / copy-prop ([`fold`]), [`licm`]
//! - **§7.5.1-B `T_f`**: bounded cross-call table ([`plan`]); representation = slots | DenseInt
//!
//! Pipeline orchestration (pass order, Release interleaving) stays in `lumia_opt::optimize`.

use lumia_core::CoreModule;

mod cse;
mod fold;
mod licm;
mod plan;

pub(crate) use cse::cse_module;
pub(crate) use plan::{apply_memo_plan, plan_memo_tf};

/// Planner-facing widths (IDs stored as `u32` on [`MemoTf`](lumia_core::MemoTf)).
pub(crate) const MEMO_TF_MAX_FUNS_U32: u32 = lumia_abi::MEMO_TF_MAX_FUNS as u32;
pub(crate) const MEMO_IDX_MAX_FUNS: u32 = lumia_abi::MEMO_IDX_MAX_FUNS as u32;

pub(crate) use fold::{const_fold_block, copy_prop_block};
pub(crate) use licm::licm_seeded;

/// Local const-fold + copy-prop (DESIGN §7.5.1-A).
pub struct ConstFoldPass;
impl ConstFoldPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        for f in &mut module.functions {
            const_fold_block(&mut f.body);
            copy_prop_block(&mut f.body);
        }
    }
}

/// Loop-invariant code motion (DESIGN §7.5.1-A).
pub struct LicmPass;
impl LicmPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        use lumia_ty::Type;
        use rustc_hash::FxHashSet as HashSet;
        for f in &mut module.functions {
            let mut float_locals = HashSet::default();
            for (i, ty) in f.param_tys.iter().enumerate() {
                if matches!(ty, Type::Float) {
                    if let Some(p) = f.params.get(i) {
                        float_locals.insert(p.0);
                    }
                }
            }
            // Seed from body Float defs, then hoist with that set.
            licm_seeded(&mut f.body, float_locals);
        }
    }
}

#[cfg(test)]
mod tests;
