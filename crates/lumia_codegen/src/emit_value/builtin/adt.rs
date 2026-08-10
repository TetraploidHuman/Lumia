//! Value emission — adt builtins.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use lumia_core::Local;
use lumia_hir::Builtin;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_adt_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::AdtTag => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let obj = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(obj_i, ptr_ty, "adt_ptr"),
                )?;
                let f = self.runtime_fn("lumia_adt_tag")?;
                let call =
                    crate::error::llvm(self.llvm.builder.build_call(f, &[obj.into()], "adt_tag"))?;
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?)
            }
            Builtin::AdtField => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let idx = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let obj = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(obj_i, ptr_ty, "adt_ptr"),
                )?;
                let f = self.runtime_fn("lumia_adt_field")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[obj.into(), idx.into()],
                    "adt_field",
                ))?;
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?)
            }
            _ => unreachable!("non-adt builtin in emit_adt_builtin"),
        }
    }
}
