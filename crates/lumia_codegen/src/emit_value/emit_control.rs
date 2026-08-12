//! Value emission — control flow (if/loop)

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue};
use inkwell::IntPredicate;
use lumia_core::{Local, Value};
use lumia_syntax::BinOp;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    /// Core stores Bool as i64 0/1 (via `zext` of `icmp`). Prefer `trunc` to i1
    /// over `icmp ne 0` when the value is known boolean — saves a compare on
    /// every loop/if latch.
    pub(crate) fn as_cond_i1(
        &self,
        bits: IntValue<'ctx>,
        src: Option<Local>,
    ) -> Result<IntValue<'ctx>> {
        let i1 = self.llvm.context.bool_type();
        let use_trunc = src.is_some_and(|l| {
            matches!(self.frame.local_tys.get(&l.0), Some(Type::Bool))
                || matches!(
                    self.frame.leaf_defs.get(&l.0),
                    Some(
                        Value::Bool(_)
                            | Value::Binary {
                                op: BinOp::Eq
                                    | BinOp::Ne
                                    | BinOp::Lt
                                    | BinOp::Le
                                    | BinOp::Gt
                                    | BinOp::Ge
                                    | BinOp::And
                                    | BinOp::Or,
                                ..
                            }
                    )
                )
        });
        if use_trunc {
            return crate::error::llvm(self.llvm.builder.build_int_truncate(bits, i1, "cond"));
        }
        let zero = self.llvm.i64_ty.const_int(0, false);
        crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::NE,
            bits,
            zero,
            "cond_ne",
        ))
    }

    pub(crate) fn emit_value_if(
        &mut self,
        cond: &Local,
        then_block: &lumia_core::Block,
        else_block: &lumia_core::Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        let c = self.as_i64(self.local(*cond)?)?;
        let cond_i1 = self.as_cond_i1(c, Some(*cond))?;
        let then_bb = self.llvm.context.append_basic_block(fv, "then");
        let else_bb = self.llvm.context.append_basic_block(fv, "else");
        let merge_bb = self.llvm.context.append_basic_block(fv, "merge");
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(cond_i1, then_bb, else_bb),
        )?;

        self.llvm.builder.position_at_end(then_bb);
        let then_raw = self
            .emit_scoped_block(then_block, fv)?
            .unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into());
        let then_terminated = self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some();
        let mut then_incoming_i = None;
        let mut then_incoming_f = None;
        if !then_terminated {
            let then_bb_end = self
                .llvm
                .builder
                .get_insert_block()
                .context("insert block")?;
            then_incoming_i = Some((self.coerce_i64(then_raw)?, then_bb_end));
            then_incoming_f = Some((self.promote_f64(then_raw)?, then_bb_end));
            crate::error::llvm(self.llvm.builder.build_unconditional_branch(merge_bb))?;
        }
        let then_is_float = matches!(then_raw, BasicValueEnum::FloatValue(_));

        self.llvm.builder.position_at_end(else_bb);
        let else_raw = self
            .emit_scoped_block(else_block, fv)?
            .unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into());
        let else_terminated = self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some();
        let mut else_incoming_i = None;
        let mut else_incoming_f = None;
        if !else_terminated {
            let else_bb_end = self
                .llvm
                .builder
                .get_insert_block()
                .context("insert block")?;
            else_incoming_i = Some((self.coerce_i64(else_raw)?, else_bb_end));
            else_incoming_f = Some((self.promote_f64(else_raw)?, else_bb_end));
            crate::error::llvm(self.llvm.builder.build_unconditional_branch(merge_bb))?;
        }
        let float_merge = then_is_float || matches!(else_raw, BasicValueEnum::FloatValue(_));

        self.llvm.builder.position_at_end(merge_bb);
        if float_merge {
            match (then_incoming_f, else_incoming_f) {
                (Some((tv, tb)), Some((ev, eb))) => {
                    let phi = crate::error::llvm(
                        self.llvm
                            .builder
                            .build_phi(self.llvm.context.f64_type(), "iftmpf"),
                    )?;
                    phi.add_incoming(&[(&tv, tb), (&ev, eb)]);
                    Ok(phi.as_basic_value())
                }
                (Some((tv, _)), None) | (None, Some((tv, _))) => Ok(tv.into()),
                (None, None) => Ok(self.llvm.context.f64_type().const_float(0.0).into()),
            }
        } else {
            match (then_incoming_i, else_incoming_i) {
                (Some((tv, tb)), Some((ev, eb))) => {
                    let phi =
                        crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "iftmp"))?;
                    phi.add_incoming(&[(&tv, tb), (&ev, eb)]);
                    Ok(phi.as_basic_value())
                }
                (Some((tv, _)), None) | (None, Some((tv, _))) => Ok(tv.into()),
                (None, None) => Ok(self.llvm.i64_ty.const_int(0, false).into()),
            }
        }
    }

    pub(crate) fn emit_value_loop(
        &mut self,
        header: &lumia_core::Block,
        body: &lumia_core::Block,
        latch: &lumia_core::Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        let header_bb = self.llvm.context.append_basic_block(fv, "loop_header");
        let body_bb = self.llvm.context.append_basic_block(fv, "loop_body");
        let latch_bb = self.llvm.context.append_basic_block(fv, "loop_latch");
        let exit_bb = self.llvm.context.append_basic_block(fv, "loop_exit");
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(header_bb))?;

        // continue → latch (runs step); break → exit; both restore loop roots
        let loop_depth = self.frame.root_depth;
        self.frame.loop_stack.push((latch_bb, exit_bb, loop_depth));

        self.llvm.builder.position_at_end(header_bb);
        let cond_raw = self
            .emit_scoped_block(header, fv)?
            .unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into());
        if self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_none()
        {
            let c = self.coerce_i64(cond_raw)?;
            // Loop header result is the condition local when present.
            let cond_i1 = self.as_cond_i1(c, header.result)?;
            crate::error::llvm(
                self.llvm
                    .builder
                    .build_conditional_branch(cond_i1, body_bb, exit_bb),
            )?;
        }

        self.llvm.builder.position_at_end(body_bb);
        let _ = self.emit_scoped_block(body, fv)?;
        if self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_none()
        {
            crate::error::llvm(self.llvm.builder.build_unconditional_branch(latch_bb))?;
        }

        self.llvm.builder.position_at_end(latch_bb);
        let _ = self.emit_scoped_block(latch, fv)?;
        if self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_none()
        {
            crate::error::llvm(self.llvm.builder.build_unconditional_branch(header_bb))?;
        }

        self.frame.loop_stack.pop();
        self.llvm.builder.position_at_end(exit_bb);
        Ok(self.llvm.i64_ty.const_int(0, false).into())
    }
}
