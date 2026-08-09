//! Value emission — string builtins.

use super::super::super::Codegen;
use anyhow::Result;
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
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let s = self.builder.build_int_to_ptr(s_i, ptr_ty, "str").unwrap();
                let fname = match name {
                    Builtin::StrTrim => "lumia_str_trim",
                    Builtin::StrToLower => "lumia_str_to_lower",
                    _ => "lumia_str_to_upper",
                };
                let f = self.module.get_function(fname).unwrap();
                let call = self.builder.build_call(f, &[s.into()], "str_op").unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "str_i64")
                    .unwrap()
                    .into())
            }
            Builtin::StrSplit => {
                let s_i = self.coerce_i64(self.local(args[0])?)?;
                let sep = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let s = self.builder.build_int_to_ptr(s_i, ptr_ty, "str").unwrap();
                let f = self.module.get_function("lumia_str_split").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[s.into(), sep.into()], "split")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "split_i64")
                    .unwrap()
                    .into())
            }
            Builtin::StrSubstring => {
                let s_i = self.coerce_i64(self.local(args[0])?)?;
                let a = self.coerce_i64(self.local(args[1])?)?;
                let b = self.coerce_i64(self.local(args[2])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let s = self.builder.build_int_to_ptr(s_i, ptr_ty, "str").unwrap();
                let f = self.module.get_function("lumia_str_substring").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[s.into(), a.into(), b.into()], "substr")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "substr_i64")
                    .unwrap()
                    .into())
            }
            Builtin::StrStartsWith | Builtin::StrEndsWith => {
                let a_i = self.coerce_i64(self.local(args[0])?)?;
                let b_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let a = self.builder.build_int_to_ptr(a_i, ptr_ty, "a").unwrap();
                let b = self.builder.build_int_to_ptr(b_i, ptr_ty, "b").unwrap();
                let fname = match name {
                    Builtin::StrStartsWith => "lumia_str_starts_with",
                    _ => "lumia_str_ends_with",
                };
                let f = self.module.get_function(fname).unwrap();
                let call = self
                    .builder
                    .build_call(f, &[a.into(), b.into()], "str_affix")
                    .unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            _ => unreachable!("non-string builtin in emit_string_builtin"),
        }
    }
}
