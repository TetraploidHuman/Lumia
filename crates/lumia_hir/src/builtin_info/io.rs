use super::{bi, BuiltinEffect, BuiltinEmit, BuiltinInfo, ResultHeap, NO_F};
use crate::ast::{Builtin, BuiltinFamily};

pub(crate) fn info_io(b: Builtin) -> BuiltinInfo {
    use Builtin::*;
    use BuiltinEmit::*;
    use ResultHeap::*;
    let pure = BuiltinEffect::Pure;
    let io = BuiltinEffect::Io;
    let f = BuiltinFamily::Io;
    match b {
        // Show: may_capture false; escape seed treats Show specially for rooting.
        Println => bi(f, 1, 1, io, None, NO_F, Custom, true, Never),
        Show => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_show"),
            NO_F,
            Custom,
            false,
            Always,
        ),
        ReadStdin => bi(
            f,
            0,
            0,
            io,
            Some("lumia_read_stdin"),
            NO_F,
            NullaryPtr,
            true,
            Always,
        ),
        MatchFail => bi(
            f,
            0,
            0,
            pure,
            Some("lumia_match_fail"),
            NO_F,
            NullaryVoid,
            false,
            Never,
        ),
        Assert => bi(
            f,
            1,
            2,
            pure,
            Some("lumia_assert"),
            NO_F,
            Custom,
            false,
            Never,
        ),
        _ => unreachable!("info_io: {b:?}"),
    }
}
