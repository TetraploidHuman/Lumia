//! Binary / float / Ord / unary value emission.

use super::super::super::Codegen;
use anyhow::{bail, Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue};
use inkwell::{FloatPredicate, IntPredicate};
use lumia_core::{CoreBinOp as BinOp, CoreUnOp as UnOp, Local};
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
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
            BinOp::Add if self.dest_is_nsw_safe() => {
                self.emit_nsw_binop(l, r, left, right, "add")?
            }
            BinOp::Sub if self.dest_is_nsw_safe() => self.emit_nsw_binop_sub(l, r, "sub")?,
            BinOp::Mul if self.dest_is_nsw_safe() => {
                self.emit_nsw_binop_mul(l, r, left, right, "mul")?
            }
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
