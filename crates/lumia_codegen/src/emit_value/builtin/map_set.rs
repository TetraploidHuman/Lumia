//! Value emission — map_set builtins.

use super::super::super::Codegen;
use anyhow::Result;
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use lumia_core::Local;
use lumia_hir::Builtin;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_map_set_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::Contains => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let key = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let obj = self
                    .builder
                    .build_int_to_ptr(obj_i, ptr_ty, "col_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_contains").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[obj.into(), key.into()], "contains")
                    .unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            Builtin::MapSet => {
                let map_i = self.coerce_i64(self.local(args[0])?)?;
                let key = self.coerce_i64(self.local(args[1])?)?;
                let val = self.coerce_i64(self.local(args[2])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let mut map = self
                    .builder
                    .build_int_to_ptr(map_i, ptr_ty, "col_ptr")
                    .unwrap();
                if matches!(self.local_tys.get(&args[1].0), Some(Type::Float)) {
                    let ens = self.module.get_function("lumia_ensure_map_f64").unwrap();
                    map = self
                        .builder
                        .build_call(ens, &[map.into()], "ens_mf64")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                }
                if matches!(self.local_tys.get(&args[2].0), Some(Type::Float)) {
                    let ens = self.module.get_function("lumia_ensure_map_vf64").unwrap();
                    map = self
                        .builder
                        .build_call(ens, &[map.into()], "ens_mvf64")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                }
                let f = self.module.get_function("lumia_set").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[map.into(), key.into(), val.into()], "col_set")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "set_i64")
                    .unwrap()
                    .into())
            }
            Builtin::MapRemove => {
                let map_i = self.coerce_i64(self.local(args[0])?)?;
                let key = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let map = self
                    .builder
                    .build_int_to_ptr(map_i, ptr_ty, "col_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_remove").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[map.into(), key.into()], "col_rm")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "rm_i64")
                    .unwrap()
                    .into())
            }
            Builtin::SetInsert => {
                let set_i = self.coerce_i64(self.local(args[0])?)?;
                let elem = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let mut set = self
                    .builder
                    .build_int_to_ptr(set_i, ptr_ty, "set_ptr")
                    .unwrap();
                if matches!(self.local_tys.get(&args[1].0), Some(Type::Float)) {
                    let ens = self.module.get_function("lumia_ensure_set_f64").unwrap();
                    set = self
                        .builder
                        .build_call(ens, &[set.into()], "ens_sf64")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                }
                let f = self.module.get_function("lumia_set_insert").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[set.into(), elem.into()], "set_ins")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "set_ins_i64")
                    .unwrap()
                    .into())
            }
            _ => unreachable!("non-map_set builtin in emit_map_set_builtin"),
        }
    }
}
