//! Value emission — arithmetic and binary/unary ops

use super::super::Codegen;
use anyhow::{bail, Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue};
use inkwell::{FloatPredicate, IntPredicate};
use lumia_core::Local;
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
        Ok(crate::error::llvm(
            self.llvm.builder.build_int_neg(o, "neg"),
        )?)
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
            // Float-typed locals are IEEE bits in i64; Int locals are numeric
            // (sitofp) so `{ x -> x + 1 }` works after Float monomorphization.
            let l = self.arith_as_f64(lv, &lt)?;
            let r = self.arith_as_f64(rv, &rt)?;
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
                    let c = crate::error::llvm(
                        self.llvm.builder.build_float_compare(pred, l, r, "fcmp"),
                    )?;
                    return Ok(crate::error::llvm(self.llvm.builder.build_int_z_extend(
                        c,
                        self.llvm.i64_ty,
                        "fcmpz",
                    ))?
                    .into());
                }
                _ => unreachable!(),
            };
            return Ok(v.into());
        }
        let l = self.as_i64(lv)?;
        let r = self.as_i64(rv)?;
        // `instance Num for T`: `__Num_T_add` / `__Num_T_mul`.
        if matches!(op, BinOp::Add | BinOp::Mul) {
            if let Some(name) = Self::adt_method_name(&lt, &rt) {
                let method = if matches!(op, BinOp::Add) {
                    "add"
                } else {
                    "mul"
                };
                let mangled = format!("__Num_{name}_{method}");
                if let Some(callee) = self.funs.functions.get(&mangled).copied() {
                    let call = crate::error::llvm(self.llvm.builder.build_call(
                        callee,
                        &[l.into(), r.into()],
                        "num_ov",
                    ))?;
                    return Ok(call
                        .try_as_basic_value()
                        .basic()
                        .unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into()));
                }
            }
        }
        let v = match op {
            BinOp::Add => self.emit_checked_binop(l, r, fv, "sadd")?,
            BinOp::Sub => self.emit_checked_binop(l, r, fv, "ssub")?,
            BinOp::Mul => self.emit_checked_binop(l, r, fv, "smul")?,
            BinOp::Div => self.emit_checked_div_rem(l, r, fv, false)?,
            BinOp::Rem => self.emit_checked_div_rem(l, r, fv, true)?,
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
                if let Some(name) = Self::adt_method_name(&lt, &rt) {
                    if self
                        .funs
                        .functions
                        .contains_key(&format!("__Ord_{name}_less"))
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
                            BinOp::Lt | BinOp::Gt => less.into(),
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
                                .into()
                            }
                            _ => unreachable!(),
                        });
                    }
                }
                // Structural Ord via runtime (String/Char/ADT); never SLT pointers.
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
                let c =
                    crate::error::llvm(self.llvm.builder.build_int_compare(pred, cmp, z, "ord"))?;
                crate::error::llvm(self.llvm.builder.build_int_z_extend(
                    c,
                    self.llvm.i64_ty,
                    "ordz",
                ))?
            }
            BinOp::And => self
                .llvm
                .builder
                .build_and(l, r, "and")
                .context("call return value")?,
            BinOp::Or => self
                .llvm
                .builder
                .build_or(l, r, "or")
                .context("call return value")?,
        };
        Ok(v.into())
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
