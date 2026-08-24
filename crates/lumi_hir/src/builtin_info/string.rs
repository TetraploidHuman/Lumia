use super::{bi, BuiltinEffect, BuiltinEmit, BuiltinInfo, ResultHeap, NO_F};
use crate::ast::{Builtin, BuiltinFamily};

pub(crate) fn info_string(b: Builtin) -> BuiltinInfo {
    use Builtin::*;
    use BuiltinEmit::*;
    use ResultHeap::*;
    let pure = BuiltinEffect::Pure;
    let f = BuiltinFamily::String;
    match b {
        StrTrim => bi(
            f,
            1,
            1,
            pure,
            Some("lumi_str_trim"),
            NO_F,
            UnaryObjPtr,
            false,
            Always,
        ),
        StrSplit => bi(
            f,
            2,
            2,
            pure,
            Some("lumi_str_split"),
            NO_F,
            ObjI64Ptr,
            false,
            Always,
        ),
        StrSubstring => bi(
            f,
            3,
            3,
            pure,
            Some("lumi_str_substring"),
            NO_F,
            ObjI64I64Ptr,
            false,
            Always,
        ),
        StrToLower => bi(
            f,
            1,
            1,
            pure,
            Some("lumi_str_to_lower"),
            NO_F,
            UnaryObjPtr,
            false,
            Always,
        ),
        StrToUpper => bi(
            f,
            1,
            1,
            pure,
            Some("lumi_str_to_upper"),
            NO_F,
            UnaryObjPtr,
            false,
            Always,
        ),
        StrStartsWith => bi(
            f,
            2,
            2,
            pure,
            Some("lumi_str_starts_with"),
            NO_F,
            ObjObjScalar,
            false,
            Never,
        ),
        StrEndsWith => bi(
            f,
            2,
            2,
            pure,
            Some("lumi_str_ends_with"),
            NO_F,
            ObjObjScalar,
            false,
            Never,
        ),
        _ => unreachable!("info_string: {b:?}"),
    }
}
