//! Dead-code elimination for pure, non-trapping lets (DESIGN §7.2).
//!
//! Keeps anything that may trap or observe mutation (§2.4): Int arith / Neg,
//! effectful / trapping builtins, calls, allocs, and control flow. Unused
//! Float arith, comparisons, literals, and Local copies may be dropped.
//!
//! Liveness walks **nested** `If`/`Loop` bodies — shallow `for_each_local` alone
//! would drop loop-carried temps (e.g. `let z = 0` only read inside a loop).

use crate::ir_util::collect_float_locals;
use lumia_core::{
    collect_ssa_live_refs, for_each_local, for_each_nested_block_mut, Block, CoreFun, CoreModule,
    Op, Value,
};
use lumia_core::{CoreBinOp as BinOp, CoreUnOp as UnOp};
use rustc_hash::FxHashSet as HashSet;

pub(crate) struct DcePass;
impl DcePass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        for f in &mut module.functions {
            if f.external.is_some() {
                continue;
            }
            dce_fun(f);
        }
    }
}

fn dce_fun(f: &mut CoreFun) {
    let mut float_locals = HashSet::default();
    for (i, ty) in f.param_tys.iter().enumerate() {
        if matches!(ty, lumia_ty::Type::Float) {
            if let Some(p) = f.params.get(i) {
                float_locals.insert(p.0);
            }
        }
    }
    collect_float_locals(&f.body, &mut float_locals);
    dce_block(&mut f.body, &float_locals);
}

fn dce_block(block: &mut Block, float_locals: &HashSet<u32>) {
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } => {
                for_each_nested_block_mut(value, &mut |nested| {
                    dce_block(nested, float_locals);
                });
            }
            _ => {}
        }
    }

    let mut live = HashSet::default();
    collect_ssa_live_refs(block, &mut live);

    // Trapping / effectful lets stay even if their SSA name is unread.
    let mut changed = true;
    while changed {
        changed = false;
        for op in &block.ops {
            if let Op::Let { local, value, .. } = op {
                if must_keep(value, float_locals) && live.insert(local.0) {
                    changed = true;
                }
                if live.contains(&local.0) {
                    let before = live.len();
                    mark_uses_shallow(value, &mut live);
                    if live.len() != before {
                        changed = true;
                    }
                }
            }
        }
    }

    block.ops.retain(|op| match op {
        Op::Let { local, value, .. } => {
            live.contains(&local.0) || must_keep(value, float_locals)
        }
        _ => true,
    });
}

fn mark_uses_shallow(value: &Value, used: &mut HashSet<u32>) {
    for_each_local(value, &mut |l| {
        used.insert(l.0);
    });
}

fn must_keep(value: &Value, float_locals: &HashSet<u32>) -> bool {
    match value {
        Value::If { .. } | Value::Loop { .. } | Value::Lambda { .. } => true,
        Value::AllocList { .. }
        | Value::AllocSet { .. }
        | Value::AllocMap { .. }
        | Value::AllocAdt { .. }
        | Value::AllocClosure { .. } => true,
        Value::Call { .. } | Value::IndirectCall { .. } => true,
        Value::Name(_) => true,
        Value::Binary {
            op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem,
            left,
            right,
        } => !(float_locals.contains(&left.0) && float_locals.contains(&right.0)),
        Value::Unary {
            op: UnOp::Neg,
            operand,
        } => !float_locals.contains(&operand.0),
        Value::Builtin { name, .. } => crate::memo::builtin_may_trap_or_effect(name),
        _ => false,
    }
}

#[cfg(test)]
#[path = "dce_tests.rs"]
mod tests;
