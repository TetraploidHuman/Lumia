//! Transparent result reuse family (DESIGN §7.5) — not a single pass.
//!
//! This module groups the §7.5 "reuse" pipeline pieces that share PE / scalar
//! environments and the `T_f` planner:
//!
//! - **§7.5.1-A local**: [`cse`], const-fold / copy-prop ([`fold`]), [`licm`]
//! - **§7.5.1-B `T_f`**: bounded cross-call table ([`plan`]); representation = slots | DenseInt
//!
//! Pipeline orchestration (pass order, Release interleaving) stays in
//! [`crate::pipeline`] / [`crate::optimize`].

use lumi_core::CoreModule;

mod cse;
mod fold;
mod licm;
#[cfg(feature = "opt-memo")]
mod plan;

pub use cse::cse_module;

#[cfg(feature = "opt-memo")]
pub use plan::{apply_memo_plan, plan_memo_tf};

#[cfg(not(feature = "opt-memo"))]
pub fn plan_memo_tf(
    _module: &lumi_core::CoreModule,
    _prefer_dense: bool,
) -> rustc_hash::FxHashMap<String, lumi_core::MemoTf> {
    rustc_hash::FxHashMap::default()
}

#[cfg(not(feature = "opt-memo"))]
pub fn apply_memo_plan(
    _module: &mut lumi_core::CoreModule,
    _plan: &rustc_hash::FxHashMap<String, lumi_core::MemoTf>,
) {
}

/// Planner-facing widths (IDs stored as `u32` on [`MemoTf`](lumi_core::MemoTf)).
#[cfg(feature = "opt-memo")]
pub const MEMO_TF_MAX_FUNS_U32: u32 = lumi_abi::MEMO_TF_MAX_FUNS as u32;
pub const MEMO_IDX_MAX_FUNS: u32 = lumi_abi::MEMO_IDX_MAX_FUNS as u32;
/// Keys outside `0..MEMO_IDX_CAP` are never cached (DESIGN §7.5 hard bound).
pub const MEMO_IDX_CAP: u32 = lumi_abi::MEMO_IDX_CAP as u32;
pub use lumi_abi::{
    MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP, MEMO_SLOTS_TABLE_BYTES, MEMO_TF_MAX_ARGS,
    MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS,
};

pub(crate) use fold::{const_fold_block, copy_prop_block};
pub(crate) use licm::licm_block;

/// Local const-fold + copy-prop (DESIGN §7.5.1-A).
pub struct ConstFoldPass;
impl crate::Pass for ConstFoldPass {
    fn name(&self) -> &str {
        "const_fold"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            const_fold_block(&mut f.body);
            copy_prop_block(&mut f.body);
        }
    }
}

/// Loop-invariant code motion (DESIGN §7.5.1-A).
pub struct LicmPass;
impl crate::Pass for LicmPass {
    fn name(&self) -> &str {
        "licm"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            licm_block(&mut f.body);
        }
    }
}

#[cfg(test)]
mod tests;
