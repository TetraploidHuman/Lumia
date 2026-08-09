//! Transparent result reuse (DESIGN §7.5).
//!
//! - **§7.5.1-A local**: CSE + const-fold / copy-prop + LICM (no `T_f`)
//! - **§7.5.1-B `T_f`**: bounded cross-call table; representation = slots | DenseInt

use lumia_core::CoreModule;

mod cse;
mod fold;
mod licm;
mod plan;

pub use cse::cse_module;
pub use plan::{apply_memo_plan, plan_memo_tf};

/// Planner-facing widths (IDs stored as `u32` on [`MemoTf`](lumia_core::MemoTf)).
pub const MEMO_L2_MAX_FUNS: u32 = lumia_abi::MEMO_L2_MAX_FUNS as u32;
pub const MEMO_IDX_MAX_FUNS: u32 = lumia_abi::MEMO_IDX_MAX_FUNS as u32;
/// Keys outside `0..MEMO_IDX_CAP` are never cached (DESIGN §7.5 hard bound).
pub const MEMO_IDX_CAP: u32 = lumia_abi::MEMO_IDX_CAP as u32;
pub use lumia_abi::MEMO_L2_SLOTS;
pub use lumia_abi::{
    MEMO_IDX_TABLE_BYTES, MEMO_L2_MAX_ARGS, MEMO_PROCESS_BYTE_CAP, MEMO_SLOTS_TABLE_BYTES,
};
/// DESIGN-facing aliases for the slots/`T_f` caps (same values as `MEMO_L2_*`).
pub use lumia_abi::{MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS};

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
