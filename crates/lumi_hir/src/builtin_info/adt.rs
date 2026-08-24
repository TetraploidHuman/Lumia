use super::{bi, BuiltinEffect, BuiltinEmit, BuiltinInfo, ResultHeap, NO_F};
use crate::ast::{Builtin, BuiltinFamily};

pub(crate) fn info_adt(b: Builtin) -> BuiltinInfo {
    use Builtin::*;
    use BuiltinEmit::*;
    use ResultHeap::*;
    let pure = BuiltinEffect::Pure;
    let f = BuiltinFamily::Adt;
    match b {
        AdtTag => bi(
            f,
            1,
            1,
            pure,
            Some("lumi_adt_tag"),
            NO_F,
            UnaryObjScalar,
            false,
            Never,
        ),
        // HIR may pass a 3rd name-hint arg; Core strips it before codegen.
        AdtField => bi(
            f,
            2,
            3,
            pure,
            Some("lumi_adt_field"),
            NO_F,
            ObjI64Scalar,
            false,
            Typed,
        ),
        _ => unreachable!("info_adt: {b:?}"),
    }
}
