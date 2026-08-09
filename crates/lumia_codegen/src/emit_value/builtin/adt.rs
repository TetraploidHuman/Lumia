//! Value emission — adt builtins.

use super::super::super::Codegen;
use anyhow::Result;
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
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let obj = self
                    .builder
                    .build_int_to_ptr(obj_i, ptr_ty, "adt_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_adt_tag").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[obj.into()], "adt_tag")
                    .unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            Builtin::AdtField => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let idx = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let obj = self
                    .builder
                    .build_int_to_ptr(obj_i, ptr_ty, "adt_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_adt_field").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[obj.into(), idx.into()], "adt_field")
                    .unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            _ => unreachable!("non-adt builtin in emit_adt_builtin"),
        }
    }
}
