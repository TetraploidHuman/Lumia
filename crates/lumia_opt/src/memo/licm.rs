use crate::builtin_effect::builtin_may_trap_or_effect;
use crate::ir_util::collect_float_locals;
use lumia_core::{
    flat_map_top_level_ops_in_block, for_each_ctrl_nested_block_mut, Block, Op, Value,
};
use lumia_core::{CoreBinOp as BinOp, CoreUnOp as UnOp};
use rustc_hash::FxHashSet as HashSet;

pub(crate) fn licm_seeded(block: &mut Block, mut float_locals: HashSet<u32>) {
    collect_float_locals(block, &mut float_locals);
    licm_block_with_floats(block, &float_locals);
}

fn licm_block_with_floats(block: &mut Block, float_locals: &HashSet<u32>) {
    flat_map_top_level_ops_in_block(block, &mut |mut op| {
        if let Op::Let {
            value:
                Value::Loop {
                    header,
                    body,
                    latch,
                },
            ..
        } = &mut op
        {
            licm_block_with_floats(header, float_locals);
            licm_block_with_floats(body, float_locals);
            licm_block_with_floats(latch, float_locals);

            let mut loop_defs = HashSet::default();
            collect_defs(header, &mut loop_defs);
            collect_defs(body, &mut loop_defs);
            collect_defs(latch, &mut loop_defs);

            let mut hoisted = Vec::new();
            loop {
                let mut round = Vec::new();
                flat_map_top_level_ops_in_block(body, &mut |hop| {
                    if let Op::Let {
                        local,
                        value,
                        pure_region: true,
                    } = &hop
                    {
                        if is_hoistable(value, &loop_defs, float_locals) {
                            loop_defs.remove(&local.0);
                            round.push(hop);
                            return vec![];
                        }
                    }
                    vec![hop]
                });
                if round.is_empty() {
                    break;
                }
                hoisted.extend(round);
            }
            let mut out = hoisted;
            out.push(op);
            return out;
        }
        if let Op::Let { value, .. } = &mut op {
            for_each_ctrl_nested_block_mut(value, &mut |nested| {
                licm_block_with_floats(nested, float_locals);
            });
        }
        vec![op]
    });
}

fn collect_defs(block: &Block, defs: &mut HashSet<u32>) {
    // Same DFS collector as inline/captures (Lambda params included — more conservative).
    lumia_core::collect_defined_locals(block, defs);
}

fn is_hoistable(value: &Value, loop_defs: &HashSet<u32>, float_locals: &HashSet<u32>) -> bool {
    match value {
        // Don't hoist control / alloc / names (may observe mutation).
        Value::If { .. }
        | Value::Loop { .. }
        | Value::Lambda { .. }
        | Value::Name(_)
        | Value::AllocList { .. }
        | Value::AllocSet { .. }
        | Value::AllocMap { .. }
        | Value::AllocAdt { .. }
        | Value::AllocClosure { .. }
        | Value::IndirectCall { .. } => false,
        // Checked Int arithmetic / Neg may trap — must not hoist past break (§2.4).
        // IEEE Float arith is fine to hoist when both operands are loop-invariant.
        Value::Binary {
            op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem,
            left,
            right,
        } => {
            float_locals.contains(&left.0)
                && float_locals.contains(&right.0)
                && !loop_defs.contains(&left.0)
                && !loop_defs.contains(&right.0)
        }
        Value::Unary {
            op: UnOp::Neg,
            operand,
        } => float_locals.contains(&operand.0) && !loop_defs.contains(&operand.0),
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::FunRef(_) => true,
        Value::Local(l) => !loop_defs.contains(&l.0),
        Value::Unary { operand, .. } => !loop_defs.contains(&operand.0),
        Value::Binary { left, right, .. } => {
            !loop_defs.contains(&left.0) && !loop_defs.contains(&right.0)
        }
        // User calls / builtins may trap or allocate; only hoist trivial locals above.
        Value::Call { .. } => false,
        Value::Builtin { name, args, .. } => {
            if builtin_may_trap_or_effect(name) {
                return false;
            }
            args.iter().all(|a| !loop_defs.contains(&a.0))
        }
        Value::ClosureCap { env, .. } => !loop_defs.contains(&env.0),
    }
}
