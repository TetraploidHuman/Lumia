//! Rewrite trial-division `d = d + 1` to the odd-step select in Core.
//!
//! After checking `n % d == 0`, any even composite is already rejected at
//! `d == 2`, so the latch may be `d = (d == 2) ? 3 : d + 2`. Generic loop
//! emit is then enough; codegen no longer special-cases this shape.
//!
//! Runs in Debug and Release whenever Cargo feature `domain-sr` is on
//! (whole-fn `countPrimes` → RT stays Release-only via [`super::DomainSrPass`]).

use super::match_primes::match_trial_div_loop;
use lumia_core::{
    collect_leaf_defs, for_each_op_value_mut, is_unit_inc, max_local_in_fun, Block, CoreBinOp,
    CoreFun, CoreModule, Local, Op, Value,
};
use rustc_hash::FxHashMap as HashMap;

pub(crate) struct TrialDivOddPass;

impl TrialDivOddPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        for fun in &mut module.functions {
            if fun.external.is_some() {
                continue;
            }
            rewrite_fun(fun);
        }
    }
}

fn rewrite_fun(fun: &mut CoreFun) {
    let defs = collect_leaf_defs(&fun.body, false);
    let mut next = max_local_in_fun(fun).saturating_add(1);
    for_each_op_value_mut(&mut fun.body, &mut |value| {
        let Value::Loop { header, body, latch } = value else {
            return;
        };
        let Some(pat) = match_trial_div_loop(header, body, latch, &defs) else {
            return;
        };
        rewrite_odd_in_block(body, &pat.d, &defs, &mut next);
    });
}

fn rewrite_odd_in_block(
    block: &mut Block,
    d: &str,
    defs: &HashMap<u32, Value>,
    next: &mut u32,
) -> bool {
    let mut changed = false;
    for op in &mut block.ops {
        if let Op::Let {
            value: Value::If {
                then_block,
                else_block,
                ..
            },
            ..
        } = op
        {
            changed |= rewrite_odd_in_block(then_block, d, defs, next);
            changed |= rewrite_odd_in_block(else_block, d, defs, next);
        }
    }
    changed |= replace_unit_inc_assigns(block, d, defs, next);
    changed
}

fn replace_unit_inc_assigns(
    block: &mut Block,
    d: &str,
    defs: &HashMap<u32, Value>,
    next: &mut u32,
) -> bool {
    let mut new_ops = Vec::with_capacity(block.ops.len());
    let mut changed = false;
    for op in std::mem::take(&mut block.ops) {
        match &op {
            Op::Assign {
                name,
                value: Local(v),
            } if name == d && is_unit_inc(*v, d, defs) => {
                changed = true;
                let nxt = emit_odd_step_lets(&mut new_ops, d, next);
                new_ops.push(Op::Assign {
                    name: d.to_string(),
                    value: nxt,
                });
            }
            _ => new_ops.push(op),
        }
    }
    block.ops = new_ops;
    changed
}

fn emit_odd_step_lets(ops: &mut Vec<Op>, d: &str, next: &mut u32) -> Local {
    let two = alloc(next);
    let three = alloc(next);
    let dload = alloc(next);
    let is2 = alloc(next);
    let d2 = alloc(next);
    let nxt = alloc(next);
    ops.push(let_pure(two, Value::Int(2)));
    ops.push(let_pure(three, Value::Int(3)));
    ops.push(let_pure(dload, Value::Name(d.to_string())));
    ops.push(let_pure(
        is2,
        Value::Binary {
            op: CoreBinOp::Eq,
            left: dload,
            right: two,
        },
    ));
    ops.push(let_pure(
        d2,
        Value::Binary {
            op: CoreBinOp::Add,
            left: dload,
            right: two,
        },
    ));
    ops.push(let_pure(
        nxt,
        Value::If {
            cond: is2,
            then_block: Box::new(Block {
                ops: vec![],
                result: Some(three),
            }),
            else_block: Box::new(Block {
                ops: vec![],
                result: Some(d2),
            }),
        },
    ));
    nxt
}

fn alloc(next: &mut u32) -> Local {
    let l = Local(*next);
    *next = next.saturating_add(1);
    l
}

fn let_pure(local: Local, value: Value) -> Op {
    Op::Let {
        local,
        value,
        pure_region: true,
    }
}

#[cfg(test)]
#[path = "trial_div_odd_tests.rs"]
mod tests;
