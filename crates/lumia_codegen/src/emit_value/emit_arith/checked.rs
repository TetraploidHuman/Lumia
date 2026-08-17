//! Checked Int overflow / NSW / div-rem.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{FunctionValue, IntValue};
use inkwell::IntPredicate;
use lumia_core::{Local, Value};

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_checked_neg(
        &mut self,
        o: IntValue<'ctx>,
        fv: FunctionValue<'ctx>,
    ) -> Result<IntValue<'ctx>> {
        let min = self.llvm.i64_ty.const_int(i64::MIN as u64, true);
        let is_min = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::EQ,
            o,
            min,
            "neg_min",
        ))?;
        let trap_bb = self
            .llvm
            .context
            .append_basic_block(fv, "neg_overflow_trap");
        let ok_bb = self.llvm.context.append_basic_block(fv, "neg_ok");
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(is_min, trap_bb, ok_bb),
        )?;
        self.llvm.builder.position_at_end(trap_bb);
        let trap = self.runtime_fn("lumia_trap_overflow")?;
        crate::error::llvm(self.llvm.builder.build_call(trap, &[], "trap_neg"))?;
        crate::error::llvm(self.llvm.builder.build_unreachable())?;
        self.llvm.builder.position_at_end(ok_bb);
        crate::error::llvm(self.llvm.builder.build_int_neg(o, "neg"))
    }

    pub(super) fn dest_is_nsw_safe(&self) -> bool {
        self.frame
            .emit_dest
            .is_some_and(|d| self.frame.nsw_binop_locals.contains(&d))
    }

    /// Dividend proven ≥ 0 (nonneg IV load or nonnegative Int) ⇒ `urem`/`udiv`.
    ///
    /// NSW-safe ≠ nonnegative: bounded trees mark `Sub` (e.g. `i - 5`) which can
    /// be negative; those must keep signed `srem`/`sdiv`.
    pub(super) fn dividend_nonneg(&self, left: &Local) -> bool {
        if self.frame.nonneg_iv_load_locals.contains(&left.0) {
            return true;
        }
        matches!(self.frame.leaf_defs.get(&left.0), Some(Value::Int(n)) if *n >= 0)
    }

    pub(super) fn emit_nsw_binop(
        &self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>> {
        crate::error::llvm(self.llvm.builder.build_int_nsw_add(l, r, name))
    }

    pub(super) fn emit_nsw_binop_sub(
        &self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>> {
        crate::error::llvm(self.llvm.builder.build_int_nsw_sub(l, r, name))
    }

    pub(super) fn emit_nsw_binop_mul(
        &self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>> {
        crate::error::llvm(self.llvm.builder.build_int_nsw_mul(l, r, name))
    }

    pub(crate) fn emit_checked_binop(
        &mut self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        fv: FunctionValue<'ctx>,
        kind: &str,
    ) -> Result<IntValue<'ctx>> {
        let name = format!("llvm.{kind}.with.overflow.i64");
        let intrinsic = inkwell::intrinsics::Intrinsic::find(&name)
            .with_context(|| format!("missing intrinsic {name}"))?;
        let id_tys = [self.llvm.i64_ty.into()];
        let fnty = intrinsic
            .get_declaration(&self.llvm.module, &id_tys)
            .context("intrinsic declaration")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            fnty,
            &[l.into(), r.into()],
            "checked",
        ))?;
        let agg = call
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_struct_value();
        let result = crate::error::llvm(self.llvm.builder.build_extract_value(agg, 0, "ov_res"))?
            .into_int_value();
        let overflow =
            crate::error::llvm(self.llvm.builder.build_extract_value(agg, 1, "ov_flag"))?
                .into_int_value();
        let trap_bb = self.llvm.context.append_basic_block(fv, "overflow_trap");
        let ok_bb = self.llvm.context.append_basic_block(fv, "overflow_ok");
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(overflow, trap_bb, ok_bb),
        )?;
        self.llvm.builder.position_at_end(trap_bb);
        let trap = self.runtime_fn("lumia_trap_overflow")?;
        crate::error::llvm(self.llvm.builder.build_call(trap, &[], "trap_ov"))?;
        crate::error::llvm(self.llvm.builder.build_unreachable())?;
        self.llvm.builder.position_at_end(ok_bb);
        Ok(result)
    }

    pub(crate) fn emit_checked_div_rem(
        &mut self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        fv: FunctionValue<'ctx>,
        is_rem: bool,
    ) -> Result<IntValue<'ctx>> {
        // Constant divisor: skip checks that cannot fire.
        if let Some(c) = r.get_sign_extended_constant() {
            if c == 0 {
                let trap = self.runtime_fn("lumia_trap_div0")?;
                crate::error::llvm(self.llvm.builder.build_call(trap, &[], "trap0"))?;
                crate::error::llvm(self.llvm.builder.build_unreachable())?;
                // Unreachable; keep a value for typing.
                return Ok(self.llvm.i64_ty.const_int(0, false));
            }
            if c == -1 {
                let i64_min = self.llvm.i64_ty.const_int(i64::MIN as u64, true);
                let is_min = crate::error::llvm(self.llvm.builder.build_int_compare(
                    IntPredicate::EQ,
                    l,
                    i64_min,
                    "div_min",
                ))?;
                let trap_bb = self.llvm.context.append_basic_block(fv, "div_ov_trap");
                let ok_bb = self.llvm.context.append_basic_block(fv, "div_ok");
                crate::error::llvm(
                    self.llvm
                        .builder
                        .build_conditional_branch(is_min, trap_bb, ok_bb),
                )?;
                self.llvm.builder.position_at_end(trap_bb);
                let t1 = self.runtime_fn("lumia_trap_overflow")?;
                crate::error::llvm(self.llvm.builder.build_call(t1, &[], "trap_ov"))?;
                crate::error::llvm(self.llvm.builder.build_unreachable())?;
                self.llvm.builder.position_at_end(ok_bb);
            } else {
                // c ∉ {0, -1}: no div0 / MIN÷-1.
                return Ok(if is_rem {
                    crate::error::llvm(self.llvm.builder.build_int_signed_rem(l, r, "rem"))?
                } else {
                    crate::error::llvm(self.llvm.builder.build_int_signed_div(l, r, "div"))?
                });
            }
            return Ok(if is_rem {
                crate::error::llvm(self.llvm.builder.build_int_signed_rem(l, r, "rem"))?
            } else {
                crate::error::llvm(self.llvm.builder.build_int_signed_div(l, r, "div"))?
            });
        }

        let zero = self.llvm.i64_ty.const_int(0, false);
        let minus_one = self.llvm.i64_ty.const_int((-1i64) as u64, true);
        let i64_min = self.llvm.i64_ty.const_int(i64::MIN as u64, true);
        let is_zero = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::EQ,
            r,
            zero,
            "div0",
        ))?;
        let is_m1 = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::EQ,
            r,
            minus_one,
            "div_m1",
        ))?;
        let is_min = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::EQ,
            l,
            i64_min,
            "div_min",
        ))?;
        let ov = crate::error::llvm(self.llvm.builder.build_and(is_m1, is_min, "div_ov"))?;
        let bad = crate::error::llvm(self.llvm.builder.build_or(is_zero, ov, "div_bad"))?;
        let trap_bb = self.llvm.context.append_basic_block(fv, "div_trap");
        let ok_bb = self.llvm.context.append_basic_block(fv, "div_ok");
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(bad, trap_bb, ok_bb),
        )?;
        self.llvm.builder.position_at_end(trap_bb);
        let div0_bb = self.llvm.context.append_basic_block(fv, "div0_trap");
        let ov_bb = self.llvm.context.append_basic_block(fv, "div_ov_trap");
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(is_zero, div0_bb, ov_bb),
        )?;
        self.llvm.builder.position_at_end(div0_bb);
        let t0 = self.runtime_fn("lumia_trap_div0")?;
        crate::error::llvm(self.llvm.builder.build_call(t0, &[], "trap0"))?;
        crate::error::llvm(self.llvm.builder.build_unreachable())?;
        self.llvm.builder.position_at_end(ov_bb);
        let t1 = self.runtime_fn("lumia_trap_overflow")?;
        crate::error::llvm(self.llvm.builder.build_call(t1, &[], "trap_ov"))?;
        crate::error::llvm(self.llvm.builder.build_unreachable())?;
        self.llvm.builder.position_at_end(ok_bb);
        Ok(if is_rem {
            crate::error::llvm(self.llvm.builder.build_int_signed_rem(l, r, "rem"))?
        } else {
            crate::error::llvm(self.llvm.builder.build_int_signed_div(l, r, "div"))?
        })
    }

    pub(super) fn emit_unchecked_div_rem(
        &self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        is_rem: bool,
        unsigned: bool,
    ) -> Result<IntValue<'ctx>> {
        Ok(if is_rem {
            if unsigned {
                crate::error::llvm(self.llvm.builder.build_int_unsigned_rem(l, r, "urem"))?
            } else {
                crate::error::llvm(self.llvm.builder.build_int_signed_rem(l, r, "rem"))?
            }
        } else if unsigned {
            crate::error::llvm(self.llvm.builder.build_int_unsigned_div(l, r, "udiv"))?
        } else {
            crate::error::llvm(self.llvm.builder.build_int_signed_div(l, r, "div"))?
        })
    }

}
