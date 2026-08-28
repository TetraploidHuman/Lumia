//! Builtin pass inventory — identity + IR annotation contracts.
//!
//! Adding a pass: implement `Pass`, add a `PipelinePass` arm, append a schedule
//! slot, and register a [`PassInfo`] here. Removing: drop the schedule slot and
//! this entry; consumers must already treat missing annotations as defaults.

use crate::pipeline::{IrAnno, PassInfo, PassKind, PassStage};

pub const MEMO_TF: PassInfo = PassInfo {
    id: "memo_tf",
    kind: PassKind::Plan,
    stage: PassStage::Plan,
    requires: &[],
    writes: &[IrAnno::Memo],
    reads: &[IrAnno::FunBodies],
};

pub const CSE: PassInfo = PassInfo {
    id: "cse",
    kind: PassKind::Transform,
    stage: PassStage::LocalReuse,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    reads: &[],
};

pub const CONST_FOLD: PassInfo = PassInfo {
    id: "const_fold",
    kind: PassKind::Transform,
    stage: PassStage::LocalReuse,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    reads: &[],
};

pub const SPECIALIZE_CONST: PassInfo = PassInfo {
    id: "specialize_const",
    kind: PassKind::Transform,
    stage: PassStage::Specialize,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    reads: &[],
};

pub const LICM: PassInfo = PassInfo {
    id: "licm",
    kind: PassKind::Transform,
    stage: PassStage::Loop,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    reads: &[],
};

pub const DENSE_F64_SR: PassInfo = PassInfo {
    id: "dense_f64_sr",
    kind: PassKind::Transform,
    stage: PassStage::PatternSr,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    reads: &[],
};

pub const INLINE: PassInfo = PassInfo {
    id: "inline",
    kind: PassKind::Transform,
    stage: PassStage::Size,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    // Skips callees with `memo.is_some()` when present.
    reads: &[IrAnno::Memo, IrAnno::FunBodies],
};

pub const ESCAPE: PassInfo = PassInfo {
    id: "escape",
    kind: PassKind::Analysis,
    stage: PassStage::Escape,
    requires: &[],
    writes: &[IrAnno::Escaping],
    reads: &[IrAnno::FunBodies],
};

pub const CONCAT_IDENT: PassInfo = PassInfo {
    id: "concat_ident",
    kind: PassKind::Transform,
    stage: PassStage::CleanupMid,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    reads: &[],
};

pub const REPR_SELECT: PassInfo = PassInfo {
    id: "repr_select",
    kind: PassKind::Transform,
    stage: PassStage::Repr,
    // Lit*/Small* specialization is unsound without escape facts.
    requires: &["escape"],
    writes: &[IrAnno::Repr],
    reads: &[IrAnno::Escaping, IrAnno::FunBodies],
};

pub const COPY_ELIM: PassInfo = PassInfo {
    id: "copy_elim",
    kind: PassKind::Transform,
    stage: PassStage::CleanupLate,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    reads: &[],
};

/// All builtin passes (order is documentation only; schedules own run order).
pub const ALL: &[PassInfo] = &[
    MEMO_TF,
    CSE,
    CONST_FOLD,
    SPECIALIZE_CONST,
    LICM,
    DENSE_F64_SR,
    INLINE,
    ESCAPE,
    CONCAT_IDENT,
    REPR_SELECT,
    COPY_ELIM,
];

pub fn info(id: &str) -> Option<&'static PassInfo> {
    ALL.iter().find(|p| p.id == id)
}
