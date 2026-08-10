//! Float ABI inference for lifted lambdas.

use crate::ir::{Block, Local, Op, Value};
use rustc_hash::FxHashSet as HashSet;

/// Infer which lambda parameters are used in float contexts.
pub(super) fn params_used_as_float(block: &Block, params: &[Local]) -> HashSet<u32> {
    let param_set: HashSet<u32> = params.iter().map(|p| p.0).collect();
    let mut float_locals: HashSet<u32> = HashSet::default();
    let mut used: HashSet<u32> = HashSet::default();
    mark_float_uses(block, &param_set, &mut float_locals, &mut used);
    used
}

fn mark_float_uses(
    block: &Block,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                mark_float_in_value(value, params, float_locals, used);
                if value_is_float_producing(value, float_locals) {
                    float_locals.insert(local.0);
                }
            }
            Op::Effect { value } => mark_float_in_value(value, params, float_locals, used),
            _ => {}
        }
    }
}

fn mark_float_in_value(
    v: &Value,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
) {
    match v {
        Value::Binary { left, right, .. } => {
            let lf = float_locals.contains(&left.0);
            let rf = float_locals.contains(&right.0);
            if lf || rf {
                touch_param(left.0, params, used);
                touch_param(right.0, params, used);
            }
        }
        Value::Unary { operand, .. } => {
            if float_locals.contains(&operand.0) {
                touch_param(operand.0, params, used);
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            mark_float_uses(then_block, params, float_locals, used);
            mark_float_uses(else_block, params, float_locals, used);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            mark_float_uses(header, params, float_locals, used);
            mark_float_uses(body, params, float_locals, used);
            mark_float_uses(latch, params, float_locals, used);
        }
        _ => {}
    }
}

fn touch_param(id: u32, params: &HashSet<u32>, used: &mut HashSet<u32>) {
    if params.contains(&id) {
        used.insert(id);
    }
}

pub(super) fn value_is_float_producing(v: &Value, float_locals: &HashSet<u32>) -> bool {
    match v {
        Value::Float(_) => true,
        Value::Local(Local(id)) => float_locals.contains(id),
        Value::ClosureCap { as_float: true, .. } => true,
        Value::Binary { left, right, .. } => {
            float_locals.contains(&left.0) || float_locals.contains(&right.0)
        }
        Value::Unary { operand, .. } => float_locals.contains(&operand.0),
        _ => false,
    }
}

pub(super) fn block_result_is_float(block: &Block) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let float_locals = compute_float_locals_in_block(block);
    float_locals.contains(&r)
}

/// Locals that hold Float values in `block` (for closure-capture ABI).
pub(super) fn compute_float_locals_in_block(block: &Block) -> HashSet<u32> {
    let mut float_locals: HashSet<u32> = HashSet::default();
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if value_is_float_producing(value, &float_locals) || matches!(value, Value::Float(_)) {
                float_locals.insert(local.0);
            }
            if let Value::Binary { left, right, .. } = value {
                if float_locals.contains(&left.0) || float_locals.contains(&right.0) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::Local(Local(src)) = value {
                if float_locals.contains(src) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::ClosureCap { as_float: true, .. } = value {
                float_locals.insert(local.0);
            }
            if let Value::Unary { operand, .. } = value {
                if float_locals.contains(&operand.0) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::If {
                then_block,
                else_block,
                ..
            } = value
            {
                float_locals.extend(compute_float_locals_in_block(then_block));
                float_locals.extend(compute_float_locals_in_block(else_block));
            }
        }
    }
    float_locals
}
