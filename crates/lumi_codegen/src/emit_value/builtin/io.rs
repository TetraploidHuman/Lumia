//! Value emission — io builtins (Custom shapes only).

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use lumi_core::Local;
use lumi_core::Value;
use lumi_hir::Builtin;
use lumi_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_io_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::Println => {
                let arg = self.local(args[0])?;
                let mut arg_ty = self.infer_value_ty(&Value::Local(args[0]));
                if matches!(arg_ty, Type::Var(_)) {
                    if let Some(ty) = self.frame.local_tys.get(&args[0].0) {
                        arg_ty = ty.clone();
                    }
                }
                self.emit_println_value(arg, &arg_ty)?;
                Ok(self.llvm.i64_ty.const_int(0, false).into())
            }
            Builtin::Show => {
                let arg = self.local(args[0])?;
                let arg_ty = self
                    .frame
                    .local_tys
                    .get(&args[0].0)
                    .cloned()
                    .unwrap_or(Type::Int);
                let ptr = self.emit_show_ptr(arg, &arg_ty)?;
                self.ptr_as_i64(ptr, "show_i64")
            }
            Builtin::Assert => {
                let cond = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let (msg_ptr, msg_len) = if args.len() >= 2 {
                    let msg_i = self.coerce_i64(self.local(args[1])?)?;
                    let msg_ptr = crate::error::llvm(self.llvm.builder.build_int_to_ptr(
                        msg_i,
                        ptr_ty,
                        "assert_msg",
                    ))?;
                    let len_f = self.runtime_fn("lumi_str_len")?;
                    let call = crate::error::llvm(self.llvm.builder.build_call(
                        len_f,
                        &[msg_ptr.into()],
                        "assert_len",
                    ))?;
                    let len = call
                        .try_as_basic_value()
                        .basic()
                        .context("call return value")?;
                    (msg_ptr, len)
                } else {
                    (
                        ptr_ty.const_null(),
                        self.llvm.i64_ty.const_int(0, false).into(),
                    )
                };
                let f = self.runtime_fn(Self::builtin_symbol(name)?)?;
                crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[cond.into(), msg_ptr.into(), msg_len.into()],
                    "assert",
                ))?;
                Ok(self.llvm.i64_ty.const_int(0, false).into())
            }
            _ => unreachable!(
                "non-custom io builtin `{}` should use BuiltinEmit",
                name.display_name()
            ),
        }
    }
}
