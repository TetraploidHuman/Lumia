//! Fixed-point propagation of escaping locals through aliases and containers.

use lumia_core::{Block, Local, Op, Value};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) fn propagate_block(
    block: &Block,
    escaping: &mut HashSet<Local>,
    assigns: &HashMap<String, Vec<Local>>,
) -> bool {
    let mut changed = false;
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                changed |= propagate_let(*local, value, escaping, assigns);
            }
            Op::Effect { value } => {
                changed |= propagate_value_only(value, escaping, assigns);
            }
            Op::Assign { name, value } => {
                // If this named slot already has an escaping write, new RHS escapes.
                if assigns
                    .get(name)
                    .is_some_and(|ls| ls.iter().any(|l| escaping.contains(l)))
                    && escaping.insert(*value)
                {
                    changed = true;
                }
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
    changed
}

fn propagate_let(
    local: Local,
    value: &Value,
    escaping: &mut HashSet<Local>,
    assigns: &HashMap<String, Vec<Local>>,
) -> bool {
    let mut changed = false;
    if escaping.contains(&local) {
        changed |= mark_inputs_escaping(value, escaping, assigns);
    }
    changed |= propagate_value_only(value, escaping, assigns);
    changed
}

fn propagate_value_only(
    value: &Value,
    escaping: &mut HashSet<Local>,
    assigns: &HashMap<String, Vec<Local>>,
) -> bool {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            let mut c = propagate_block(then_block, escaping, assigns);
            c |= propagate_block(else_block, escaping, assigns);
            c
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            let mut c = propagate_block(header, escaping, assigns);
            c |= propagate_block(body, escaping, assigns);
            c |= propagate_block(latch, escaping, assigns);
            c
        }
        Value::Lambda { body, .. } => propagate_block(body, escaping, assigns),
        _ => false,
    }
}

fn mark_inputs_escaping(
    value: &Value,
    escaping: &mut HashSet<Local>,
    assigns: &HashMap<String, Vec<Local>>,
) -> bool {
    let mut changed = false;
    let mut mark = |l: Local| {
        if escaping.insert(l) {
            changed = true;
        }
    };
    match value {
        Value::Local(l) => mark(*l),
        Value::Name(n) => {
            if let Some(ls) = assigns.get(n) {
                for l in ls {
                    mark(*l);
                }
            }
        }
        Value::Binary { left, right, .. } => {
            mark(*left);
            mark(*right);
        }
        Value::Unary { operand, .. } => mark(*operand),
        Value::Builtin { name, args } => {
            if name.may_capture() {
                for a in args {
                    mark(*a);
                }
            }
        }
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
        Value::Int(_)
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
