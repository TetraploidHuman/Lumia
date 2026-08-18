//! Heap reachability for lifted-lambda ABI (`ret_ty` rooting).
//!
//! Uses shared [`value_alloc_may_heap`] / [`builtin_result_may_heap`] /
//! [`type_may_heap`] so Typed builtins follow the same lattice as codegen roots.
//! Unstamped `ChannelRecv` / `TaskJoin` stay non-heap (scalar-common; channel/task
//! fixup still refines when HIR did not stamp a ground payload).

use crate::ir::{Block, Local, Op, Value};
use crate::value_ty::{builtin_result_may_heap, value_alloc_may_heap, HeapPolicy};
use rustc_hash::FxHashSet as HashSet;

/// Whether the block result may be a heap pointer. `extra_params` covers lambda
/// formals on `Value::Lambda.params` / `CoreFun.params` (blocks have no params).
pub(super) fn block_result_may_heap_with_params(block: &Block, extra_params: &[Local]) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let params: HashSet<u32> = extra_params.iter().map(|p| p.0).collect();
    local_may_heap(block, r, &params, &mut HashSet::default())
}

/// Follow `let x = y` aliases. Params are treated as maybe-heap so identity
/// lambdas like `{ s -> s }` keep a heap `ret_ty` for GC rooting at call sites.
fn local_may_heap(block: &Block, id: u32, params: &HashSet<u32>, seen: &mut HashSet<u32>) -> bool {
    if !seen.insert(id) {
        return true;
    }
    if params.contains(&id) {
        return true;
    }
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if local.0 == id {
                return value_may_heap(block, value, params, seen);
            }
        }
    }
    false
}

fn value_may_heap(
    block: &Block,
    v: &Value,
    params: &HashSet<u32>,
    seen: &mut HashSet<u32>,
) -> bool {
    if value_alloc_may_heap(v, HeapPolicy::Conservative) {
        return true;
    }
    match v {
        Value::Local(Local(id)) => local_may_heap(block, *id, params, seen),
        Value::Builtin {
            name, result_ty, ..
        } => builtin_result_may_heap(*name, result_ty.as_ref(), || None),
        Value::Call { .. } | Value::IndirectCall { .. } => true,
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            // Nested blocks inherit lambda/outer params for alias tracking.
            result_may_heap_inherited(then_block, params)
                || result_may_heap_inherited(else_block, params)
        }
        _ => false,
    }
}

fn result_may_heap_inherited(block: &Block, inherited: &HashSet<u32>) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    local_may_heap(block, r, inherited, &mut HashSet::default())
}

#[cfg(test)]
#[path = "heap_tests.rs"]
mod tests;
