//! Value emission and closely related helpers.

mod affine2_sr;
pub(crate) mod builtin;
mod collatz_sr;
mod dense_f64_sr;
mod emit_alloc;
mod emit_arith;
mod emit_calls;
mod emit_control;
mod float_sr;
mod number_theory_sr;
mod sr_pattern;
mod trial_div_sr;

use super::Codegen;
use anyhow::{bail, Result};
use inkwell::values::{BasicValueEnum, FunctionValue};
use lumia_core::{Block, Value};

impl<'ctx> Codegen<'ctx> {
    /// Try registered loop shape rewrites, then fall back to generic loop emit.
    ///
    /// Order is significant (more specific patterns first). Append new `*_sr`
    /// matchers to the array below — do not reintroduce an open if/else chain.
    fn emit_value_loop_with_srs(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        type EmitFn<'c> = fn(
            &mut Codegen<'c>,
            &Block,
            &Block,
            &Block,
            FunctionValue<'c>,
        ) -> Result<Option<BasicValueEnum<'c>>>;
        let registry: &[EmitFn<'ctx>] = &[
            Self::try_emit_collatz_total_loop,
            Self::try_emit_collatz_strided_loop,
            Self::try_emit_count_primes_loop,
            Self::try_emit_affine2_rem_sum_loop,
            Self::try_emit_gcd_sum_loop,
            Self::try_emit_divisor_sum_loop,
            Self::try_emit_product_rem_sum_loop,
            Self::try_emit_range_affine1_loop,
            Self::try_emit_matmul_affine_loop,
            Self::try_emit_float_orbit_loop,
            Self::try_emit_mandelbrot_loop,
            Self::try_emit_collatz_loop,
            Self::try_emit_trial_div_loop,
        ];
        for emit in registry {
            if let Some(v) = emit(self, header, body, latch, fv)? {
                return Ok(v);
            }
        }
        self.emit_value_loop(header, body, latch, fv)
    }

    pub(crate) fn emit_value(
        &mut self,
        value: &Value,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        match value {
            Value::Int(n) => Ok(self.llvm.i64_ty.const_int(*n as u64, true).into()),
            Value::Float(n) => Ok(self.llvm.context.f64_type().const_float(*n).into()),
            Value::Bool(b) => Ok(self
                .llvm
                .i64_ty
                .const_int(if *b { 1 } else { 0 }, false)
                .into()),
            Value::String(s) => {
                let gv = self
                    .llvm
                    .builder
                    .build_global_string_ptr(s, "str")
                    .map_err(|e| anyhow::anyhow!("global string: {e}"))?;
                let ptr = gv.as_pointer_value();
                let len = self.llvm.i64_ty.const_int(s.len() as u64, false);
                let heap = self
                    .call_rt_basic("lumia_alloc_string", &[ptr.into(), len.into()], "alloc_str")?
                    .into_pointer_value();
                Ok(self
                    .llvm
                    .builder
                    .build_ptr_to_int(heap, self.llvm.i64_ty, "str_i64")
                    .map_err(|e| anyhow::anyhow!("ptr_to_int str: {e}"))?
                    .into())
            }
            Value::Char(c) => {
                let cp = self.llvm.i64_ty.const_int(*c as u32 as u64, false);
                let heap = self
                    .call_rt_basic("lumia_alloc_char", &[cp.into()], "alloc_char")?
                    .into_pointer_value();
                Ok(self
                    .llvm
                    .builder
                    .build_ptr_to_int(heap, self.llvm.i64_ty, "char_i64")
                    .map_err(|e| anyhow::anyhow!("ptr_to_int char: {e}"))?
                    .into())
            }
            Value::Unit => Ok(self.llvm.i64_ty.const_int(0, false).into()),
            Value::Local(l) => self.local(*l),
            Value::Name(name) => self.load_slot(name),
            Value::Binary { op, left, right } => self.emit_value_binary(op, left, right, fv),
            Value::Unary { op, operand } => self.emit_value_unary(op, operand, fv),
            Value::Call { fun, args } => self.emit_value_call(fun, args),
            Value::IndirectCall { callee, args } => self.emit_value_indirect_call(callee, args),
            Value::FunRef(name) => self.emit_value_funref(name),
            Value::Builtin { name, args, .. } => self.emit_value_builtin(name, args),
            Value::If {
                cond,
                then_block,
                else_block,
            } => self.emit_value_if(cond, then_block, else_block, fv),
            Value::Loop {
                header,
                body,
                latch,
            } => self.emit_value_loop_with_srs(header, body, latch, fv),
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
