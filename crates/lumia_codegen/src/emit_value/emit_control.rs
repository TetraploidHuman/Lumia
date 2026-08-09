//! Value emission — control flow (if/loop)

use super::super::Codegen;
use anyhow::Result;
use inkwell::values::{BasicValueEnum, FunctionValue};
use inkwell::IntPredicate;
use lumia_core::Local;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_value_if(
        &mut self,
        cond: &Local,
        then_block: &lumia_core::Block,
        else_block: &lumia_core::Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        let c = self.as_i64(self.local(*cond)?)?;
        let zero = self.i64_ty.const_int(0, false);
        let cond_i1 = self
            .builder
            .build_int_compare(IntPredicate::NE, c, zero, "ifcond")
            .unwrap();
        let then_bb = self.context.append_basic_block(fv, "then");
        let else_bb = self.context.append_basic_block(fv, "else");
        let merge_bb = self.context.append_basic_block(fv, "merge");
        self.builder
            .build_conditional_branch(cond_i1, then_bb, else_bb)
            .unwrap();

        self.builder.position_at_end(then_bb);
        let then_raw = self
            .emit_scoped_block(then_block, fv)?
            .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
        let then_terminated = self
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some();
        let mut then_incoming_i = None;
        let mut then_incoming_f = None;
        if !then_terminated {
            let then_bb_end = self.builder.get_insert_block().unwrap();
            then_incoming_i = Some((self.coerce_i64(then_raw)?, then_bb_end));
            then_incoming_f = Some((self.promote_f64(then_raw)?, then_bb_end));
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }
        let then_is_float = matches!(then_raw, BasicValueEnum::FloatValue(_));

        self.builder.position_at_end(else_bb);
        let else_raw = self
            .emit_scoped_block(else_block, fv)?
            .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
        let else_terminated = self
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some();
        let mut else_incoming_i = None;
        let mut else_incoming_f = None;
        if !else_terminated {
            let else_bb_end = self.builder.get_insert_block().unwrap();
            else_incoming_i = Some((self.coerce_i64(else_raw)?, else_bb_end));
            else_incoming_f = Some((self.promote_f64(else_raw)?, else_bb_end));
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }
        let float_merge = then_is_float || matches!(else_raw, BasicValueEnum::FloatValue(_));

        self.builder.position_at_end(merge_bb);
        if float_merge {
            match (then_incoming_f, else_incoming_f) {
                (Some((tv, tb)), Some((ev, eb))) => {
                    let phi = self
                        .builder
                        .build_phi(self.context.f64_type(), "iftmpf")
                        .unwrap();
                    phi.add_incoming(&[(&tv, tb), (&ev, eb)]);
                    Ok(phi.as_basic_value())
                }
                (Some((tv, _)), None) | (None, Some((tv, _))) => Ok(tv.into()),
                (None, None) => Ok(self.context.f64_type().const_float(0.0).into()),
            }
        } else {
            match (then_incoming_i, else_incoming_i) {
                (Some((tv, tb)), Some((ev, eb))) => {
                    let phi = self.builder.build_phi(self.i64_ty, "iftmp").unwrap();
                    phi.add_incoming(&[(&tv, tb), (&ev, eb)]);
                    Ok(phi.as_basic_value())
                }
                (Some((tv, _)), None) | (None, Some((tv, _))) => Ok(tv.into()),
                (None, None) => Ok(self.i64_ty.const_int(0, false).into()),
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
        let header_bb = self.context.append_basic_block(fv, "loop_header");
        let body_bb = self.context.append_basic_block(fv, "loop_body");
        let latch_bb = self.context.append_basic_block(fv, "loop_latch");
        let exit_bb = self.context.append_basic_block(fv, "loop_exit");
        self.builder.build_unconditional_branch(header_bb).unwrap();

        // continue → latch (runs step); break → exit; both restore loop roots
        let loop_depth = self.root_depth;
        self.loop_stack.push((latch_bb, exit_bb, loop_depth));

        self.builder.position_at_end(header_bb);
        let cond_raw = self
            .emit_scoped_block(header, fv)?
            .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
        if self
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_none()
        {
            let c = self.coerce_i64(cond_raw)?;
            let zero = self.i64_ty.const_int(0, false);
            let cond_i1 = self
                .builder
                .build_int_compare(IntPredicate::NE, c, zero, "loopcond")
                .unwrap();
            self.builder
                .build_conditional_branch(cond_i1, body_bb, exit_bb)
                .unwrap();
        }

        self.builder.position_at_end(body_bb);
        let _ = self.emit_scoped_block(body, fv)?;
        if self
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_none()
        {
            self.builder.build_unconditional_branch(latch_bb).unwrap();
        }

        self.builder.position_at_end(latch_bb);
        let _ = self.emit_scoped_block(latch, fv)?;
        if self
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_none()
        {
            self.builder.build_unconditional_branch(header_bb).unwrap();
        }

        self.loop_stack.pop();
        self.builder.position_at_end(exit_bb);
        Ok(self.i64_ty.const_int(0, false).into())
    }
}
