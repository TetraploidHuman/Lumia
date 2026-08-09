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
        let min = self.i64_ty.const_int(i64::MIN as u64, true);
        let is_min = self
            .builder
            .build_int_compare(IntPredicate::EQ, o, min, "neg_min")
            .unwrap();
        let trap_bb = self.context.append_basic_block(fv, "neg_overflow_trap");
        let ok_bb = self.context.append_basic_block(fv, "neg_ok");
        self.builder
            .build_conditional_branch(is_min, trap_bb, ok_bb)
            .unwrap();
        self.builder.position_at_end(trap_bb);
        let trap = self.module.get_function("lumia_trap_overflow").unwrap();
        self.builder.build_call(trap, &[], "trap_neg").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ok_bb);
        Ok(self.builder.build_int_neg(o, "neg").unwrap())
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
        let id_tys = [self.i64_ty.into()];
        let fnty = intrinsic.get_declaration(&self.module, &id_tys).unwrap();
        let call = self
            .builder
            .build_call(fnty, &[l.into(), r.into()], "checked")
            .unwrap();
        let agg = call
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_struct_value();
        let result = self
            .builder
            .build_extract_value(agg, 0, "ov_res")
            .unwrap()
            .into_int_value();
        let overflow = self
            .builder
            .build_extract_value(agg, 1, "ov_flag")
            .unwrap()
            .into_int_value();
        let trap_bb = self.context.append_basic_block(fv, "overflow_trap");
        let ok_bb = self.context.append_basic_block(fv, "overflow_ok");
        self.builder
            .build_conditional_branch(overflow, trap_bb, ok_bb)
            .unwrap();
        self.builder.position_at_end(trap_bb);
        let trap = self.module.get_function("lumia_trap_overflow").unwrap();
        self.builder.build_call(trap, &[], "trap_ov").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ok_bb);
        Ok(result)
    }

    pub(crate) fn emit_checked_div_rem(
        &mut self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        fv: FunctionValue<'ctx>,
        is_rem: bool,
    ) -> Result<IntValue<'ctx>> {
        let zero = self.i64_ty.const_int(0, false);
        let minus_one = self.i64_ty.const_int((-1i64) as u64, true);
        let i64_min = self.i64_ty.const_int(i64::MIN as u64, true);
        let is_zero = self
            .builder
            .build_int_compare(IntPredicate::EQ, r, zero, "div0")
            .unwrap();
        let is_m1 = self
            .builder
            .build_int_compare(IntPredicate::EQ, r, minus_one, "div_m1")
            .unwrap();
        let is_min = self
            .builder
            .build_int_compare(IntPredicate::EQ, l, i64_min, "div_min")
            .unwrap();
        let ov = self.builder.build_and(is_m1, is_min, "div_ov").unwrap();
        let bad = self.builder.build_or(is_zero, ov, "div_bad").unwrap();
        let trap_bb = self.context.append_basic_block(fv, "div_trap");
        let ok_bb = self.context.append_basic_block(fv, "div_ok");
        self.builder
            .build_conditional_branch(bad, trap_bb, ok_bb)
            .unwrap();
        self.builder.position_at_end(trap_bb);
        let div0_bb = self.context.append_basic_block(fv, "div0_trap");
        let ov_bb = self.context.append_basic_block(fv, "div_ov_trap");
        self.builder
            .build_conditional_branch(is_zero, div0_bb, ov_bb)
            .unwrap();
        self.builder.position_at_end(div0_bb);
        let t0 = self.module.get_function("lumia_trap_div0").unwrap();
        self.builder.build_call(t0, &[], "trap0").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ov_bb);
        let t1 = self.module.get_function("lumia_trap_overflow").unwrap();
        self.builder.build_call(t1, &[], "trap_ov").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ok_bb);
        Ok(if is_rem {
            self.builder.build_int_signed_rem(l, r, "rem").unwrap()
        } else {
            self.builder.build_int_signed_div(l, r, "div").unwrap()
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
        let lt = self.local_tys.get(&left.0).cloned().unwrap_or(Type::Int);
        let rt = self.local_tys.get(&right.0).cloned().unwrap_or(Type::Int);
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
                BinOp::Add => self.builder.build_float_add(l, r, "fadd").unwrap(),
                BinOp::Sub => self.builder.build_float_sub(l, r, "fsub").unwrap(),
                BinOp::Mul => self.builder.build_float_mul(l, r, "fmul").unwrap(),
                BinOp::Div => self.builder.build_float_div(l, r, "fdiv").unwrap(),
                BinOp::Rem => self.builder.build_float_rem(l, r, "frem").unwrap(),
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
                    let c = self
                        .builder
                        .build_float_compare(pred, l, r, "fcmp")
                        .unwrap();
                    return Ok(self
                        .builder
                        .build_int_z_extend(c, self.i64_ty, "fcmpz")
                        .unwrap()
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
                if let Some(callee) = self.functions.get(&mangled).copied() {
                    let call = self
                        .builder
                        .build_call(callee, &[l.into(), r.into()], "num_ov")
                        .unwrap();
                    return Ok(call
                        .try_as_basic_value()
                        .basic()
                        .unwrap_or_else(|| self.i64_ty.const_int(0, false).into()));
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
                let z = self.i64_ty.const_int(0, false);
                let c = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, eq, z, "ne")
                    .unwrap();
                self.builder
                    .build_int_z_extend(c, self.i64_ty, "nez")
                    .unwrap()
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if let Some(name) = Self::adt_method_name(&lt, &rt) {
                    if self.functions.contains_key(&format!("__Ord_{name}_less")) {
                        // DESIGN less(self, other): a < b
                        let (left, right) = match op {
                            BinOp::Lt | BinOp::Ge => (l, r),
                            BinOp::Gt | BinOp::Le => (r, l),
                            _ => unreachable!(),
                        };
                        let less = self
                            .emit_less_override(&name, left, right)?
                            .expect("Ord.less");
                        let z = self.i64_ty.const_int(0, false);
                        return Ok(match op {
                            BinOp::Lt | BinOp::Gt => less.into(),
                            BinOp::Le | BinOp::Ge => {
                                // a <= b  iff  !(b < a); a >= b iff !(a < b)
                                let c = self
                                    .builder
                                    .build_int_compare(IntPredicate::EQ, less, z, "nless")
                                    .unwrap();
                                self.builder
                                    .build_int_z_extend(c, self.i64_ty, "nlessz")
                                    .unwrap()
                                    .into()
                            }
                            _ => unreachable!(),
                        });
                    }
                }
                // Structural Ord via runtime (String/Char/ADT); never SLT pointers.
                let f = self.module.get_function("lumia_cmp").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[l.into(), r.into()], "cmp")
                    .unwrap();
                let cmp = call.try_as_basic_value().basic().unwrap().into_int_value();
                let z = self.i64_ty.const_int(0, false);
                let pred = match op {
                    BinOp::Lt => IntPredicate::SLT,
                    BinOp::Le => IntPredicate::SLE,
                    BinOp::Gt => IntPredicate::SGT,
                    BinOp::Ge => IntPredicate::SGE,
                    _ => unreachable!(),
                };
                let c = self.builder.build_int_compare(pred, cmp, z, "ord").unwrap();
                self.builder
                    .build_int_z_extend(c, self.i64_ty, "ordz")
                    .unwrap()
            }
            BinOp::And => self.builder.build_and(l, r, "and").unwrap(),
            BinOp::Or => self.builder.build_or(l, r, "or").unwrap(),
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
        let ot = self.local_tys.get(&operand.0).cloned().unwrap_or(Type::Int);
        if matches!(ot, Type::Float) || matches!(ov, BasicValueEnum::FloatValue(_)) {
            let o = self.promote_f64(ov)?;
            let v = match op {
                UnOp::Neg => self.builder.build_float_neg(o, "fneg").unwrap(),
                UnOp::Not => bail!("not on Float"),
            };
            return Ok(v.into());
        }
        let o = self.as_i64(ov)?;
        let v = match op {
            UnOp::Neg => self.emit_checked_neg(o, fv)?,
            UnOp::Not => {
                let z = self.i64_ty.const_int(0, false);
                let c = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, o, z, "not")
                    .unwrap();
                self.builder
                    .build_int_z_extend(c, self.i64_ty, "notz")
                    .unwrap()
            }
        };
        Ok(v.into())
    }
}
