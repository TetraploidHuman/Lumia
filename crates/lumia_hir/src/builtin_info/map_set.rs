use super::{
    bi, BuiltinEffect, BuiltinEmit, BuiltinInfo, ResultHeap, ENS_MAP_SET, ENS_SET_INSERT, NO_F,
};
use crate::ast::{Builtin, BuiltinFamily};

pub(crate) fn info_map_set(b: Builtin) -> BuiltinInfo {
    use Builtin::*;
    use BuiltinEmit::*;
    use ResultHeap::*;
    let pure = BuiltinEffect::Pure;
    let f = BuiltinFamily::MapSet;
    match b {
        Contains => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_contains"),
            NO_F,
            ObjI64Scalar,
            false,
            Never,
        ),
        MapSet => bi(
            f,
            3,
            3,
            pure,
            Some("lumia_set"),
            ENS_MAP_SET,
            ObjI64I64Ptr,
            true,
            Always,
        ),
        MapRemove => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_remove"),
            NO_F,
            ObjI64Ptr,
            true,
            Always,
        ),
        SetInsert => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_set_insert"),
            ENS_SET_INSERT,
            ObjI64Ptr,
            true,
            Always,
        ),
        MapKeys => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_map_keys"),
            NO_F,
            UnaryObjPtr,
            true,
            Always,
        ),
        MapValues => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_map_values"),
            NO_F,
            UnaryObjPtr,
            true,
            Always,
        ),
        MapItems => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_map_items"),
            NO_F,
            UnaryObjPtr,
            true,
            Always,
        ),
        _ => unreachable!("info_map_set: {b:?}"),
    }
}
