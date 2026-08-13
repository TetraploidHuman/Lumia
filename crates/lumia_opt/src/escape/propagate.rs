//! Fixed-point propagation of escaping locals through aliases and containers.

use lumia_core::{Block, Local, MapRepr, Op, Value};
use lumia_hir::Builtin;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Must match [`crate::repr_select`] heap thresholds: large / always-heap
/// containers store field pointers on the GC heap even when the container
/// local itself does not "escape". Stack `Lit*` fields in those slots are
/// invisible to the collector (and dangle after the frame returns).
fn alloc_forces_heap(value: &Value) -> bool {
    match value {
        Value::AllocAdt { fields, .. } => fields.len() > 8,
        Value::AllocList { elems, .. } => elems.len() > 8,
        Value::AllocSet { elems, .. } => elems.is_empty() || elems.len() > 8,
        Value::AllocMap { flat_pairs, repr, .. } => {
            let n = flat_pairs.len() / 2;
            matches!(repr, MapRepr::AssocList) || n == 0 || n > 8
        }
        Value::AllocClosure { .. } => true,
        _ => false,
    }
}

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
    // Escaping container **or** non-escaping Heap* (size / always-heap): fields
    // must not stay stack Lit* — GC cannot trace stack payloads via heap edges.
    if escaping.contains(&local) || alloc_forces_heap(value) {
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
            } else if matches!(
                *name,
                Builtin::ListGet | Builtin::AdtField | Builtin::ListTake | Builtin::ListSlice
            ) {
                // Escaping get/field/take/slice result ⇒ container escapes
                // (take/slice copy element pointers into a fresh list).
                if let Some(c) = args.first() {
                    mark(*c);
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
