//! BuiltinCall typing (split by family for maintainability).

mod adt;
mod io;
mod list;
mod map_set;
mod string;

use super::Infer;
use crate::types::{Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(crate) fn infer_builtin_call(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::Println
            | Builtin::PrintlnInt
            | Builtin::PrintlnStr
            | Builtin::ReadStdin
            | Builtin::Assert
            | Builtin::MatchFail
            | Builtin::Show => self.infer_io_builtin(name, args, span),

            Builtin::ListLen
            | Builtin::ListGet
            | Builtin::ListSlice
            | Builtin::ListAppend
            | Builtin::ListConcat
            | Builtin::ListTake
            | Builtin::ListReverse
            | Builtin::ListSort
            | Builtin::ListSortByKeys
            | Builtin::ListParMap
            | Builtin::ListParFold
            | Builtin::ListJoin
            | Builtin::Elems
            | Builtin::Range
            | Builtin::RangeInclusive => self.infer_list_builtin(name, args, span),

            Builtin::Contains
            | Builtin::MapSet
            | Builtin::MapRemove
            | Builtin::SetInsert
            | Builtin::MapKeys
            | Builtin::MapValues
            | Builtin::MapItems => self.infer_map_set_builtin(name, args, span),

            Builtin::StrTrim
            | Builtin::StrSplit
            | Builtin::StrSubstring
            | Builtin::StrToLower
            | Builtin::StrToUpper
            | Builtin::StrStartsWith
            | Builtin::StrEndsWith => self.infer_string_builtin(name, args, span),

            Builtin::AdtTag | Builtin::AdtField => self.infer_adt_builtin(name, args, span),
        }
    }
}
