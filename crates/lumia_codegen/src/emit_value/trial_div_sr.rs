//! Trial-division strength reduction: after checking `d == 2`, step by 2.
//!
//! Matches (after opt/inline rename):
//! ```text
//! for d * d <= n {
//!   if n % d == 0 { ok = false; break }
//!   d = d + 1
//! }
//! ```
//! Latch becomes `d = (d == 2) ? 3 : d + 2`. Safe because any even composite
//! is already rejected at `d == 2`.

use inkwell::values::{BasicValueEnum, FunctionValue};
use inkwell::IntPredicate;
use lumia_core::{local_let_matches, Block, Local, Op, Value};
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use super::sr_pattern::{
    body_assigns_unit_inc, body_assigns_zero_or_false, header_le_const, header_name_sq_le_name,
    is_name_ne_zero, is_unit_inc, local_is_zero_or_false, name_of, rem_eq_zero_names,
};
use anyhow::{Context as AnyhowContext, Result};

#[derive(Debug)]
struct TrialDivLoop {
    d: String,
    n: String,
    ok: String,
}

#[derive(Debug)]
struct CountPrimesLoop {
    count: String,
    n: String,
    limit: i64,
}

impl<'ctx> Codegen<'ctx> {
    /// Outer `for n ≤ LIMIT { if isPrime(n) { c++ } }` → sieve runtime.
    pub(crate) fn try_emit_count_primes_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        _fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_count_primes_loop(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        // Sieve counts `2..=limit` from a zero accumulator.
        if !self.slot_known_eq(&pat.n, 2) || !self.slot_known_eq(&pat.count, 0) {
            return Ok(None);
        }
        let rt = self.runtime_fn("lumia_count_primes")?;
        let lim = self.llvm.i64_ty.const_int(pat.limit as u64, true);
        let call = crate::error::llvm(self.llvm.builder.build_call(rt, &[lim.into()], "nprimes"))?;
        let c = call
            .try_as_basic_value()
            .basic()
            .context("count_primes result")?
            .into_int_value();
        self.store_slot_i64(&pat.count, c)?;
        self.store_slot_i64(
            &pat.n,
            self.llvm.i64_ty.const_int((pat.limit + 1) as u64, true),
        )?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
    }

    /// If `header`/`body`/`latch` form a trial-division loop, emit the odd-step path.
    pub(crate) fn try_emit_trial_div_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_trial_div_loop(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        self.emit_trial_div_loop_fast(&pat, fv)?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
    }

    fn emit_trial_div_loop_fast(
        &mut self,
        pat: &TrialDivLoop,
        fv: FunctionValue<'ctx>,
    ) -> Result<()> {
        let header_bb = self.llvm.context.append_basic_block(fv, "td_header");
        let body_bb = self.llvm.context.append_basic_block(fv, "td_body");
        let composite_bb = self.llvm.context.append_basic_block(fv, "td_composite");
        let step_bb = self.llvm.context.append_basic_block(fv, "td_step");
        let exit_bb = self.llvm.context.append_basic_block(fv, "td_exit");

        crate::error::llvm(self.llvm.builder.build_unconditional_branch(header_bb))?;

        // header: d * d <= n  (NSW mul: d starts at 2 and only grows by ≤2 under d*d <= n)
        self.llvm.builder.position_at_end(header_bb);
        let d = self.load_slot_i64(&pat.d)?;
        let n = self.load_slot_i64(&pat.n)?;
        let dd = crate::error::llvm(self.llvm.builder.build_int_nsw_mul(d, d, "td_dd"))?;
        let cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLE,
            dd,
            n,
            "td_le",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(cont, body_bb, exit_bb),
        )?;

        // body: if n % d == 0 → composite; else step
        self.llvm.builder.position_at_end(body_bb);
        let d = self.load_slot_i64(&pat.d)?;
        let n = self.load_slot_i64(&pat.n)?;
        // d ≥ 2 and n ≥ d*d ≥ 0 ⇒ unsigned rem is valid and cheaper.
        let rem = crate::error::llvm(self.llvm.builder.build_int_unsigned_rem(n, d, "td_rem"))?;
        let is_div = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::EQ,
            rem,
            self.llvm.i64_ty.const_int(0, false),
            "td_divides",
        ))?;
        crate::error::llvm(self.llvm.builder.build_conditional_branch(
            is_div,
            composite_bb,
            step_bb,
        ))?;

        self.llvm.builder.position_at_end(composite_bb);
        self.store_slot_i64(&pat.ok, self.llvm.i64_ty.const_int(0, false))?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(exit_bb))?;

        // d = (d == 2) ? 3 : d + 2
        self.llvm.builder.position_at_end(step_bb);
        let d = self.load_slot_i64(&pat.d)?;
        let two = self.llvm.i64_ty.const_int(2, false);
        let three = self.llvm.i64_ty.const_int(3, false);
        let is2 = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::EQ,
            d,
            two,
            "td_is2",
        ))?;
        let d_plus_2 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(d, two, "td_d2"))?;
        let next = crate::error::llvm(
            self.llvm
                .builder
                .build_select(is2, three, d_plus_2, "td_next"),
        )?
        .into_int_value();
        self.store_slot_i64(&pat.d, next)?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(header_bb))?;

        self.llvm.builder.position_at_end(exit_bb);
        Ok(())
    }
}

fn match_trial_div_loop(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<TrialDivLoop> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (d, n) = header_name_sq_le_name(header, defs)?;
    let ok = body_trial_parts(body, &d, &n, defs)?;
    Some(TrialDivLoop { d, n, ok })
}

/// `for n ≤ LIMIT { … trial-div on n …; if ok { c += 1 }; n += 1 }`.
fn match_count_primes_loop(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<CountPrimesLoop> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (n, limit) = header_le_const(header, defs)?;
    if limit < 2 {
        return None;
    }
    let mut ok_name: Option<String> = None;
    let mut saw_trial = false;
    // Nested trial loop on the same `n`.
    fn walk(
        b: &Block,
        n: &str,
        defs: &HashMap<u32, Value>,
        ok: &mut Option<String>,
        saw: &mut bool,
    ) {
        for op in &b.ops {
            if let Op::Let {
                value:
                    Value::Loop {
                        header,
                        body,
                        latch,
                    },
                ..
            } = op
            {
                if let Some(p) = match_trial_div_loop(header, body, latch, defs) {
                    if p.n == n {
                        *saw = true;
                        *ok = Some(p.ok);
                    }
                }
                walk(body, n, defs, ok, saw);
            }
            if let Op::Let {
                value:
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    },
                ..
            } = op
            {
                walk(then_block, n, defs, ok, saw);
                walk(else_block, n, defs, ok, saw);
            }
        }
    }
    walk(body, &n, defs, &mut ok_name, &mut saw_trial);
    if !saw_trial {
        return None;
    }
    let ok = ok_name?;
    let mut count_name: Option<String> = None;
    let mut saw_n_inc = false;
    for op in &body.ops {
        match op {
            Op::Assign {
                name,
                value: Local(v),
            } => {
                if name == &n && is_unit_inc(*v, &n, defs) {
                    saw_n_inc = true;
                }
            }
            Op::Let {
                value: Value::If {
                    cond, then_block, ..
                },
                ..
            } => {
                // `if ok { c += 1 }` — cond may be Name(ok), `ok != 0`, or the
                // SSA result of an inlined `isPrime` `If` (`Value::If` is not in
                // leaf_defs). Reject trial then-arms that clear `ok` (false/0).
                let mut then_defs = defs.clone();
                for top in &then_block.ops {
                    if let Op::Let { local, value, .. } = top {
                        then_defs.insert(local.0, value.clone());
                    }
                }
                let cond_ok = name_of(*cond, defs).as_deref() == Some(ok.as_str())
                    || is_name_ne_zero(*cond, &ok, defs)
                    || (local_let_matches(*cond, body, |v| matches!(v, Value::If { .. }))
                        && !body_assigns_zero_or_false(then_block, &ok, &then_defs));
                if !cond_ok {
                    continue;
                }
                for top in &then_block.ops {
                    if let Op::Assign {
                        name,
                        value: Local(v),
                    } = top
                    {
                        if name != &n && name != &ok && is_unit_inc(*v, name, defs) {
                            count_name = Some(name.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if saw_n_inc {
        Some(CountPrimesLoop {
            count: count_name?,
            n,
            limit,
        })
    } else {
        None
    }
}


/// Trial loop body: `if n % d == 0 { ok = false; break } else { d += 1 }` (or latch step).
fn body_trial_parts(body: &Block, d: &str, n: &str, defs: &HashMap<u32, Value>) -> Option<String> {
    let mut ok_name: Option<String> = None;
    let mut saw_break = false;
    let mut saw_step = false;

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
                if !rem_eq_zero_names(*cond, n, d, defs) {
                    return None;
                }
                // then: ok = false; break (Bool(false) may be local to then_block)
                let mut then_defs = defs.clone();
                for top in &then_block.ops {
                    if let Op::Let { local, value, .. } = top {
                        then_defs.insert(local.0, value.clone());
                    }
                }
                for top in &then_block.ops {
                    match top {
                        Op::Assign {
                            name,
                            value: Local(v),
                        } if local_is_zero_or_false(Local(*v), &then_defs) => {
                            ok_name = Some(name.clone());
                        }
                        Op::Break => saw_break = true,
                        _ => {}
                    }
                }
                if body_assigns_unit_inc(else_block, d, defs) {
                    saw_step = true;
                }
            }
            Op::Assign {
                name,
                value: Local(v),
            } if name == d && is_unit_inc(*v, d, defs) => {
                saw_step = true;
            }
            _ => {}
        }
    }

    if saw_break && saw_step {
        ok_name
    } else {
        None
    }
}

#[cfg(test)]
#[path = "trial_div_sr_tests.rs"]
mod match_tests;
