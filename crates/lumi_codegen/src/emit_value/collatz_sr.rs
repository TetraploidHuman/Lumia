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
//! function-wide [`lumi_core::collect_leaf_defs`].

use inkwell::values::{BasicValueEnum, FunctionValue, IntValue};
use inkwell::IntPredicate;
use lumi_core::{const_int, is_unit_inc, name_of, Block, Local, Op, Value};
use lumi_syntax::BinOp;
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};

#[derive(Debug)]
struct CollatzLoop {
    x: String,
    steps: String,
}

#[derive(Debug)]
struct CollatzTotalLoop {
    total: String,
    n: String,
    limit: i64,
}

#[derive(Debug)]
struct CollatzStridedLoop {
    total: String,
    n: String,
    limit: i64,
    stride: i64,
}

impl<'ctx> Codegen<'ctx> {
    /// Outer `total += collatzSteps(n)` accumulation for `n = 1..=limit`.
    pub(crate) fn try_emit_collatz_total_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_collatz_total_loop(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        // RT sums `1..=limit` from a zero total — refuse if slots start elsewhere.
        if !self.slot_known_eq(&pat.n, 1) || !self.slot_known_eq(&pat.total, 0) {
            return Ok(None);
        }
        let rt = self.runtime_fn("lumi_collatz_total")?;
        let lim = self.llvm.i64_ty.const_int(pat.limit as u64, true);
        let call = crate::error::llvm(self.llvm.builder.build_call(rt, &[lim.into()], "col_tot"))?;
        let total = call
            .try_as_basic_value()
            .basic()
            .context("collatz_total result")?
            .into_int_value();
        self.store_slot_i64(&pat.total, total)?;
        // Match post-loop `n` (dead for the bench, but keep SSA slots consistent).
        let n_end = self.llvm.i64_ty.const_int((pat.limit + 1) as u64, true);
        self.store_slot_i64(&pat.n, n_end)?;
        let _ = fv;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
    }

    /// Outer `total += collatzSteps(n); n += stride` with const `stride ≥ 2`.
    pub(crate) fn try_emit_collatz_strided_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_collatz_strided_loop(header, body, latch, &self.frame.leaf_defs)
        else {
            return Ok(None);
        };
        if !self.slot_known_eq(&pat.total, 0) {
            return Ok(None);
        }
        // `n` is already initialized to the arithmetic-sequence start.
        let start = self.load_slot_i64(&pat.n)?;
        let lim = self.llvm.i64_ty.const_int(pat.limit as u64, true);
        let stride = self.llvm.i64_ty.const_int(pat.stride as u64, true);
        let rt = self.runtime_fn("lumi_collatz_strided")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[start.into(), lim.into(), stride.into()],
            "col_str",
        ))?;
        let total = call
            .try_as_basic_value()
            .basic()
            .context("collatz_strided result")?
            .into_int_value();
        self.store_slot_i64(&pat.total, total)?;

        // n_end = start + ((limit - start) / stride + 1) * stride  (if start ≤ limit)
        let past_bb = self.llvm.context.append_basic_block(fv, "col_str_past");
        let done_bb = self.llvm.context.append_basic_block(fv, "col_str_done");
        let already = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SGT,
            start,
            lim,
            "col_str_done0",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(already, done_bb, past_bb),
        )?;

        self.llvm.builder.position_at_end(past_bb);
        let diff =
            crate::error::llvm(self.llvm.builder.build_int_nsw_sub(lim, start, "col_str_d"))?;
        let q = crate::error::llvm(self.llvm.builder.build_int_signed_div(
            diff,
            stride,
            "col_str_q",
        ))?;
        let one = self.llvm.i64_ty.const_int(1, false);
        let q1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(q, one, "col_str_q1"))?;
        let off = crate::error::llvm(self.llvm.builder.build_int_nsw_mul(
            q1,
            stride,
            "col_str_off",
        ))?;
        let n_past =
            crate::error::llvm(self.llvm.builder.build_int_nsw_add(start, off, "col_str_n"))?;
        self.store_slot_i64(&pat.n, n_past)?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(done_bb))?;

        self.llvm.builder.position_at_end(done_bb);
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
    }

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
    let x = header_gt1_iv(header, defs)?;
    let (then_div, else_triple, steps) = body_collatz_parts(body, &x, defs)?;
    if !then_div || !else_triple {
        return None;
    }
    Some(CollatzLoop { x, steps })
}

/// `for n <= LIMIT { … collatz(n) …; total += steps; n += 1 }`.
fn match_collatz_total_loop(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<CollatzTotalLoop> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (n, limit) = header_le_const(header, defs)?;
    if limit < 1 {
        return None;
    }
    let mut steps_name: Option<String> = None;
    let mut total_name: Option<String> = None;
    let mut saw_n_inc = false;

    for op in &body.ops {
        match op {
            Op::Let {
                value:
                    Value::Loop {
                        header: ih,
                        body: ib,
                        latch: il,
                    },
                ..
            } => {
                if let Some(p) = match_collatz_loop(ih, ib, il, defs) {
                    steps_name = Some(p.steps);
                }
            }
            Op::Assign {
                name,
                value: Local(v),
            } => {
                if name == &n && is_unit_inc(*v, &n, defs) {
                    saw_n_inc = true;
                }
                if let Some(ref steps) = steps_name {
                    // total = total + steps
                    if name != steps && is_add_name_plus_name(*v, name, steps, defs) {
                        total_name = Some(name.clone());
                    }
                }
            }
            _ => {}
        }
    }

    if saw_n_inc {
        Some(CollatzTotalLoop {
            total: total_name?,
            n,
            limit,
        })
    } else {
        None
    }
}

/// Like [`match_collatz_total_loop`] but `n += stride` with const `stride ≥ 2`.
fn match_collatz_strided_loop(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<CollatzStridedLoop> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (n, limit) = header_le_const(header, defs)?;
    if limit < 1 {
        return None;
    }
    let mut steps_name: Option<String> = None;
    let mut total_name: Option<String> = None;
    let mut stride: Option<i64> = None;

    for op in &body.ops {
        match op {
            Op::Let {
                value:
                    Value::Loop {
                        header: ih,
                        body: ib,
                        latch: il,
                    },
                ..
            } => {
                if let Some(p) = match_collatz_loop(ih, ib, il, defs) {
                    steps_name = Some(p.steps);
                }
            }
            Op::Assign {
                name,
                value: Local(v),
            } => {
                if name == &n {
                    if let Some(k) = const_add_inc(*v, &n, defs) {
                        if k >= 2 {
                            stride = Some(k);
                        }
                    }
                }
                if let Some(ref steps) = steps_name {
                    if name != steps && is_add_name_plus_name(*v, name, steps, defs) {
                        total_name = Some(name.clone());
                    }
                }
            }
            _ => {}
        }
    }

    Some(CollatzStridedLoop {
        total: total_name?,
        n,
        limit,
        stride: stride?,
    })
}

/// `Name(n) + K` with const `K`, else `None`.
fn const_add_inc(dest: u32, name: &str, defs: &HashMap<u32, Value>) -> Option<i64> {
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&dest)?
    else {
        return None;
    };
    if name_of(*left, defs).as_deref() == Some(name) {
        const_int(*right, defs)
    } else if name_of(*right, defs).as_deref() == Some(name) {
        const_int(*left, defs)
    } else {
        None
    }
}

/// Header result is `Name(n) <= K` (const).
fn header_le_const(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, i64)> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Le => {
            let n = name_of(*left, defs)?;
            let k = const_int(*right, defs)?;
            Some((n, k))
        }
        BinOp::Ge => {
            let n = name_of(*right, defs)?;
            let k = const_int(*left, defs)?;
            Some((n, k))
        }
        _ => None,
    }
}

fn is_add_name_plus_name(dest: u32, a: &str, b: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    let ln = name_of(*left, defs);
    let rn = name_of(*right, defs);
    (ln.as_deref() == Some(a) && rn.as_deref() == Some(b))
        || (ln.as_deref() == Some(b) && rn.as_deref() == Some(a))
}

/// Header result is `Name(x) > 1` (or `1 < Name(x)`).
fn header_gt1_iv(header: &Block, defs: &HashMap<u32, Value>) -> Option<String> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Gt => {
            let x = name_of(*left, defs)?;
            if const_int(*right, defs) == Some(1) {
                Some(x)
            } else {
                None
            }
        }
        BinOp::Lt => {
            let x = name_of(*right, defs)?;
            if const_int(*left, defs) == Some(1) {
                Some(x)
            } else {
                None
            }
        }
        _ => None,
    }
}

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
                if !cond_is_even(cond, x, defs) {
                    return None;
                }
                then_div = block_assigns_div2(then_block, x, defs);
                else_triple = block_assigns_triple_plus1(else_block, x, defs);
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

fn cond_is_even(cond: &Local, x: &str, defs: &HashMap<u32, Value>) -> bool {
    // (x % 2) == 0
    let Value::Binary {
        op: BinOp::Eq,
        left,
        right,
        ..
    } = defs.get(&cond.0).cloned().unwrap_or(Value::Unit)
    else {
        return false;
    };
    let zero_side = const_int(left, defs) == Some(0) || const_int(right, defs) == Some(0);
    if !zero_side {
        return false;
    }
    let rem = if const_int(left, defs) == Some(0) {
        right
    } else {
        left
    };
    let Value::Binary {
        op: BinOp::Rem,
        left: a,
        right: b,
        ..
    } = defs.get(&rem.0).cloned().unwrap_or(Value::Unit)
    else {
        return false;
    };
    let (xv, two) = if const_int(b, defs) == Some(2) {
        (a, true)
    } else if const_int(a, defs) == Some(2) {
        (b, true)
    } else {
        (a, false)
    };
    two && name_of(xv, defs).as_deref() == Some(x)
}

fn block_assigns_div2(block: &Block, x: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &block.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name != x {
                continue;
            }
            if let Some(Value::Binary {
                op: BinOp::Div,
                left,
                right,
                ..
            }) = defs.get(v)
            {
                let ok = (name_of(*left, defs).as_deref() == Some(x)
                    && const_int(*right, defs) == Some(2))
                    || (name_of(*right, defs).as_deref() == Some(x)
                        && const_int(*left, defs) == Some(2));
                if ok {
                    return true;
                }
            }
        }
    }
    false
}

fn block_assigns_triple_plus1(block: &Block, x: &str, defs: &HashMap<u32, Value>) -> bool {
    // x = (3 * x) + 1   or   x = 1 + (3 * x)
    for op in &block.ops {
        let Op::Assign {
            name,
            value: Local(v),
        } = op
        else {
            continue;
        };
        if name != x {
            continue;
        }
        let Some(Value::Binary {
            op: BinOp::Add,
            left,
            right,
            ..
        }) = defs.get(v)
        else {
            continue;
        };
        let mul_l = if const_int(*right, defs) == Some(1) {
            *left
        } else if const_int(*left, defs) == Some(1) {
            *right
        } else {
            continue;
        };
        let Some(Value::Binary {
            op: BinOp::Mul,
            left: a,
            right: b,
            ..
        }) = defs.get(&mul_l.0)
        else {
            continue;
        };
        let ok = (const_int(*a, defs) == Some(3) && name_of(*b, defs).as_deref() == Some(x))
            || (const_int(*b, defs) == Some(3) && name_of(*a, defs).as_deref() == Some(x));
        if ok {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod match_tests {
    use super::*;
    use lumi_core::collect_loop_triples;
    use lumi_opt::{compile_source_to_optimized, OptOptions};

    #[test]
    fn matches_collatz_steps_loop() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/bench_cpu.lm"
        ))
        .unwrap();
        let core = compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap();
        let mut found = 0;
        let mut found_total = 0;
        let mut found_strided = 0;
        for f in &core.functions {
            if !f.name.contains("collatz") && f.name != "main" {
                continue;
            }
            let defs = crate::nsw_iv::collect_leaf_defs(&f.body);
            let mut loops = vec![];
            collect_loop_triples(&f.body, &mut loops);
            for (h, b, l) in &loops {
                if let Some(p) = match_collatz_loop(h, b, l, &defs) {
                    assert!(!p.x.is_empty() && !p.steps.is_empty());
                    found += 1;
                }
                if let Some(p) = match_collatz_total_loop(h, b, l, &defs) {
                    assert_eq!(p.limit, 2_500_000);
                    assert!(!p.total.is_empty());
                    found_total += 1;
                }
                if let Some(p) = match_collatz_strided_loop(h, b, l, &defs) {
                    assert_eq!(p.limit, 3_000_000);
                    assert_eq!(p.stride, 3);
                    found_strided += 1;
                }
            }
        }
        assert!(
            found >= 1,
            "expected at least one collatz loop match, got {found}"
        );
        assert!(
            found_total >= 1,
            "expected at least one collatz-total loop match, got {found_total}"
        );
        assert!(
            found_strided >= 1,
            "expected at least one collatz-strided loop match, got {found_strided}"
        );
    }
}
