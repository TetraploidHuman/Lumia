//! Builtin pass inventory — identity + IR annotation contracts.
//!
//! Adding a pass: implement `Pass`, add a `PipelinePass` arm, append a schedule
//! slot, and register a [`PassInfo`] here. Removing: drop the schedule slot and
//! this entry; consumers must already treat missing annotations as defaults.
//!
//! Optional passes are behind Cargo features (`opt-memo`, `opt-dense-f64`,
//! `opt-inline`, `opt-repr-stack`).

use crate::pipeline::{IrAnno, PassInfo, PassKind, PassStage};

#[cfg(feature = "opt-memo")]
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

#[cfg(feature = "opt-dense-f64")]
pub const DENSE_F64_SR: PassInfo = PassInfo {
    id: "dense_f64_sr",
    kind: PassKind::Transform,
    stage: PassStage::PatternSr,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    reads: &[],
};

#[cfg(feature = "opt-inline")]
pub const INLINE: PassInfo = PassInfo {
    id: "inline",
    kind: PassKind::Transform,
    stage: PassStage::Size,
    requires: &[],
    writes: &[IrAnno::FunBodies],
    // Skips callees with `memo.is_some()` when present.
    reads: &[IrAnno::Memo, IrAnno::FunBodies],
};

#[cfg(feature = "opt-repr-stack")]
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

#[cfg(feature = "opt-repr-stack")]
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

/// All builtin passes enabled in this build (order is documentation only).
pub const ALL: &[PassInfo] = &[
    #[cfg(feature = "opt-memo")]
    MEMO_TF,
    CSE,
    CONST_FOLD,
    SPECIALIZE_CONST,
    LICM,
    #[cfg(feature = "opt-dense-f64")]
    DENSE_F64_SR,
    #[cfg(feature = "opt-inline")]
    INLINE,
    #[cfg(feature = "opt-repr-stack")]
    ESCAPE,
    CONCAT_IDENT,
    #[cfg(feature = "opt-repr-stack")]
    REPR_SELECT,
    COPY_ELIM,
];

pub fn info(id: &str) -> Option<&'static PassInfo> {
    ALL.iter().find(|p| p.id == id)
}
