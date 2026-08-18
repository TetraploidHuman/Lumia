//! Shared walks over [`Value`] / [`Block`] local operands.
//!
//! New `Value` arms that carry `Local`s should update [`for_each_local_mut`] so
//! remap / collect / max-local stay exhaustive in one place.

use crate::ir::{Block, CoreFun, CoreModule, Local, Op, Value};
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
        &mut |b| rewrite_block_locals(b, remap),
    );
}

/// Remap every `Local` in `block` (ops, result, and nested values).
pub fn rewrite_block_locals(block: &mut Block, remap: &HashMap<u32, u32>) {
    if remap.is_empty() {
        return;
    }
    let map_l = |l: &mut Local| {
        if let Some(&r) = remap.get(&l.0) {
            *l = Local(r);
        }
    };
    if let Some(r) = &mut block.result {
        map_l(r);
    }
    for op in &mut block.ops {
        match op {
            Op::Let { local, value, .. } => {
                map_l(local);
                rewrite_value_locals(value, remap);
            }
            Op::Assign { value, .. } | Op::Return { value } => map_l(value),
            Op::Break | Op::Continue => {}
        }
    }
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
                .max(max_local_in_block(then_block))
                .max(max_local_in_block(else_block));
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            max = max
                .max(max_local_in_block(header))
                .max(max_local_in_block(body))
                .max(max_local_in_block(latch));
        }
        Value::Lambda { body, .. } => {
            max = max.max(max_local_in_block(body));
        }
        _ => {}
    }
    max
}

/// Highest `Local` id used in a block (ops + nested values + result).
pub fn max_local_in_block(block: &Block) -> u32 {
    let mut max = 0u32;
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                max = max.max(local.0);
                max = max.max(max_local_in_value(value));
            }
            Op::Assign { value, .. } | Op::Return { value } => max = max.max(value.0),
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = &block.result {
        max = max.max(r.0);
    }
    max
}

/// Highest `Local` id used in a function (params + body).
pub fn max_local_in_fun(fun: &CoreFun) -> u32 {
    let mut max = 0u32;
    for p in &fun.params {
        max = max.max(p.0);
    }
    max.max(max_local_in_block(&fun.body))
}

pub(crate) fn max_local_in_module(module: &CoreModule) -> u32 {
    let mut max = 0u32;
    for fun in &module.functions {
        max = max.max(max_local_in_fun(fun));
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
            Op::Let { value, .. } => {
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

/// Nested If/Loop regions only — skips Lambda (CSE/fold/effect must not enter closures).
pub fn for_each_ctrl_nested_block_mut(value: &mut Value, f: &mut impl FnMut(&mut Block)) {
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

/// First direct `Value::Loop` in `block.ops` (no nested DFS).
///
/// Used by nest SR matchers that only want the immediate inner loop body.
pub fn first_direct_loop(block: &Block) -> Option<(&Block, &Block, &Block)> {
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
            return Some((header, body, latch));
        }
    }
    None
}

/// Collect every `Value::Loop` as cloned `(header, body, latch)`, DFS into nested regions.
///
/// Shared by codegen `*_sr` match tests (and any pass that needs loop shapes) so
/// hand-rolled `find_loops` walkers do not diverge on If/Lambda nesting.
pub fn collect_loops(block: &Block, out: &mut Vec<(Block, Block, Block)>) {
    for op in &block.ops {
        let Op::Let { value, .. } = op else {
            continue;
        };
        if let Value::Loop {
            header,
            body,
            latch,
        } = value
        {
            out.push((
                header.as_ref().clone(),
                body.as_ref().clone(),
                latch.as_ref().clone(),
            ));
        }
        for_each_nested_block(value, &mut |nested| collect_loops(nested, out));
    }
}

/// Depth-first over a block and every nested If/Loop/Lambda body reached from its ops.
pub fn for_each_block_dfs(block: &Block, f: &mut impl FnMut(&Block)) {
    f(block);
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                for_each_nested_block(value, &mut |nested| for_each_block_dfs(nested, f));
            }
            _ => {}
        }
    }
}

/// Collect SSA locals defined by `Let` (and nested `Lambda` params) under `block`.
///
/// Order-independent set inserts — DFS is safe. Shared by inline / captures / LICM.
pub fn collect_defined_locals(block: &Block, defined: &mut HashSet<u32>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Let { local, value, .. } = op {
                defined.insert(local.0);
                if let Value::Lambda { params, .. } = value {
                    for p in params {
                        defined.insert(p.0);
                    }
                }
            }
        }
    });
}

/// Collect mutable slot names written by `Assign` under `block` (DFS).
pub fn collect_assigned_names(block: &Block, out: &mut HashSet<String>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Assign { name, .. } = op {
                out.insert(name.clone());
            }
        }
    });
}

/// Collect slot names from `Assign` writes and `Value::Name` loads (DFS).
pub fn collect_slot_names(block: &Block, names: &mut HashSet<String>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            match op {
                Op::Assign { name, .. } => {
                    names.insert(name.clone());
                }
                Op::Let {
                    value: Value::Name(n),
                    ..
                } => {
                    names.insert(n.clone());
                }
                _ => {}
            }
        }
    });
}

/// Visit every `Op::Let` value in this block tree (DFS into nested blocks).
///
/// For **order-independent** set/map inserts only. Do **not** use for Let-ordered
/// analyses (`mark_float`, `collect_closure_cap_funrefs`, `collect_free`, …).
pub fn for_each_let_value(block: &Block, f: &mut impl FnMut(&Block, &Value)) {
    for_each_let(block, &mut |b, _local, value| f(b, value));
}

/// Like [`for_each_let_value`], but also passes the Let destination [`Local`].
pub fn for_each_let(block: &Block, f: &mut impl FnMut(&Block, Local, &Value)) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Let { local, value, .. } = op {
                f(b, *local, value);
            }
        }
    });
}

/// Visit every `Op::Let` value under If/Loop only — **skips Lambda** bodies.
///
/// Matches `dense_f64_sr` nest matching (lambdas are not SR shapes) and any
/// analysis that must not see nested function bodies (`block_has_io` style).
pub fn for_each_let_value_ctrl(block: &Block, f: &mut impl FnMut(&Block, &Value)) {
    for op in &block.ops {
        let Op::Let { value, .. } = op else {
            continue;
        };
        f(block, value);
        match value {
            Value::If {
                then_block,
                else_block,
                ..
            } => {
                for_each_let_value_ctrl(then_block, f);
                for_each_let_value_ctrl(else_block, f);
            }
            Value::Loop {
                header,
                body,
                latch,
            } => {
                for_each_let_value_ctrl(header, f);
                for_each_let_value_ctrl(body, f);
                for_each_let_value_ctrl(latch, f);
            }
            _ => {}
        }
    }
}

/// Whether `local` is defined by an `Op::Let` whose value matches `pred`.
///
/// Order-independent existence check — DFS (enters Lambda). Used when
/// `leaf_defs` omits nested shapes (e.g. trial_div inlined `isPrime` → `If`).
pub fn local_let_matches(
    local: Local,
    block: &Block,
    mut pred: impl FnMut(&Value) -> bool,
) -> bool {
    let mut found = false;
    for_each_block_dfs(block, &mut |b| {
        if found {
            return;
        }
        for op in &b.ops {
            if let Op::Let {
                local: l, value, ..
            } = op
            {
                if *l == local && pred(value) {
                    found = true;
                    return;
                }
            }
        }
    });
    found
}

/// Collect SSA live refs: block results + shallow Let uses + Assign/Return locals.
///
/// Order-independent set inserts — DFS (enters Lambda). Used by opt DCE.
pub fn collect_ssa_live_refs(block: &Block, live: &mut HashSet<u32>) {
    for_each_block_dfs(block, &mut |b| {
        if let Some(r) = b.result {
            live.insert(r.0);
        }
        for op in &b.ops {
            match op {
                Op::Let { value, .. } => {
                    for_each_local(value, &mut |l| {
                        live.insert(l.0);
                    });
                }
                Op::Assign { value, .. } | Op::Return { value } => {
                    live.insert(value.0);
                }
                Op::Break | Op::Continue => {}
            }
        }
    });
}

/// Collect `AllocClosure` sites: lifted fun name → capture locals.
///
/// Order-independent map inserts — safe for DFS (channel_hint / float_cap_fixup).
pub fn collect_alloc_closure_caps(block: &Block, lam_caps: &mut HashMap<String, Vec<Local>>) {
    for_each_let_value(block, &mut |_b, value| {
        if let Value::AllocClosure { fun, captures } = value {
            lam_caps.insert(fun.name.clone(), captures.clone());
        }
    });
}

/// Collect lifted funs whose `AllocClosure` has a non-empty env (DFS-safe).
pub fn collect_alloc_closure_env_funs(block: &Block, out: &mut HashSet<String>) {
    for_each_let_value(block, &mut |_b, value| {
        if let Value::AllocClosure { fun, captures } = value {
            if !captures.is_empty() {
                out.insert(fun.name.clone());
            }
        }
    });
}

/// Collect Call targets that appear in `methods` (DFS-safe; enters Lambda bodies).
pub fn collect_call_names_in(block: &Block, methods: &HashSet<String>, out: &mut HashSet<String>) {
    for_each_let_value(block, &mut |_b, value| {
        if let Value::Call { fun, .. } = value {
            if methods.contains(fun.as_str()) {
                out.insert(fun.name.clone());
            }
        }
    });
}

/// Collect `Assign` name → value locals (order-independent; DFS-safe).
pub fn collect_assigns(block: &Block, assigns: &mut HashMap<String, Vec<Local>>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Assign { name, value } = op {
                assigns.entry(name.clone()).or_default().push(*value);
            }
        }
    });
}

/// Track FunRef / AllocClosure SSA aliases and which capture slots hold FunRefs.
///
/// **Let-ordered** (not DFS): aliasing depends on definition order. Shared by
/// mono `directize` and `float_cap_fixup`.
pub fn collect_closure_cap_funrefs(
    block: &Block,
    funref_locals: &mut HashMap<u32, String>,
    cap_funs: &mut HashMap<String, HashMap<u32, String>>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                match value {
                    Value::FunRef(name) => {
                        funref_locals.insert(local.0, name.name.clone());
                    }
                    Value::AllocClosure { fun, captures } => {
                        funref_locals.insert(local.0, fun.name.clone());
                        let entry = cap_funs.entry(fun.name.clone()).or_default();
                        for (i, cap) in captures.iter().enumerate() {
                            if let Some(n) = funref_locals.get(&cap.0) {
                                entry.insert(i as u32, n.clone());
                            }
                        }
                    }
                    Value::Local(Local(src)) => {
                        if let Some(n) = funref_locals.get(src).cloned() {
                            funref_locals.insert(local.0, n);
                        } else {
                            funref_locals.remove(&local.0);
                        }
                    }
                    _ => {
                        funref_locals.remove(&local.0);
                    }
                }
                for_each_nested_block(value, &mut |b| {
                    collect_closure_cap_funrefs(b, funref_locals, cap_funs);
                });
            }
            _ => {}
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
    let mut found = false;
    for_each_block_dfs(block, &mut |b| {
        if found {
            return;
        }
        for op in &b.ops {
            if let Op::Let {
                value: Value::Call { fun: f, .. },
                ..
            } = op
            {
                if f == fun {
                    found = true;
                    return;
                }
            }
        }
    });
    found
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
        Value::Call { fun, .. } if io_callees.contains(fun.as_str()) => true,
        Value::Builtin { name, .. } if name.is_io() => true,
        // Indirect call may invoke an IO Fun; opt must not treat it as pure.
        Value::IndirectCall { .. } => true,
        Value::If {
            then_block,
            else_block,
            ..
        } => block_has_io(then_block, io_callees) || block_has_io(else_block, io_callees),
        Value::Loop {
            header,
            body,
            latch,
        } => {
            block_has_io(header, io_callees)
                || block_has_io(body, io_callees)
                || block_has_io(latch, io_callees)
        }
        // Constructing a nested IO thunk is pure; that body is analyzed when lifted.
        Value::Lambda { .. } => false,
        _ => false,
    }
}

/// Whether `block` or a nested region has `Op::Assign` or a `Value::Name` load.
pub fn has_assign_or_name(block: &Block) -> bool {
    let mut found = false;
    for_each_block_dfs(block, &mut |b| {
        if found {
            return;
        }
        for op in &b.ops {
            match op {
                Op::Assign { .. } => {
                    found = true;
                    return;
                }
                Op::Let {
                    value: Value::Name(_),
                    ..
                } => {
                    found = true;
                    return;
                }
                _ => {}
            }
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
            Op::Let { value, .. } => {
                on_value(value);
                for_each_nested_block_mut(value, &mut |nested| {
                    for_each_op_value_mut(nested, on_value);
                });
            }
            _ => {}
        }
    }
}

/// Fill [`CallTarget::id`] on every direct `Call` / `FunRef` / `AllocClosure`
/// from current [`CoreModule::functions`] names.
///
/// Call after passes that rename/add functions (or at Escape entry). Cleared ids stay
/// `None` until the next resolve; prefer clearing id when rewriting a callee name.
pub fn resolve_module_call_fun_ids(module: &mut crate::CoreModule) {
    use crate::ir::FunId;
    let mut by_name: HashMap<String, FunId> = HashMap::default();
    by_name.reserve(module.functions.len());
    for (i, f) in module.functions.iter().enumerate() {
        by_name.insert(f.name.clone(), FunId(i as u32));
    }
    for f in &mut module.functions {
        for_each_op_value_mut(&mut f.body, &mut |value| match value {
            Value::Call { fun, .. } | Value::FunRef(fun) | Value::AllocClosure { fun, .. } => {
                fun.id = by_name.get(&fun.name).copied();
            }
            _ => {}
        });
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
            ops: vec![Op::Return { value: Local(3) }],
            result: None,
        };
        let mut locals = HashSet::default();
        let mut names = HashSet::default();
        collect_uses(&block, &mut locals, &mut names);
        assert!(locals.contains(&3));

        let mut remap = HashMap::default();
        remap.insert(3, 9);
        rewrite_block_locals(&mut block, &remap);
        match &block.ops[0] {
            Op::Return { value } => assert_eq!(value.0, 9),
            _ => panic!("expected return"),
        }
        assert_eq!(max_local_in_block(&block), 9);
    }

    #[test]
    fn count_ops_includes_nested_if() {
        let block = Block {
            ops: vec![Op::Let {
                local: Local(0),
                value: Value::If {
                    cond: Local(1),
                    then_block: Box::new(Block {
                        ops: vec![Op::Let {
                            local: Local(2),
                            value: Value::Int(1),
                            pure_region: true,
                        }],
                        result: Some(Local(2)),
                    }),
                    else_block: Box::new(Block {
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
            ops: vec![Op::Return { value: Local(0) }],
            result: None,
        }));
        assert!(block_calls(
            &Block {
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
            ops: vec![Op::Assign {
                name: "x".into(),
                value: Local(0),
            }],
            result: None,
        }));
    }

    #[test]
    fn collect_loops_finds_nested_under_if() {
        let inner = Value::Loop {
            header: Box::new(Block {
                ops: vec![],
                result: Some(Local(1)),
            }),
            body: Box::new(Block {
                ops: vec![],
                result: None,
            }),
            latch: Box::new(Block {
                ops: vec![],
                result: None,
            }),
        };
        let block = Block {
            ops: vec![Op::Let {
                local: Local(0),
                value: Value::If {
                    cond: Local(2),
                    then_block: Box::new(Block {
                        ops: vec![Op::Let {
                            local: Local(3),
                            value: inner,
                            pure_region: true,
                        }],
                        result: Some(Local(3)),
                    }),
                    else_block: Box::new(Block {
                        ops: vec![],
                        result: Some(Local(4)),
                    }),
                },
                pure_region: true,
            }],
            result: Some(Local(0)),
        };
        let mut loops = vec![];
        collect_loops(&block, &mut loops);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].0.result, Some(Local(1)));
    }

    #[test]
    fn resolve_module_call_fun_ids_fills_call_target() {
        use crate::{CoreFun, CoreModule, FunId, FunKind};
        use lumia_ty::{Effect, Type};

        let callee = CoreFun {
            name: "g".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body: Block {
                ops: vec![],
                result: Some(Local(0)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: crate::ForeignAbi::C,
            escaping: Default::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        };
        let caller = CoreFun {
            name: "f".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body: Block {
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::FunRef("g".into()),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Call {
                            fun: "g".into(),
                            args: vec![],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(1)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: crate::ForeignAbi::C,
            escaping: Default::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        };
        let mut module = CoreModule::with_functions("M", vec![callee, caller]);
        resolve_module_call_fun_ids(&mut module);
        let Op::Let {
            value: Value::FunRef(fr),
            ..
        } = &module.functions[1].body.ops[0]
        else {
            panic!("expected FunRef");
        };
        assert_eq!(fr.id, Some(FunId(0)));
        let Op::Let {
            value: Value::Call { fun, .. },
            ..
        } = &module.functions[1].body.ops[1]
        else {
            panic!("expected Call");
        };
        assert_eq!(fun.as_str(), "g");
        assert_eq!(fun.id, Some(FunId(0)));
    }
}
