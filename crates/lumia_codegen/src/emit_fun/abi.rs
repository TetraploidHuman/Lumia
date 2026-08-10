//! C / runtime ABI argument and return conversions.

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum};
use inkwell::AddressSpace;
use lumia_core::Local;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    /// Coerce a Lumia local to a C ABI argument for `foreign` calls.
    pub(crate) fn emit_c_abi_arg(
        &mut self,
        local: Local,
        ty: &Type,
    ) -> Result<BasicMetadataValueEnum<'ctx>> {
        match ty {
            Type::Float => Ok(self.promote_f64(self.local(local)?)?.into()),
            Type::Bool => {
                let i = self.coerce_i64(self.local(local)?)?;
                Ok(crate::error::llvm(self.llvm.builder.build_int_truncate(
                    i,
                    self.llvm.context.i8_type(),
                    "c_bool",
                ))?
                .into())
            }
            Type::String => {
                let s_i = self.coerce_i64(self.local(local)?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let s =
                    crate::error::llvm(self.llvm.builder.build_int_to_ptr(s_i, ptr_ty, "cstr_in"))?;
                let f = self.runtime_fn("lumia_string_cstr")?;
                let call =
                    crate::error::llvm(self.llvm.builder.build_call(f, &[s.into()], "cstr"))?;
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value()
                    .into())
            }
            _ => Ok(self.coerce_i64(self.local(local)?)?.into()),
        }
    }

    pub(crate) fn restore_c_abi_ret(
        &self,
        fun: &str,
        call: inkwell::values::CallSiteValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        let ret = self.funs.fun_ret_tys.get(fun).cloned().unwrap_or(Type::Int);
        match ret {
            Type::Unit => Ok(self.llvm.i64_ty.const_int(0, false).into()),
            Type::Float => {
                let f = call
                    .try_as_basic_value()
                    .basic()
                    .context("foreign float return")?
                    .into_float_value();
                Ok(f.into())
            }
            Type::Bool => {
                let b = call
                    .try_as_basic_value()
                    .basic()
                    .context("foreign bool return")?
                    .into_int_value();
                Ok(crate::error::llvm(self.llvm.builder.build_int_z_extend(
                    b,
                    self.llvm.i64_ty,
                    "bool_i64",
                ))?
                .into())
            }
            Type::String => {
                let p = call
                    .try_as_basic_value()
                    .basic()
                    .context("foreign string return")?
                    .into_pointer_value();
                let f = self.runtime_fn("lumia_cstr_to_string")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[p.into()],
                    "cstr_to_str",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "str_ret",
                ))?
                .into())
            }
            _ => Ok(call
                .try_as_basic_value()
                .basic()
                .unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into())),
        }
    }

    /// `lumia_rt` object ABI: String/List as heap pointers (no cstr conversion).
    pub(crate) fn emit_runtime_abi_arg(
        &mut self,
        local: Local,
        ty: &Type,
    ) -> Result<BasicMetadataValueEnum<'ctx>> {
        match ty {
            Type::Float => Ok(self.promote_f64(self.local(local)?)?.into()),
            Type::String | Type::List(_) => {
                let s_i = self.coerce_i64(self.local(local)?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                Ok(
                    crate::error::llvm(self.llvm.builder.build_int_to_ptr(s_i, ptr_ty, "rt_obj"))?
                        .into(),
                )
            }
            _ => Ok(self.coerce_i64(self.local(local)?)?.into()),
        }
    }

    pub(crate) fn restore_runtime_abi_ret(
        &self,
        fun: &str,
        call: inkwell::values::CallSiteValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        let ret = self.funs.fun_ret_tys.get(fun).cloned().unwrap_or(Type::Int);
        match ret {
            Type::Unit => Ok(self.llvm.i64_ty.const_int(0, false).into()),
            Type::Float => {
                let f = call
                    .try_as_basic_value()
                    .basic()
                    .context("runtime float return")?
                    .into_float_value();
                Ok(f.into())
            }
            Type::String | Type::List(_) => {
                let p = call
                    .try_as_basic_value()
                    .basic()
                    .context("runtime object return")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    p,
                    self.llvm.i64_ty,
                    "rt_obj_ret",
                ))?
                .into())
            }
            _ => Ok(call
                .try_as_basic_value()
                .basic()
                .unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into())),
        }
    }
}
