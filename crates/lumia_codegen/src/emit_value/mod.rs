//! Value emission and closely related helpers.

mod emit_alloc;
mod emit_arith;
mod emit_builtin;
mod emit_calls;
mod emit_control;

use super::Codegen;
use anyhow::{bail, Result};
use inkwell::values::{BasicValueEnum, FunctionValue};
use lumia_core::Value;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_value(
        &mut self,
        value: &Value,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        match value {
            Value::Int(n) => Ok(self.i64_ty.const_int(*n as u64, true).into()),
            Value::Float(n) => Ok(self.context.f64_type().const_float(*n).into()),
            Value::Bool(b) => Ok(self.i64_ty.const_int(if *b { 1 } else { 0 }, false).into()),
            Value::String(s) => {
                let gv = self.builder.build_global_string_ptr(s, "str").unwrap();
                let ptr = gv.as_pointer_value();
                let len = self.i64_ty.const_int(s.len() as u64, false);
                let f = self.module.get_function("lumia_alloc_string").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[ptr.into(), len.into()], "alloc_str")
                    .unwrap();
                let heap = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(heap, self.i64_ty, "str_i64")
                    .unwrap()
                    .into())
            }
            Value::Char(c) => {
                let cp = self.i64_ty.const_int(*c as u32 as u64, false);
                let f = self.module.get_function("lumia_alloc_char").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[cp.into()], "alloc_char")
                    .unwrap();
                let heap = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(heap, self.i64_ty, "char_i64")
                    .unwrap()
                    .into())
            }
            Value::Unit => Ok(self.i64_ty.const_int(0, false).into()),
            Value::Local(l) => self.local(*l),
            Value::Name(name) => self.load_slot(name),
            Value::Binary { op, left, right } => self.emit_value_binary(op, left, right, fv),
            Value::Unary { op, operand } => self.emit_value_unary(op, operand, fv),
            Value::Call { fun, args } => self.emit_value_call(fun, args),
            Value::IndirectCall { callee, args } => self.emit_value_indirect_call(callee, args),
            Value::FunRef(name) => self.emit_value_funref(name),
            Value::Builtin { name, args } => self.emit_value_builtin(name, args),
            Value::If {
                cond,
                then_block,
                else_block,
            } => self.emit_value_if(cond, then_block, else_block, fv),
            Value::Loop {
                header,
                body,
                latch,
            } => self.emit_value_loop(header, body, latch, fv),
            Value::Lambda { .. } => bail!("lambda should have been lifted to FunRef/AllocClosure"),
            Value::AllocClosure { fun, captures } => self.emit_value_alloc_closure(fun, captures),
            Value::ClosureCap {
                env,
                index,
                as_float,
            } => self.emit_value_closure_cap(env, *index, *as_float),
            Value::AllocList { elems, repr } => self.emit_value_alloc_list(elems, *repr),
            Value::AllocSet { elems, repr } => self.emit_value_alloc_set(elems, *repr),
            Value::AllocMap { flat_pairs, repr } => self.emit_value_alloc_map(flat_pairs, *repr),
            Value::AllocAdt {
                adt_name,
                tag,
                fields,
                repr,
            } => self.emit_value_alloc_adt(adt_name, *tag, fields, *repr),
        }
    }
}
