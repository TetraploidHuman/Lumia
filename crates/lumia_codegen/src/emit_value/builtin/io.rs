//! Value emission — io builtins.

use super::super::super::Codegen;
use anyhow::Result;
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use lumia_core::Local;
use lumia_hir::Builtin;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_io_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr => {
                let arg = self.local(args[0])?;
                let arg_ty = self.local_tys.get(&args[0].0).cloned().unwrap_or(Type::Int);
                match arg_ty {
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
                            .builder
                            .build_int_truncate(i, self.context.i8_type(), "bool8")
                            .map_err(|e| anyhow::anyhow!("truncate bool8: {e}"))?;
                        self.call_rt_void("lumia_println_bool", &[b.into()], "println_bool")?;
                    }
                    Type::Adt { name, params } => {
                        let ptr = if let Some(ptr) = self.emit_show_override(&name, arg)? {
                            Some(ptr)
                        } else if params.iter().any(|p| matches!(p, Type::Float | Type::Bool)) {
                            Some(self.emit_typed_adt_show(arg, &params)?)
                        } else {
                            None
                        };
                        if let Some(ptr) = ptr {
                            let len = self
                                .call_rt_basic("lumia_str_len", &[ptr.into()], "show_len")?
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
                Ok(self.i64_ty.const_int(0, false).into())
            }
            Builtin::Show => {
                let arg = self.local(args[0])?;
                let arg_ty = self.local_tys.get(&args[0].0).cloned().unwrap_or(Type::Int);
                let ptr = match arg_ty {
                    Type::Float => {
                        let f = match arg {
                            BasicValueEnum::FloatValue(f) => f,
                            other => self.promote_f64(other)?,
                        };
                        let fun = self.module.get_function("lumia_show_float").unwrap();
                        self.builder
                            .build_call(fun, &[f.into()], "show_float")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value()
                    }
                    Type::Bool => {
                        let i = self.coerce_i64(arg)?;
                        let b = self
                            .builder
                            .build_int_truncate(i, self.context.i8_type(), "bool8")
                            .unwrap();
                        let fun = self.module.get_function("lumia_show_bool").unwrap();
                        self.builder
                            .build_call(fun, &[b.into()], "show_bool")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value()
                    }
                    Type::Adt { name, params } => {
                        if let Some(ptr) = self.emit_show_override(&name, arg)? {
                            ptr
                        } else if params.iter().any(|p| matches!(p, Type::Float | Type::Bool)) {
                            self.emit_typed_adt_show(arg, &params)?
                        } else {
                            let i = self.coerce_i64(arg)?;
                            let fun = self.module.get_function("lumia_show").unwrap();
                            self.builder
                                .build_call(fun, &[i.into()], "show")
                                .unwrap()
                                .try_as_basic_value()
                                .basic()
                                .unwrap()
                                .into_pointer_value()
                        }
                    }
                    _ => {
                        let i = self.coerce_i64(arg)?;
                        let fun = self.module.get_function("lumia_show").unwrap();
                        self.builder
                            .build_call(fun, &[i.into()], "show")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value()
                    }
                };
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "show_i64")
                    .unwrap()
                    .into())
            }
            Builtin::ReadStdin => {
                let f = self.module.get_function("lumia_read_stdin").unwrap();
                let call = self.builder.build_call(f, &[], "stdin").unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "stdin_i64")
                    .unwrap()
                    .into())
            }
            Builtin::MatchFail => {
                let f = self.module.get_function("lumia_match_fail").unwrap();
                self.builder.build_call(f, &[], "match_fail").unwrap();
                // Unreachable in practice; keep SSA well-typed.
                Ok(self.i64_ty.const_int(0, false).into())
            }
            Builtin::Assert => {
                let cond = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let (msg_ptr, msg_len) = if args.len() >= 2 {
                    let msg_i = self.coerce_i64(self.local(args[1])?)?;
                    let msg_ptr = self
                        .builder
                        .build_int_to_ptr(msg_i, ptr_ty, "assert_msg")
                        .unwrap();
                    let len_f = self.module.get_function("lumia_str_len").unwrap();
                    let len = self
                        .builder
                        .build_call(len_f, &[msg_ptr.into()], "assert_len")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap();
                    (msg_ptr, len)
                } else {
                    (ptr_ty.const_null(), self.i64_ty.const_int(0, false).into())
                };
                let f = self.module.get_function("lumia_assert").unwrap();
                self.builder
                    .build_call(f, &[cond.into(), msg_ptr.into(), msg_len.into()], "assert")
                    .unwrap();
                Ok(self.i64_ty.const_int(0, false).into())
            }
            _ => unreachable!("non-io builtin in emit_io_builtin"),
        }
    }
}
