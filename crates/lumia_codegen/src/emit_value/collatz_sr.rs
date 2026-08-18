//! Recognize classic Collatz step loops and emit a `cttz`-batched lowering.
//!
//! Matches (after opt/inline rename):
//! ```text
//! for x > 1 {
//!   if x % 2 == 0 { x = x / 2 } else { x = 3 * x + 1 }
//!   steps = steps + 1
//! }
//! ```
//! Even runs become `k = cttz(x); x >>= k; steps += k`.
//!
//! Note: LICM may hoist `Int` literals outside the loop; matching therefore uses
//! function-wide [`lumia_core::collect_leaf_defs`].

use inkwell::values::{BasicValueEnum, FunctionValue, IntValue};
use inkwell::IntPredicate;
use lumia_core::{Block, Local, Op, Value};
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use super::sr_pattern::{
    body_assigns_name_div_const, body_assigns_name_mul_const_plus_const, header_gt_eq,
    is_name_rem_eq_const, is_unit_inc,
};
use anyhow::{Context as AnyhowContext, Result};

#[derive(Debug)]
struct CollatzLoop {
    x: String,
    steps: String,
}

impl<'ctx> Codegen<'ctx> {
    /// If `header`/`body`/`latch` form a Collatz steps loop, emit the fast path.
    pub(crate) fn try_emit_collatz_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_collatz_loop(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        self.emit_collatz_loop_fast(&pat, fv)?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
    }

    fn emit_collatz_loop_fast(&mut self, pat: &CollatzLoop, fv: FunctionValue<'ctx>) -> Result<()> {
        let header_bb = self.llvm.context.append_basic_block(fv, "col_header");
        let body_bb = self.llvm.context.append_basic_block(fv, "col_body");
        let even_bb = self.llvm.context.append_basic_block(fv, "col_even");
        let odd_bb = self.llvm.context.append_basic_block(fv, "col_odd");
        let latch_bb = self.llvm.context.append_basic_block(fv, "col_latch");
        let exit_bb = self.llvm.context.append_basic_block(fv, "col_exit");

        crate::error::llvm(self.llvm.builder.build_unconditional_branch(header_bb))?;

        // header: x > 1
        self.llvm.builder.position_at_end(header_bb);
        let x = self.load_slot_i64(&pat.x)?;
        let one = self.llvm.i64_ty.const_int(1, false);
        let cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SGT,
            x,
            one,
            "col_gt1",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(cont, body_bb, exit_bb),
        )?;

        // body: branch on LSB
        self.llvm.builder.position_at_end(body_bb);
        let x = self.load_slot_i64(&pat.x)?;
        let odd_bit = crate::error::llvm(self.llvm.builder.build_and(x, one, "col_lsb"))?;
        let is_odd = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::NE,
            odd_bit,
            self.llvm.i64_ty.const_int(0, false),
            "col_odd",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(is_odd, odd_bb, even_bb),
        )?;

        // even: k = cttz(x); x >>= k; steps += k
        self.llvm.builder.position_at_end(even_bb);
        let x = self.load_slot_i64(&pat.x)?;
        let k = self.emit_cttz_i64(x)?;
        let x2 = crate::error::llvm(self.llvm.builder.build_right_shift(x, k, false, "col_shr"))?;
        self.store_slot_i64(&pat.x, x2)?;
        let steps = self.load_slot_i64(&pat.steps)?;
        let steps2 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(steps, k, "col_addk"))?;
        self.store_slot_i64(&pat.steps, steps2)?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(latch_bb))?;

        // odd: x = 3*x+1; steps += 1
        self.llvm.builder.position_at_end(odd_bb);
        let x = self.load_slot_i64(&pat.x)?;
        let three = self.llvm.i64_ty.const_int(3, false);
        let mul = crate::error::llvm(self.llvm.builder.build_int_nsw_mul(x, three, "col_mul3"))?;
        let x2 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(mul, one, "col_add1"))?;
        self.store_slot_i64(&pat.x, x2)?;
        let steps = self.load_slot_i64(&pat.steps)?;
        let steps2 =
            crate::error::llvm(self.llvm.builder.build_int_nsw_add(steps, one, "col_add1s"))?;
        self.store_slot_i64(&pat.steps, steps2)?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(latch_bb))?;

        self.llvm.builder.position_at_end(latch_bb);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(header_bb))?;

        self.llvm.builder.position_at_end(exit_bb);
        Ok(())
    }

    fn emit_cttz_i64(&self, x: IntValue<'ctx>) -> Result<IntValue<'ctx>> {
        let name = "llvm.cttz.i64";
        let intrinsic = inkwell::intrinsics::Intrinsic::find(name)
            .with_context(|| format!("missing intrinsic {name}"))?;
        let decl = intrinsic
            .get_declaration(&self.llvm.module, &[self.llvm.i64_ty.into()])
            .context("cttz declaration")?;
        // is_zero_poison = false (i1 0): well-defined at zero (returns 64).
        let is_zero_undef = self.llvm.context.bool_type().const_int(0, false);
        let call = crate::error::llvm(self.llvm.builder.build_call(
            decl,
            &[x.into(), is_zero_undef.into()],
            "cttz",
        ))?;
        Ok(call
            .try_as_basic_value()
            .basic()
            .context("cttz result")?
            .into_int_value())
    }

    pub(crate) fn load_slot_i64(&mut self, name: &str) -> Result<IntValue<'ctx>> {
        let v = self.load_slot(name)?;
        self.as_i64(v)
    }

    pub(crate) fn store_slot_i64(&mut self, name: &str, v: IntValue<'ctx>) -> Result<()> {
        let ptr = *self
            .frame
            .slots
            .get(name)
            .with_context(|| format!("missing slot {name}"))?;
        crate::error::llvm(self.llvm.builder.build_store(ptr, v))?;
        self.note_slot_i64_const(name, v);
        Ok(())
    }
}

fn match_collatz_loop(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<CollatzLoop> {
    // Latch should be empty (no ops) for the classic lowering.
    if !latch.ops.is_empty() {
        return None;
    }
    let x = header_gt_eq(header, 1, defs)?;
    let (then_div, else_triple, steps) = body_collatz_parts(body, &x, defs)?;
    if !then_div || !else_triple {
        return None;
    }
    Some(CollatzLoop { x, steps })
}

/// Expect: Let If { ... }; Assign steps = steps+1  (order may vary slightly)
fn body_collatz_parts(
    body: &Block,
    x: &str,
    defs: &HashMap<u32, Value>,
) -> Option<(bool, bool, String)> {
    // Expect: Let If { ... }; Assign steps = steps+1  (order may vary slightly)
    let mut then_div = false;
    let mut else_triple = false;
    let mut steps: Option<String> = None;

    for op in &body.ops {
        match op {
            Op::Let {
                value:
                    Value::If {
                        cond,
                        then_block,
                        else_block,
                    },
                ..
            } => {
                if !is_name_rem_eq_const(*cond, x, 2, defs) {
                    return None;
                }
                then_div = body_assigns_name_div_const(then_block, x, 2, defs);
                else_triple = body_assigns_name_mul_const_plus_const(else_block, x, 3, 1, defs);
            }
            Op::Assign {
                name,
                value: Local(v),
            } => {
                if is_unit_inc(*v, name, defs) {
                    steps = Some(name.clone());
                }
            }
            _ => {}
        }
    }
    Some((then_div, else_triple, steps?))
}

#[cfg(test)]
#[path = "collatz_sr_tests.rs"]
mod match_tests;
