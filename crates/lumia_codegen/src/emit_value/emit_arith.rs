//! Value emission — arithmetic and binary/unary ops

use super::super::Codegen;
use anyhow::{bail, Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue};
use inkwell::{FloatPredicate, IntPredicate};
use lumia_core::{Local, Value};
use lumia_syntax::{BinOp, UnOp};
use lumia_ty::Type;

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

    fn dest_is_nsw_safe(&self) -> bool {
        self.frame
            .emit_dest
            .is_some_and(|d| self.frame.nsw_binop_locals.contains(&d))
    }

    /// Dividend proven ≥ 0 (nonneg IV load or nonnegative Int) ⇒ `urem`/`udiv`.
    ///
    /// NSW-safe ≠ nonnegative: bounded trees mark `Sub` (e.g. `i - 5`) which can
    /// be negative; those must keep signed `srem`/`sdiv`.
    fn dividend_nonneg(&self, left: &Local) -> bool {
        if self.frame.nonneg_iv_load_locals.contains(&left.0) {
            return true;
        }
        matches!(self.frame.leaf_defs.get(&left.0), Some(Value::Int(n)) if *n >= 0)
    }

    fn emit_nsw_binop(
        &self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>> {
        crate::error::llvm(self.llvm.builder.build_int_nsw_add(l, r, name))
    }

    fn emit_nsw_binop_sub(
        &self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>> {
        crate::error::llvm(self.llvm.builder.build_int_nsw_sub(l, r, name))
    }

    fn emit_nsw_binop_mul(
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

    fn emit_unchecked_div_rem(
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

    pub(crate) fn emit_value_binary(
        &mut self,
        op: &BinOp,
        left: &Local,
        right: &Local,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        let lv = self.local(*left)?;
        let rv = self.local(*right)?;
        // Heap loads (`ListGet`, fields) keep Float as i64 bits; consult
        // `local_tys`, not only LLVM FloatValue, or we do Int ops on IEEE bits.
        let lt = self
            .frame
            .local_tys
            .get(&left.0)
            .cloned()
            .unwrap_or(Type::Int);
        let rt = self
            .frame
            .local_tys
            .get(&right.0)
            .cloned()
            .unwrap_or(Type::Int);
        let either_float = matches!(lt, Type::Float)
            || matches!(rt, Type::Float)
            || matches!(lv, BasicValueEnum::FloatValue(_))
            || matches!(rv, BasicValueEnum::FloatValue(_));
        if either_float
            && matches!(
                op,
                BinOp::Add
                    | BinOp::Sub
                    | BinOp::Mul
                    | BinOp::Div
                    | BinOp::Rem
                    | BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Le
                    | BinOp::Gt
                    | BinOp::Ge
            )
        {
            return self.emit_float_binary(op, lv, rv, &lt, &rt);
        }
        let l = self.as_i64(lv)?;
        let r = self.as_i64(rv)?;
        // Num ADT `+`/`*` must already be `Call(__Num_*)` after
        // `resolve_trait_method_calls`; residual Binary is an ICE.
        self.reject_residual_num_binary(op, &lt, &rt)?;
        let v = match op {
            BinOp::Add if self.dest_is_nsw_safe() => self.emit_nsw_binop(l, r, "add")?,
            BinOp::Sub if self.dest_is_nsw_safe() => self.emit_nsw_binop_sub(l, r, "sub")?,
            BinOp::Mul if self.dest_is_nsw_safe() => self.emit_nsw_binop_mul(l, r, "mul")?,
            BinOp::Add => self.emit_checked_binop(l, r, fv, "sadd")?,
            BinOp::Sub => self.emit_checked_binop(l, r, fv, "ssub")?,
            BinOp::Mul => self.emit_checked_binop(l, r, fv, "smul")?,
            BinOp::Div => {
                if self.frame.safe_divisor_locals.contains(&right.0) {
                    let unsigned = self.dividend_nonneg(left);
                    self.emit_unchecked_div_rem(l, r, false, unsigned)?
                } else {
                    self.emit_checked_div_rem(l, r, fv, false)?
                }
            }
            BinOp::Rem => {
                if self.frame.safe_divisor_locals.contains(&right.0) {
                    let unsigned = self.dividend_nonneg(left);
                    self.emit_unchecked_div_rem(l, r, true, unsigned)?
                } else {
                    self.emit_checked_div_rem(l, r, fv, true)?
                }
            }
            BinOp::Eq => self.emit_value_eq(&lt, &rt, l, r)?,
            BinOp::Ne => {
                let eq = self.emit_value_eq(&lt, &rt, l, r)?;
                let z = self.llvm.i64_ty.const_int(0, false);
                let c = crate::error::llvm(self.llvm.builder.build_int_compare(
                    IntPredicate::EQ,
                    eq,
                    z,
                    "ne",
                ))?;
                crate::error::llvm(self.llvm.builder.build_int_z_extend(
                    c,
                    self.llvm.i64_ty,
                    "nez",
                ))?
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.emit_ord_compare(op, &lt, &rt, l, r)?
            }
            BinOp::And | BinOp::Or => {
                bail!("ICE: BinOp::And|Or reached codegen; expected If desugar")
            }
        };
        Ok(v.into())
    }

    fn emit_float_binary(
        &mut self,
        op: &BinOp,
        lv: BasicValueEnum<'ctx>,
        rv: BasicValueEnum<'ctx>,
        lt: &Type,
        rt: &Type,
    ) -> Result<BasicValueEnum<'ctx>> {
        // Float-typed locals are IEEE bits in i64; Int locals are numeric
        // (sitofp) so `{ x -> x + 1 }` works after Float monomorphization.
        let l = self.arith_as_f64(lv, lt)?;
        let r = self.arith_as_f64(rv, rt)?;
        let v = match op {
            BinOp::Add => crate::error::llvm(self.llvm.builder.build_float_add(l, r, "fadd"))?,
            BinOp::Sub => self
                .llvm
                .builder
                .build_float_sub(l, r, "fsub")
                .context("call return value")?,
            BinOp::Mul => self
                .llvm
                .builder
                .build_float_mul(l, r, "fmul")
                .context("call return value")?,
            BinOp::Div => self
                .llvm
                .builder
                .build_float_div(l, r, "fdiv")
                .context("call return value")?,
            BinOp::Rem => self
                .llvm
                .builder
                .build_float_rem(l, r, "frem")
                .context("call return value")?,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let pred = match op {
                    BinOp::Eq => FloatPredicate::OEQ,
                    // UNE: NaN != x is true (IEEE unordered-or-ne).
                    BinOp::Ne => FloatPredicate::UNE,
                    BinOp::Lt => FloatPredicate::OLT,
                    BinOp::Le => FloatPredicate::OLE,
                    BinOp::Gt => FloatPredicate::OGT,
                    BinOp::Ge => FloatPredicate::OGE,
                    _ => unreachable!(),
                };
                let c =
                    crate::error::llvm(self.llvm.builder.build_float_compare(pred, l, r, "fcmp"))?;
                return Ok(crate::error::llvm(self.llvm.builder.build_int_z_extend(
                    c,
                    self.llvm.i64_ty,
                    "fcmpz",
                ))?
                .into());
            }
            _ => unreachable!(),
        };
        Ok(v.into())
    }

    /// Mono must rewrite Num ADT `+`/`*` to `Call(__Num_T_*)`. A surviving
    /// Binary here would previously hit a silent codegen override on the
    /// unspecialized instance body — treat that as an ICE instead.
    fn reject_residual_num_binary(&self, op: &BinOp, lt: &Type, rt: &Type) -> Result<()> {
        if !matches!(op, BinOp::Add | BinOp::Mul) {
            return Ok(());
        }
        let Some(name) = Self::adt_method_name(lt, rt) else {
            return Ok(());
        };
        let method = if matches!(op, BinOp::Add) {
            "add"
        } else {
            "mul"
        };
        let mangled = lumia_hir::mangle_trait_method("Num", &name, method);
        if self.funs.functions.contains_key(&mangled) {
            anyhow::bail!(
                "ICE: Num Binary `{op:?}` on `{name}` survived to codegen \
                 (expected Call(`{mangled}`) after resolve_trait_method_calls)"
            );
        }
        Ok(())
    }

    fn emit_ord_compare(
        &mut self,
        op: &BinOp,
        lt: &Type,
        rt: &Type,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>> {
        if let Some(name) = Self::adt_method_name(lt, rt) {
            if self
                .funs
                .functions
                .contains_key(&lumia_hir::mangle_trait_method("Ord", &name, "less"))
            {
                // DESIGN less(self, other): a < b
                let (left, right) = match op {
                    BinOp::Lt | BinOp::Ge => (l, r),
                    BinOp::Gt | BinOp::Le => (r, l),
                    _ => unreachable!(),
                };
                let less = self
                    .emit_less_override(&name, left, right)?
                    .context("Ord.less")?;
                let z = self.llvm.i64_ty.const_int(0, false);
                return Ok(match op {
                    BinOp::Lt | BinOp::Gt => less,
                    BinOp::Le | BinOp::Ge => {
                        // a <= b  iff  !(b < a); a >= b iff !(a < b)
                        let c = crate::error::llvm(self.llvm.builder.build_int_compare(
                            IntPredicate::EQ,
                            less,
                            z,
                            "nless",
                        ))?;
                        crate::error::llvm(self.llvm.builder.build_int_z_extend(
                            c,
                            self.llvm.i64_ty,
                            "nlessz",
                        ))?
                    }
                    _ => unreachable!(),
                });
            }
        }
        // Structural Ord via runtime (String/Char/ADT); never SLT pointers.
        // Int/Bool are bit-identity scalars — native icmp (hot path for loop latches).
        if Self::is_bit_identity_scalar(lt) && Self::is_bit_identity_scalar(rt) {
            let pred = match op {
                BinOp::Lt => IntPredicate::SLT,
                BinOp::Le => IntPredicate::SLE,
                BinOp::Gt => IntPredicate::SGT,
                BinOp::Ge => IntPredicate::SGE,
                _ => unreachable!(),
            };
            let c = crate::error::llvm(self.llvm.builder.build_int_compare(pred, l, r, "icmp"))?;
            return crate::error::llvm(self.llvm.builder.build_int_z_extend(
                c,
                self.llvm.i64_ty,
                "icmpz",
            ));
        }
        let f = self.runtime_fn("lumia_cmp")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            f,
            &[l.into(), r.into()],
            "cmp",
        ))?;
        let cmp = call
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_int_value();
        let z = self.llvm.i64_ty.const_int(0, false);
        let pred = match op {
            BinOp::Lt => IntPredicate::SLT,
            BinOp::Le => IntPredicate::SLE,
            BinOp::Gt => IntPredicate::SGT,
            BinOp::Ge => IntPredicate::SGE,
            _ => unreachable!(),
        };
        let c = crate::error::llvm(self.llvm.builder.build_int_compare(pred, cmp, z, "ord"))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_int_z_extend(c, self.llvm.i64_ty, "ordz"),
        )
    }

    /// Int / Bool / Unit: compare as i64 bits (not heap pointers).
    pub(crate) fn is_bit_identity_scalar(ty: &Type) -> bool {
        matches!(ty, Type::Int | Type::Bool | Type::Unit)
    }

    pub(crate) fn emit_value_unary(
        &mut self,
        op: &UnOp,
        operand: &Local,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        let ov = self.local(*operand)?;
        let ot = self
            .frame
            .local_tys
            .get(&operand.0)
            .cloned()
            .unwrap_or(Type::Int);
        if matches!(ot, Type::Float) || matches!(ov, BasicValueEnum::FloatValue(_)) {
            let o = self.promote_f64(ov)?;
            let v = match op {
                UnOp::Neg => crate::error::llvm(self.llvm.builder.build_float_neg(o, "fneg"))?,
                UnOp::Not => bail!("not on Float"),
            };
            return Ok(v.into());
        }
        let o = self.as_i64(ov)?;
        let v = match op {
            UnOp::Neg => self.emit_checked_neg(o, fv)?,
            UnOp::Not => {
                let z = self.llvm.i64_ty.const_int(0, false);
                let c = crate::error::llvm(self.llvm.builder.build_int_compare(
                    IntPredicate::EQ,
                    o,
                    z,
                    "not",
                ))?;
                crate::error::llvm(self.llvm.builder.build_int_z_extend(
                    c,
                    self.llvm.i64_ty,
                    "notz",
                ))?
            }
        };
        Ok(v.into())
    }
}
