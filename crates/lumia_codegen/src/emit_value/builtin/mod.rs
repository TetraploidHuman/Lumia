//! Value emission — builtin intrinsics (split by family).

mod convention;
mod io;
mod list;
pub(crate) mod show;
mod task;

use super::super::Codegen;
use anyhow::Result;
use inkwell::values::BasicValueEnum;
use lumia_core::Local;
use lumia_hir::{Builtin, BuiltinEmit, BuiltinFamily};

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_value_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        let emit = name.info().emit;
        if emit != BuiltinEmit::Custom {
            return self.emit_by_convention(name, args, emit);
        }
        match name.family() {
            BuiltinFamily::Io => self.emit_io_builtin(name, args),
            BuiltinFamily::List => self.emit_list_builtin(name, args),
            BuiltinFamily::Task => self.emit_task_builtin(name, args),
            BuiltinFamily::MapSet | BuiltinFamily::String | BuiltinFamily::Adt => unreachable!(
                "builtin `{}` marked Custom but family has no hand-written emit",
                name.display_name()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Codegen;
    use lumia_hir::Builtin;

    #[test]
    fn string_receiver_overrides_delegate_to_builtin() {
        assert_eq!(
            Codegen::string_receiver_rt_override(Builtin::ListReverse),
            Builtin::ListReverse.string_receiver_rt_override()
        );
        assert_eq!(
            Codegen::list_receiver_rt_override(Builtin::ListGet),
            Builtin::ListGet.list_receiver_rt_override()
        );
    }
}
