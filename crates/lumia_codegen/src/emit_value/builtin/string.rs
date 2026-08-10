//! Value emission — string builtins.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use lumia_core::Local;
use lumia_hir::Builtin;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_string_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::StrTrim | Builtin::StrToLower | Builtin::StrToUpper => {
                let s_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let s = crate::error::llvm(self.llvm.builder.build_int_to_ptr(s_i, ptr_ty, "str"))?;
                let fname = name
                    .info()
                    .runtime_symbol
                    .context("string unary builtins have runtime symbols")?;
                let f = self.runtime_fn(fname)?;
                let call =
                    crate::error::llvm(self.llvm.builder.build_call(f, &[s.into()], "str_op"))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "str_i64",
                ))?
                .into())
            }
            Builtin::StrSplit => {
                let s_i = self.coerce_i64(self.local(args[0])?)?;
                let sep = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let s = crate::error::llvm(self.llvm.builder.build_int_to_ptr(s_i, ptr_ty, "str"))?;
                let f = self.runtime_fn(
                    Builtin::StrSplit
                        .info()
                        .runtime_symbol
                        .context("runtime symbol")?,
                )?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[s.into(), sep.into()],
                    "split",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "split_i64",
                ))?
                .into())
            }
            Builtin::StrSubstring => {
                let s_i = self.coerce_i64(self.local(args[0])?)?;
                let a = self.coerce_i64(self.local(args[1])?)?;
                let b = self.coerce_i64(self.local(args[2])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let s = crate::error::llvm(self.llvm.builder.build_int_to_ptr(s_i, ptr_ty, "str"))?;
                let f = self.runtime_fn(
                    Builtin::StrSubstring
                        .info()
                        .runtime_symbol
                        .context("runtime symbol")?,
                )?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[s.into(), a.into(), b.into()],
                    "substr",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "substr_i64",
                ))?
                .into())
            }
            Builtin::StrStartsWith | Builtin::StrEndsWith => {
                let a_i = self.coerce_i64(self.local(args[0])?)?;
                let b_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let a = crate::error::llvm(self.llvm.builder.build_int_to_ptr(a_i, ptr_ty, "a"))?;
                let b = crate::error::llvm(self.llvm.builder.build_int_to_ptr(b_i, ptr_ty, "b"))?;
                let fname = match name {
                    Builtin::StrStartsWith => "lumia_str_starts_with",
                    _ => "lumia_str_ends_with",
                };
                let f = self.runtime_fn(fname)?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[a.into(), b.into()],
                    "str_affix",
                ))?;
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?)
            }
            _ => unreachable!("non-string builtin in emit_string_builtin"),
        }
    }
}
