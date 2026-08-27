//! Shared walks over [`Value`] / [`Block`] local operands.
//!
//! New `Value` arms that carry `Local`s should update [`for_each_local_mut`] so
//! remap / collect / max-local stay exhaustive in one place.

use crate::{Block, Local, Op, Value};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

macro_rules! visit_value_locals {
    ($value:expr, |$l:pat_param| $body:expr) => {
        match $value {
            Value::Local(l) => {
                let $l = l;
                $body
            }
            Value::Binary { left, right, .. } => {
                {
                    let $l = left;
                    $body
                }
                {
                    let $l = right;
                    $body
                }
            }
            Value::Unary { operand, .. } => {
                let $l = operand;
                $body
            }
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
                    let $l = a;
                    $body
                }
            }
            Value::IndirectCall { callee, args } => {
                {
                    let $l = callee;
                    $body
                }
                for a in args {
                    let $l = a;
                    $body
                }
            }
            Value::If { cond, .. } => {
                let $l = cond;
                $body
            }
            Value::Lambda { params, .. } => {
                for p in params {
                    let $l = p;
                    $body
                }
            }
            Value::ClosureCap { env, .. } => {
                let $l = env;
                $body
            }
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
    };
}

/// Visit every `Local` operand stored directly on this `Value` node.
/// Does **not** enter nested [`Block`]s (`If`/`Loop`/`Lambda` bodies).
/// `If.cond`, `Lambda.params`, and `ClosureCap.env` are included.
pub fn for_each_local_mut(value: &mut Value, f: &mut impl FnMut(&mut Local)) {
    visit_value_locals!(value, |l| f(l));
}

/// Immutable counterpart of [`for_each_local_mut`].
pub fn for_each_local(value: &Value, f: &mut impl FnMut(Local)) {
    visit_value_locals!(value, |l| f(*l));
}

macro_rules! visit_nested_regions {
    ($value:expr, |$b:pat_param| $body:expr) => {
        match $value {
            Value::If {
                then_block,
                else_block,
                ..
            } => {
                {
                    let $b = then_block;
                    $body
                }
                {
                    let $b = else_block;
                    $body
                }
            }
            Value::Loop {
                header,
                body,
                latch,
            } => {
                {
                    let $b = header;
                    $body
                }
                {
                    let $b = body;
                    $body
                }
                {
                    let $b = latch;
                    $body
                }
            }
            Value::Lambda { body, .. } => {
                let $b = body;
                $body
            }
            _ => {}
        }
    };
}

macro_rules! visit_nested_regions_mut {
    ($value:expr, |$b:pat_param| $body:expr) => {
        match $value {
            Value::If {
                then_block,
                else_block,
                ..
            } => {
                {
                    let $b = then_block.as_mut();
                    $body
                }
                {
                    let $b = else_block.as_mut();
                    $body
                }
            }
            Value::Loop {
                header,
                body,
                latch,
            } => {
                {
                    let $b = header.as_mut();
                    $body
                }
                {
                    let $b = body.as_mut();
                    $body
                }
                {
                    let $b = latch.as_mut();
                    $body
                }
            }
            Value::Lambda { body, .. } => {
                let $b = body.as_mut();
                $body
            }
            _ => {}
        }
    };
}

/// Remap locals on this node, then recurse into nested blocks via `on_block`.
pub fn map_value_locals(
    value: &mut Value,
    map_l: &mut impl FnMut(&mut Local),
    on_block: &mut impl FnMut(&mut Block),
) {
    for_each_local_mut(value, map_l);
    visit_nested_regions_mut!(value, |b| on_block(b));
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
    visit_nested_regions!(value, |b| {
        max = max.max(crate::max_local_in_block(b));
    });
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
    visit_nested_regions!(value, |b| collect_uses(b, locals, names));
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
    visit_nested_regions_mut!(value, |b| f(b));
}

pub fn for_each_nested_block(value: &Value, f: &mut impl FnMut(&Block)) {
    visit_nested_regions!(value, |b| f(b));
}

/// Visit every `Let` value in `body`, recursing into If/Loop bodies.
pub fn for_each_let(body: &Block, f: &mut dyn FnMut(&Value)) {
    for op in &body.ops {
        if let Op::Let { value, .. } = op {
            f(value);
            for_each_nested_block(value, &mut |nested| for_each_let(nested, f));
        }
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

/// SSA locals defined by simple/computable `Let` RHS (constants, names, small builtins).
/// Used by SR pattern matchers and NSW IV analysis.
pub fn collect_leaf_defs(body: &Block) -> HashMap<u32, Value> {
    let mut all_defs = HashMap::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Let { local, value, .. } = op {
                if is_leaf_def_value(value) {
                    all_defs.insert(local.0, value.clone());
                }
            }
        }
    });
    all_defs
}

fn is_leaf_def_value(value: &Value) -> bool {
    matches!(
        value,
        Value::Int(_)
            | Value::Float(_)
            | Value::Name(_)
            | Value::Binary { .. }
            | Value::Builtin { .. }
            | Value::AllocList { .. }
    )
}

/// Collect every `(header, body, latch)` from nested `Loop` values (SR test/match helper).
pub fn collect_loop_triples(block: &Block, out: &mut Vec<(Block, Block, Block)>) {
    for op in &block.ops {
        if let Op::Let {
            value:
                Value::Loop {
                    header,
                    body,
                    latch,
                },
            ..
        } = op
        {
            out.push((
                header.as_ref().clone(),
                body.as_ref().clone(),
                latch.as_ref().clone(),
            ));
            collect_loop_triples(body, out);
            collect_loop_triples(header, out);
            collect_loop_triples(latch, out);
        }
        if let Op::Let {
            value:
                Value::If {
                    then_block,
                    else_block,
                    ..
                },
            ..
        } = op
        {
            collect_loop_triples(then_block, out);
            collect_loop_triples(else_block, out);
        }
    }
}

/// Total SSA op count in `block` and nested If/Loop/Lambda bodies.
pub fn count_ops(block: &Block) -> usize {
    let mut n = 0;
    for_each_block_dfs(block, &mut |b| n += b.ops.len());
    n
}

/// Whether any nested region contains a direct `Call` to `fun` (enters Lambda).
pub fn block_calls(block: &Block, fun: &str) -> bool {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value } if value_calls(value, fun) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn value_calls(value: &Value, fun: &str) -> bool {
    match value {
        Value::Call { fun: f, .. } if f == fun => true,
        Value::If {
            then_block,
            else_block,
            ..
        } => block_calls(then_block, fun) || block_calls(else_block, fun),
        Value::Loop {
            header,
            body,
            latch,
        } => block_calls(header, fun) || block_calls(body, fun) || block_calls(latch, fun),
        Value::Lambda { body, .. } => block_calls(body, fun),
        _ => false,
    }
}

/// Whether `block` or a nested region contains `Op::Return`.
pub fn has_early_return(block: &Block) -> bool {
    let mut found = false;
    for_each_block_dfs(block, &mut |b| {
        if !found && b.ops.iter().any(|op| matches!(op, Op::Return { .. })) {
            found = true;
        }
    });
    found
}

/// Eager IO in `block` (not deferred nested-lambda bodies).
///
/// Used to mark lifted `__lam_*` [`crate::ir::CoreFun::effect`] so opt passes that
/// trust `effect.is_pure()` (inline / CSE / const-specialize) do not treat IO
/// thunks as pure. `io_callees` are known effectful top-level names.
pub fn block_has_io(block: &Block, io_callees: &HashSet<String>) -> bool {
    for op in &block.ops {
        match op {
            Op::Effect { .. } => return true,
            Op::Let {
                pure_region: false, ..
            } => return true,
            Op::Let { value, .. } => {
                if value_has_eager_io(value, io_callees) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn value_has_eager_io(value: &Value, io_callees: &HashSet<String>) -> bool {
    match value {
        Value::Call { fun, .. } if io_callees.contains(fun) => true,
        Value::Builtin { name, .. } if name.is_io() => true,
        // Indirect call may invoke an IO Fun; opt must not treat it as pure.
        Value::IndirectCall { .. } => true,
        // Constructing a nested IO thunk is pure; that body is analyzed when lifted.
        Value::Lambda { .. } => false,
        v => {
            let mut found = false;
            visit_nested_regions!(v, |b| {
                if !found && block_has_io(b, io_callees) {
                    found = true;
                }
            });
            found
        }
    }
}

/// Whether `block` or a nested region has `Op::Assign` or a `Value::Name` load.
pub fn has_assign_or_name(block: &Block) -> bool {
    for op in &block.ops {
        match op {
            Op::Assign { .. } => return true,
            Op::Let { value, .. } | Op::Effect { value } if value_has_assign_or_name(value) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn value_has_assign_or_name(value: &Value) -> bool {
    if matches!(value, Value::Name(_)) {
        return true;
    }
    let mut found = false;
    visit_nested_regions!(value, |b| {
        if !found && has_assign_or_name(b) {
            found = true;
        }
    });
    found
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

    #[test]
    fn count_ops_includes_nested_if() {
        let block = Block {
            params: vec![],
            ops: vec![Op::Let {
                local: Local(0),
                value: Value::If {
                    cond: Local(1),
                    then_block: Box::new(Block {
                        params: vec![],
                        ops: vec![Op::Let {
                            local: Local(2),
                            value: Value::Int(1),
                            pure_region: true,
                        }],
                        result: Some(Local(2)),
                    }),
                    else_block: Box::new(Block {
                        params: vec![],
                        ops: vec![],
                        result: Some(Local(3)),
                    }),
                },
                pure_region: true,
            }],
            result: Some(Local(0)),
        };
        assert_eq!(count_ops(&block), 2);
        assert!(has_early_return(&Block {
            params: vec![],
            ops: vec![Op::Return { value: Local(0) }],
            result: None,
        }));
        assert!(block_calls(
            &Block {
                params: vec![],
                ops: vec![Op::Let {
                    local: Local(0),
                    value: Value::Call {
                        fun: "f".into(),
                        args: vec![],
                    },
                    pure_region: true,
                }],
                result: Some(Local(0)),
            },
            "f"
        ));
        assert!(has_assign_or_name(&Block {
            params: vec![],
            ops: vec![Op::Assign {
                name: "x".into(),
                value: Local(0),
            }],
            result: None,
        }));
    }
}
