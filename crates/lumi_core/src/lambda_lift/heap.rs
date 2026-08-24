//! Heap reachability for lifted-lambda ABI (`ret_ty` rooting).

use crate::ir::{Block, Local, Op, Value};
use crate::value_ty::{value_alloc_may_heap, HeapPolicy};
use lumi_hir::Builtin;
use rustc_hash::FxHashSet as HashSet;

/// Whether the block result may be a heap pointer. `extra_params` covers lambda
/// formals that live on `Value::Lambda.params` rather than `body.params`.
pub(super) fn block_result_may_heap_with_params(block: &Block, extra_params: &[Local]) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut params: HashSet<u32> = block.params.iter().map(|p| p.0).collect();
    params.extend(extra_params.iter().map(|p| p.0));
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
        Value::Builtin { name, .. } => !matches!(
            name,
            Builtin::ListLen | Builtin::Contains | Builtin::Println | Builtin::Assert
        ),
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
    let mut params = inherited.clone();
    params.extend(block.params.iter().map(|p| p.0));
    local_may_heap(block, r, &params, &mut HashSet::default())
}
