//! Fixed-point propagation of escaping locals through aliases and containers.

use lumia_abi::SMALL_CONTAINER_MAX;
use lumia_core::{for_each_op_in_block, Block, Local, MapRepr, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::Sym;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Must match [`crate::repr_select`] and RT `MAP_SMALL_MAX` / `SET_SMALL_MAX`
/// ([`lumia_abi::SMALL_CONTAINER_MAX`]): large / always-heap containers store
/// field pointers on the GC heap even when the container local itself does not
/// "escape". Stack `Lit*` fields in those slots are invisible to the collector
/// (and dangle after the frame returns).
fn alloc_forces_heap(value: &Value) -> bool {
    let max = SMALL_CONTAINER_MAX;
    match value {
        Value::AllocAdt { fields, .. } => fields.len() > max,
        Value::AllocList { elems, .. } => elems.len() > max,
        Value::AllocSet { elems, .. } => elems.is_empty() || elems.len() > max,
        Value::AllocMap {
            flat_pairs, repr, ..
        } => {
            let n = flat_pairs.len() / 2;
            matches!(repr, MapRepr::AssocList) || n == 0 || n > max
        }
        Value::AllocClosure { .. } => true,
        _ => false,
    }
}

pub(super) fn propagate_block(
    block: &Block,
    escaping: &mut HashSet<Local>,
    assigns: &HashMap<Sym, Vec<Local>>,
) -> bool {
    let mut changed = false;
    for_each_op_in_block(block, &mut |op| {
        match op {
            Op::Let { local, value, .. } => {
                if propagate_let(*local, value, escaping, assigns) {
                    changed = true;
                }
            }
            Op::Assign { name, value } => {
                // If this named slot already has an escaping write, new RHS escapes.
                if assigns
                    .get(name.as_str())
                    .is_some_and(|ls| ls.iter().any(|l| escaping.contains(l)))
                    && escaping.insert(*value)
                {
                    changed = true;
                }
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    });
    changed
}

fn propagate_let(
    local: Local,
    value: &Value,
    escaping: &mut HashSet<Local>,
    assigns: &HashMap<Sym, Vec<Local>>,
) -> bool {
    // Escaping container **or** non-escaping Heap* (size / always-heap): fields
    // must not stay stack Lit* — GC cannot trace stack payloads via heap edges.
    if escaping.contains(&local) || alloc_forces_heap(value) {
        mark_inputs_escaping(value, escaping, assigns)
    } else {
        false
    }
}

fn mark_inputs_escaping(
    value: &Value,
    escaping: &mut HashSet<Local>,
    assigns: &HashMap<Sym, Vec<Local>>,
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
            if let Some(ls) = assigns.get(n.as_str()) {
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
        Value::Builtin { name, args, .. } => {
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
