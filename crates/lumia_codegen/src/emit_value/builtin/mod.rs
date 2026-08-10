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
use lumia_hir::{Builtin, BuiltinFamily};

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_value_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name.family() {
            BuiltinFamily::Io => self.emit_io_builtin(name, args),
            BuiltinFamily::List => self.emit_list_builtin(name, args),
            BuiltinFamily::MapSet => self.emit_map_set_builtin(name, args),
            BuiltinFamily::String => self.emit_string_builtin(name, args),
            BuiltinFamily::Adt => self.emit_adt_builtin(name, args),
        }
    }
}
