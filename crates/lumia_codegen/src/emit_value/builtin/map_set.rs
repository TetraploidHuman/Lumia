//! Value emission — map_set builtins.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
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
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let obj = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(obj_i, ptr_ty, "col_ptr"),
                )?;
                let f = self.runtime_fn("lumia_contains")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[obj.into(), key.into()],
                    "contains",
                ))?;
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?)
            }
            Builtin::MapSet => {
                let map_i = self.coerce_i64(self.local(args[0])?)?;
                let key = self.coerce_i64(self.local(args[1])?)?;
                let val = self.coerce_i64(self.local(args[2])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let mut map = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(map_i, ptr_ty, "col_ptr"),
                )?;
                if matches!(self.frame.local_tys.get(&args[1].0), Some(Type::Float)) {
                    let ens = self.runtime_fn(lumia_abi::ENSURE_MAP_F64)?;
                    map = crate::error::llvm(self.llvm.builder.build_call(
                        ens,
                        &[map.into()],
                        "ens_mf64",
                    ))?
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                }
                if matches!(self.frame.local_tys.get(&args[2].0), Some(Type::Float)) {
                    let ens = self.runtime_fn(lumia_abi::ENSURE_MAP_VF64)?;
                    map = crate::error::llvm(self.llvm.builder.build_call(
                        ens,
                        &[map.into()],
                        "ens_mvf64",
                    ))?
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                }
                let f = self.runtime_fn("lumia_set")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[map.into(), key.into(), val.into()],
                    "col_set",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "set_i64",
                ))?
                .into())
            }
            Builtin::MapRemove => {
                let map_i = self.coerce_i64(self.local(args[0])?)?;
                let key = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let map = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(map_i, ptr_ty, "col_ptr"),
                )?;
                let f = self.runtime_fn("lumia_remove")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[map.into(), key.into()],
                    "col_rm",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "rm_i64",
                ))?
                .into())
            }
            Builtin::SetInsert => {
                let set_i = self.coerce_i64(self.local(args[0])?)?;
                let elem = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let mut set = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(set_i, ptr_ty, "set_ptr"),
                )?;
                if matches!(self.frame.local_tys.get(&args[1].0), Some(Type::Float)) {
                    let ens = self.runtime_fn(lumia_abi::ENSURE_SET_F64)?;
                    set = crate::error::llvm(self.llvm.builder.build_call(
                        ens,
                        &[set.into()],
                        "ens_sf64",
                    ))?
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                }
                let f = self.runtime_fn("lumia_set_insert")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[set.into(), elem.into()],
                    "set_ins",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "set_ins_i64",
                ))?
                .into())
            }
            Builtin::MapKeys | Builtin::MapValues | Builtin::MapItems => {
                let map_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let map = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(map_i, ptr_ty, "map_ptr"),
                )?;
                let fname = name
                    .info()
                    .runtime_symbol
                    .context("MapKeys/Values/Items have runtime symbols")?;
                let f = self.runtime_fn(fname)?;
                let call =
                    crate::error::llvm(self.llvm.builder.build_call(f, &[map.into()], "map_kv"))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "map_kv_i64",
                ))?
                .into())
            }
            _ => unreachable!("non-map_set builtin in emit_map_set_builtin"),
        }
    }
}
