//! Value emission — builtin intrinsics (split by family).

mod adt;
mod io;
mod list;
mod map_set;
mod string;

use super::super::Codegen;
use anyhow::Result;
use inkwell::values::BasicValueEnum;
use lumia_core::Local;
use lumia_hir::Builtin;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_value_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::Println
            | Builtin::PrintlnInt
            | Builtin::PrintlnStr
            | Builtin::ReadStdin
            | Builtin::Assert
            | Builtin::MatchFail
            | Builtin::Show => self.emit_io_builtin(name, args),

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
            | Builtin::MapKeys
            | Builtin::MapValues
            | Builtin::MapItems
            | Builtin::Range
            | Builtin::RangeInclusive => self.emit_list_builtin(name, args),

            Builtin::Contains | Builtin::MapSet | Builtin::MapRemove | Builtin::SetInsert => {
                self.emit_map_set_builtin(name, args)
            }

            Builtin::StrTrim
            | Builtin::StrSplit
            | Builtin::StrSubstring
            | Builtin::StrToLower
            | Builtin::StrToUpper
            | Builtin::StrStartsWith
            | Builtin::StrEndsWith => self.emit_string_builtin(name, args),

            Builtin::AdtTag | Builtin::AdtField => self.emit_adt_builtin(name, args),
        }
    }
}
