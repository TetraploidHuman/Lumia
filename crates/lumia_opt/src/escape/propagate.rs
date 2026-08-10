//! Fixed-point propagation of escaping locals through aliases and containers.

use lumia_core::{Block, Local, Op, Value};
use rustc_hash::FxHashSet as HashSet;

pub(super) fn propagate_block(block: &Block, escaping: &mut HashSet<Local>) -> bool {
    let mut changed = false;
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                changed |= propagate_let(*local, value, escaping);
            }
            Op::Effect { value } => {
                changed |= propagate_value_only(value, escaping);
            }
            Op::Assign { .. } | Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
    changed
}

fn propagate_let(local: Local, value: &Value, escaping: &mut HashSet<Local>) -> bool {
    let mut changed = false;
    // If the binding escapes, everything it aliases / contains escapes.
    if escaping.contains(&local) {
        changed |= mark_inputs_escaping(value, escaping);
    }
    changed |= propagate_value_only(value, escaping);
    changed
}

fn propagate_value_only(value: &Value, escaping: &mut HashSet<Local>) -> bool {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            let mut c = propagate_block(then_block, escaping);
            c |= propagate_block(else_block, escaping);
            c
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            let mut c = propagate_block(header, escaping);
            c |= propagate_block(body, escaping);
            c |= propagate_block(latch, escaping);
            c
        }
        Value::Lambda { body, .. } => propagate_block(body, escaping),
        _ => false,
    }
}

fn mark_inputs_escaping(value: &Value, escaping: &mut HashSet<Local>) -> bool {
    let mut changed = false;
    let mut mark = |l: Local| {
        if escaping.insert(l) {
            changed = true;
        }
    };
    match value {
        Value::Local(l) => mark(*l),
        Value::Binary { left, right, .. } => {
            mark(*left);
            mark(*right);
        }
        Value::Unary { operand, .. } => mark(*operand),
        Value::Builtin { name, args } => {
            // Pure projections do not retain the collection; returning `xs.len()`
            // must not mark `xs` itself as escaping.
            if name.may_capture() {
                for a in args {
                    mark(*a);
                }
            }
        }
        // `Call` args are seeded from callee param-escape summaries only.
        // A escaping Call *result* does not imply args escape (unless a formal
        // aliases the return — already reflected in the summary).
        Value::AllocList { elems: args, .. }
        | Value::AllocSet { elems: args, .. }
        | Value::AllocMap {
            flat_pairs: args, ..
        }
        | Value::AllocAdt { fields: args, .. }
        | Value::AllocClosure { captures: args, .. } => {
            for a in args {
                mark(*a);
            }
        }
        Value::Call { .. } => {}
        Value::IndirectCall { callee, args } => {
            mark(*callee);
            for a in args {
                mark(*a);
            }
        }
        Value::ClosureCap { env, .. } => mark(*env),
        Value::If { cond, .. } => mark(*cond),
        Value::Name(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::FunRef(_)
        | Value::Loop { .. }
        | Value::Lambda { .. } => {}
    }
    changed
}
