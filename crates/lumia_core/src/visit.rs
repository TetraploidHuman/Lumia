//! Shared walks over [`Value`] / [`Block`] local operands.
//!
//! New `Value` arms that carry `Local`s should update [`for_each_local_mut`] so
//! remap / collect / max-local stay exhaustive in one place.

use crate::{Block, Local, Op, Value};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Visit every `Local` operand stored directly on this `Value` node.
/// Does **not** enter nested [`Block`]s (`If`/`Loop`/`Lambda` bodies).
/// `If.cond`, `Lambda.params`, and `ClosureCap.env` are included.
pub fn for_each_local_mut(value: &mut Value, f: &mut impl FnMut(&mut Local)) {
    match value {
        Value::Local(l) => f(l),
        Value::Binary { left, right, .. } => {
            f(left);
            f(right);
        }
        Value::Unary { operand, .. } => f(operand),
        Value::Call { args, .. }
        | Value::Builtin { args, .. }
        | Value::AllocList { elems: args, .. }
        | Value::AllocSet { elems: args, .. }
        | Value::AllocMap {
            flat_pairs: args, ..
        }
        | Value::AllocAdt { fields: args, .. }
        | Value::AllocClosure { captures: args, .. } => {
            for a in args {
                f(a);
            }
        }
        Value::IndirectCall { callee, args } => {
            f(callee);
            for a in args {
                f(a);
            }
        }
        Value::If { cond, .. } => f(cond),
        Value::Lambda { params, .. } => {
            for p in params {
                f(p);
            }
        }
        Value::ClosureCap { env, .. } => f(env),
        Value::Loop { .. }
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::Name(_)
        | Value::FunRef(_) => {}
    }
}

/// Immutable counterpart of [`for_each_local_mut`].
pub fn for_each_local(value: &Value, f: &mut impl FnMut(Local)) {
    match value {
        Value::Local(l) => f(*l),
        Value::Binary { left, right, .. } => {
            f(*left);
            f(*right);
        }
        Value::Unary { operand, .. } => f(*operand),
        Value::Call { args, .. }
        | Value::Builtin { args, .. }
        | Value::AllocList { elems: args, .. }
        | Value::AllocSet { elems: args, .. }
        | Value::AllocMap {
            flat_pairs: args, ..
        }
        | Value::AllocAdt { fields: args, .. }
        | Value::AllocClosure { captures: args, .. } => {
            for a in args {
                f(*a);
            }
        }
        Value::IndirectCall { callee, args } => {
            f(*callee);
            for a in args {
                f(*a);
            }
        }
        Value::If { cond, .. } => f(*cond),
        Value::Lambda { params, .. } => {
            for p in params {
                f(*p);
            }
        }
        Value::ClosureCap { env, .. } => f(*env),
        Value::Loop { .. }
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::Name(_)
        | Value::FunRef(_) => {}
    }
}

/// Remap locals on this node, then recurse into nested blocks via `on_block`.
pub fn map_value_locals(
    value: &mut Value,
    map_l: &mut impl FnMut(&mut Local),
    on_block: &mut impl FnMut(&mut Block),
) {
    for_each_local_mut(value, map_l);
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            on_block(then_block);
            on_block(else_block);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            on_block(header);
            on_block(body);
            on_block(latch);
        }
        Value::Lambda { body, .. } => on_block(body),
        _ => {}
    }
}

/// Apply `remap` to every local in `value` and nested blocks (via [`rewrite_block_locals`]).
pub fn rewrite_value_locals(value: &mut Value, remap: &HashMap<u32, u32>) {
    if remap.is_empty() {
        return;
    }
    map_value_locals(
        value,
        &mut |l| {
            if let Some(&r) = remap.get(&l.0) {
                *l = Local(r);
            }
        },
        &mut |b| crate::rewrite_block_locals(b, remap),
    );
}

/// Max local id appearing in this value (including nested blocks).
pub fn max_local_in_value(value: &Value) -> u32 {
    let mut max = 0u32;
    for_each_local(value, &mut |l| max = max.max(l.0));
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            max = max
                .max(crate::max_local_in_block(then_block))
                .max(crate::max_local_in_block(else_block));
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            max = max
                .max(crate::max_local_in_block(header))
                .max(crate::max_local_in_block(body))
                .max(crate::max_local_in_block(latch));
        }
        Value::Lambda { body, .. } => {
            max = max.max(crate::max_local_in_block(body));
        }
        _ => {}
    }
    max
}

/// Collect SSA uses (and `Name` loads) from a value, including nested blocks.
pub fn collect_uses_in_value(
    value: &Value,
    locals: &mut HashSet<u32>,
    names: &mut HashSet<String>,
) {
    for_each_local(value, &mut |l| {
        locals.insert(l.0);
    });
    if let Value::Name(n) = value {
        names.insert(n.clone());
    }
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_uses(then_block, locals, names);
            collect_uses(else_block, locals, names);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_uses(header, locals, names);
            collect_uses(body, locals, names);
            collect_uses(latch, locals, names);
        }
        Value::Lambda { body, .. } => {
            collect_uses(body, locals, names);
        }
        _ => {}
    }
}

/// Collect SSA uses (and `Name` loads) across a whole block, including nested regions.
pub(crate) fn collect_uses(block: &Block, locals: &mut HashSet<u32>, names: &mut HashSet<String>) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                collect_uses_in_value(value, locals, names);
            }
            Op::Assign { value, .. } | Op::Return { value } => {
                locals.insert(value.0);
            }
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = &block.result {
        locals.insert(r.0);
    }
}

/// Walk nested region blocks only (If/Loop/Lambda), for defined-local collection etc.
pub fn for_each_nested_block_mut(value: &mut Value, f: &mut impl FnMut(&mut Block)) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            f(then_block);
            f(else_block);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            f(header);
            f(body);
            f(latch);
        }
        Value::Lambda { body, .. } => f(body),
        _ => {}
    }
}

pub fn for_each_nested_block(value: &Value, f: &mut impl FnMut(&Block)) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            f(then_block);
            f(else_block);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            f(header);
            f(body);
            f(latch);
        }
        Value::Lambda { body, .. } => f(body),
        _ => {}
    }
}

/// Depth-first over a block and every nested If/Loop/Lambda body reached from its ops.
pub fn for_each_block_dfs(block: &Block, f: &mut impl FnMut(&Block)) {
    f(block);
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value } => {
                for_each_nested_block(value, &mut |nested| for_each_block_dfs(nested, f));
            }
            _ => {}
        }
    }
}

/// Mutating walk: for each `Let`/`Effect` value, call `on_value` then recurse into nested blocks.
///
/// `on_value` should transform the current value leaf only — nested regions are visited
/// automatically via [`for_each_nested_block_mut`].
pub fn for_each_op_value_mut(block: &mut Block, on_value: &mut dyn FnMut(&mut Value)) {
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value } => {
                on_value(value);
                for_each_nested_block_mut(value, &mut |nested| {
                    for_each_op_value_mut(nested, on_value);
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ListRepr, Op};

    #[test]
    fn rewrite_remaps_call_args_and_if_blocks() {
        let mut v = Value::If {
            cond: Local(1),
            then_block: Box::new(Block {
                params: vec![],
                ops: vec![Op::Let {
                    local: Local(2),
                    value: Value::Call {
                        fun: "f".into(),
                        args: vec![Local(3)],
                    },
                    pure_region: true,
                }],
                result: Some(Local(2)),
            }),
            else_block: Box::new(Block {
                params: vec![],
                ops: vec![],
                result: Some(Local(4)),
            }),
        };
        let mut remap = HashMap::default();
        remap.insert(1, 10);
        remap.insert(3, 30);
        remap.insert(4, 40);
        rewrite_value_locals(&mut v, &remap);
        match &v {
            Value::If {
                cond,
                then_block,
                else_block,
            } => {
                assert_eq!(cond.0, 10);
                match &then_block.ops[0] {
                    Op::Let {
                        value: Value::Call { args, .. },
                        ..
                    } => assert_eq!(args[0].0, 30),
                    _ => panic!("expected call"),
                }
                assert_eq!(else_block.result.unwrap().0, 40);
            }
            _ => panic!("expected if"),
        }
        let _ = ListRepr::HeapList; // keep repr import path warm if needed
    }

    #[test]
    fn max_local_sees_nested() {
        let v = Value::AllocList {
            elems: vec![Local(2), Local(7)],
            repr: ListRepr::HeapList,
        };
        assert_eq!(max_local_in_value(&v), 7);
    }

    #[test]
    fn return_operand_is_use_and_remapped() {
        let mut block = Block {
            params: vec![],
            ops: vec![Op::Return { value: Local(3) }],
            result: None,
        };
        let mut locals = HashSet::default();
        let mut names = HashSet::default();
        collect_uses(&block, &mut locals, &mut names);
        assert!(locals.contains(&3));

        let mut remap = HashMap::default();
        remap.insert(3, 9);
        crate::rewrite_block_locals(&mut block, &remap);
        match &block.ops[0] {
            Op::Return { value } => assert_eq!(value.0, 9),
            _ => panic!("expected return"),
        }
        assert_eq!(crate::max_local_in_block(&block), 9);
    }
}
