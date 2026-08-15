//! Println / Show value formatting for IO builtins.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, PointerValue};
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    /// Side-effecting print of one value (typed dispatch).
    pub(crate) fn emit_println_value(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        arg_ty: &Type,
    ) -> Result<()> {
        match arg_ty {
            Type::Unit => {
                // Unit is ABI i64 0; never print it as Int.
                let _ = arg;
                let s = self
                    .llvm
                    .builder
                    .build_global_string_ptr("Unit", "unit_lit")
                    .map_err(|e| anyhow::anyhow!("unit lit: {e}"))?;
                self.call_rt_void(
                    "lumia_println_cstr",
                    &[s.as_pointer_value().into()],
                    "println_unit",
                )?;
            }
            Type::Float => {
                let f = match arg {
                    BasicValueEnum::FloatValue(f) => f,
                    other => self.promote_f64(other)?,
                };
                self.call_rt_void("lumia_println_float", &[f.into()], "println_float")?;
            }
            Type::Bool => {
                let i = self.coerce_i64(arg)?;
                let b = self
                    .llvm
                    .builder
                    .build_int_truncate(i, self.llvm.context.i8_type(), "bool8")
                    .map_err(|e| anyhow::anyhow!("truncate bool8: {e}"))?;
                self.call_rt_void("lumia_println_bool", &[b.into()], "println_bool")?;
            }
            Type::List(elem) if matches!(elem.as_ref(), Type::Bool) => {
                let i = self.coerce_i64(arg)?;
                let ptr = {
                    let fun = self.runtime_fn("lumia_show_list_bool")?;
                    crate::error::llvm(self.llvm.builder.build_call(
                        fun,
                        &[i.into()],
                        "show_list_bool",
                    ))?
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value()
                };
                let len = self
                    .call_rt_basic("lumia_str_byte_len", &[ptr.into()], "show_len")?
                    .into_int_value();
                self.call_rt_void(
                    "lumia_println_str",
                    &[ptr.into(), len.into()],
                    "println_show",
                )?;
            }
            Type::Adt { name, params } => {
                let ptr = if let Some(ptr) = self.emit_show_override(name, arg)? {
                    Some(ptr)
                } else if self.funs.adt_variant_names.contains_key(name)
                    || params.iter().any(|p| matches!(p, Type::Float | Type::Bool))
                {
                    Some(self.emit_typed_adt_show(name, arg, params)?)
                } else {
                    None
                };
                if let Some(ptr) = ptr {
                    let len = self
                        .call_rt_basic("lumia_str_byte_len", &[ptr.into()], "show_len")?
                        .into_int_value();
                    self.call_rt_void(
                        "lumia_println_str",
                        &[ptr.into(), len.into()],
                        "println_show",
                    )?;
                } else {
                    let i = self.coerce_i64(arg)?;
                    self.call_rt_void("lumia_println_auto", &[i.into()], "println")?;
                }
            }
            _ => {
                let i = self.coerce_i64(arg)?;
                self.call_rt_void("lumia_println_auto", &[i.into()], "println")?;
            }
        }
        Ok(())
    }

    /// Format one value as a heap String pointer.
    pub(crate) fn emit_show_ptr(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        arg_ty: &Type,
    ) -> Result<PointerValue<'ctx>> {
        match arg_ty {
            Type::Unit => {
                let _ = arg;
                let ptr = self
                    .llvm
                    .builder
                    .build_global_string_ptr("Unit", "unit_show")
                    .map_err(|e| anyhow::anyhow!("unit show: {e}"))?
                    .as_pointer_value();
                let fun = self.runtime_fn("lumia_alloc_string")?;
                let len = self.llvm.i64_ty.const_int(4, false);
                Ok(crate::error::llvm(self.llvm.builder.build_call(
                    fun,
                    &[ptr.into(), len.into()],
                    "show_unit",
                ))?
                .try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_pointer_value())
            }
            Type::Float => {
                let f = match arg {
                    BasicValueEnum::FloatValue(f) => f,
                    other => self.promote_f64(other)?,
                };
                let fun = self.runtime_fn("lumia_show_float")?;
                Ok(crate::error::llvm(self.llvm.builder.build_call(
                    fun,
                    &[f.into()],
                    "show_float",
                ))?
                .try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_pointer_value())
            }
            Type::Bool => {
                let i = self.coerce_i64(arg)?;
                let b = crate::error::llvm(self.llvm.builder.build_int_truncate(
                    i,
                    self.llvm.context.i8_type(),
                    "bool8",
                ))?;
                let fun = self.runtime_fn("lumia_show_bool")?;
                Ok(
                    crate::error::llvm(self.llvm.builder.build_call(
                        fun,
                        &[b.into()],
                        "show_bool",
                    ))?
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value(),
                )
            }
            Type::List(elem) if matches!(elem.as_ref(), Type::Bool) => {
                let i = self.coerce_i64(arg)?;
                let fun = self.runtime_fn("lumia_show_list_bool")?;
                Ok(
                    crate::error::llvm(self.llvm.builder.build_call(
                        fun,
                        &[i.into()],
                        "show_list_bool",
                    ))?
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value(),
                )
            }
            Type::Adt { name, params } => {
                if let Some(ptr) = self.emit_show_override(name, arg)? {
                    Ok(ptr)
                } else if self.funs.adt_variant_names.contains_key(name)
                    || params.iter().any(|p| matches!(p, Type::Float | Type::Bool))
                {
                    self.emit_typed_adt_show(name, arg, params)
                } else {
                    self.emit_show_auto(arg)
                }
            }
            _ => self.emit_show_auto(arg),
        }
    }

    fn emit_show_auto(&mut self, arg: BasicValueEnum<'ctx>) -> Result<PointerValue<'ctx>> {
        let i = self.coerce_i64(arg)?;
        let fun = self.runtime_fn("lumia_show")?;
        Ok(
            crate::error::llvm(self.llvm.builder.build_call(fun, &[i.into()], "show"))?
                .try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_pointer_value(),
        )
    }
}
