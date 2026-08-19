//! Shared walks over [`Value`] / [`Block`] local operands.
//!
//! New `Value` arms that carry `Local`s should update [`for_each_local_mut`] so
//! remap / collect / max-local stay exhaustive in one place.

use crate::ir::{Block, CoreFun, CoreModule, Local, Op, Value};
use lumia_syntax::Sym;
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
        names.insert(n.as_str().to_string());
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

/// Fork-join over `Value::If` arms with independent environments.
///
/// Used by analyses that must not leak branch-local facts across arms (memo
/// structural recursion, const-arg frequency). Returns `None` when `value`
/// is not an If. Loop stays sequential via [`for_each_nested_block`].
pub fn map_if_branches<E>(
    value: &Value,
    seed: &E,
    fork: impl Fn(&E) -> E,
    mut walk: impl FnMut(&Block, &mut E),
) -> Option<(E, E)> {
    let Value::If {
        then_block,
        else_block,
        ..
    } = value
    else {
        return None;
    };
    let mut then_env = fork(seed);
    let mut else_env = fork(seed);
    walk(then_block, &mut then_env);
    walk(else_block, &mut else_env);
    Some((then_env, else_env))
}

/// In-block sequential Let walk with **If fork-join** and **Loop sequential env**.
///
/// Used by memo plan (structural recursion / const-arg reuse) and any analysis that
/// must preserve Let-order within a block but fork on `If` and clone-through on `Loop`.
/// Does not enter `Lambda` bodies.
pub fn for_each_let_in_block_ctrl<E>(
    block: &Block,
    env: &mut E,
    fork: &impl Fn(&E) -> E,
    on_let: &mut impl FnMut(Local, &Value, &mut E),
    on_control_let: &mut impl FnMut(Local, &Value, &mut E),
    merge_if: &impl Fn(&mut E, E, E),
    merge_loop: &impl Fn(&mut E, E),
) {
    for_each_let_in_block(block, &mut |local, value, _pure| match value {
        Value::If { .. } => {
            on_control_let(local, value, env);
            if let Some((t, e)) = map_if_branches(value, env, fork, |b, e| {
                for_each_let_in_block_ctrl(
                    b,
                    e,
                    fork,
                    on_let,
                    on_control_let,
                    merge_if,
                    merge_loop,
                );
            }) {
                merge_if(env, t, e);
            }
        }
        Value::Loop { .. } => {
            on_control_let(local, value, env);
            let mut loop_env = fork(env);
            for_each_nested_block(value, &mut |b| {
                for_each_let_in_block_ctrl(
                    b,
                    &mut loop_env,
                    fork,
                    on_let,
                    on_control_let,
                    merge_if,
                    merge_loop,
                );
            });
            merge_loop(env, loop_env);
        }
        Value::Lambda { .. } => on_control_let(local, value, env),
        _ => on_let(local, value, env),
    });
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
pub fn collect_assigned_names(block: &Block, out: &mut HashSet<Sym>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Assign { name, .. } = op {
                out.insert(name.clone());
            }
        }
    });
}

/// Collect slot names from `Assign` writes and `Value::Name` loads (DFS).
pub fn collect_slot_names(block: &Block, names: &mut HashSet<Sym>) {
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

/// In-block **op order** over `Op::Let` only (no nested entry — caller handles If/Loop in the callback).
///
/// Prefer [`for_each_let`] when order across nested blocks does not matter (DFS).
pub fn for_each_let_in_block(block: &Block, f: &mut impl FnMut(Local, &Value, bool)) {
    for_each_top_level_op_in_block(block, &mut |op| {
        if let Op::Let {
            local,
            value,
            pure_region,
            ..
        } = op
        {
            f(*local, value, *pure_region);
        }
    });
}

/// Mutating in-block Let walk (same order contract as [`for_each_let_in_block`]).
pub fn for_each_let_in_block_mut(block: &mut Block, f: &mut impl FnMut(Local, &mut Value, bool)) {
    for op in &mut block.ops {
        if let Op::Let {
            local,
            value,
            pure_region,
            ..
        } = op
        {
            f(*local, value, *pure_region);
        }
    }
}

/// Current block only — every `Op` in sequential order (no nested entry).
pub fn for_each_top_level_op_in_block(block: &Block, f: &mut impl FnMut(&Op)) {
    for op in &block.ops {
        f(op);
    }
}

/// Mutating in-block op walk (same order contract as [`for_each_top_level_op_in_block`]).
pub fn for_each_top_level_op_in_block_mut(block: &mut Block, f: &mut impl FnMut(&mut Op)) {
    for op in &mut block.ops {
        f(op);
    }
}

/// SSA def lookup: sequential in-block `Let`s, then nested If/Loop/Lambda bodies on earlier binds.
///
/// Shared by float ABI chase, ABI refresh alias walk, and heap typing. Does **not** follow
/// `Assign`/`Name` slots — only SSA `Op::Let`.
pub fn find_local_def<'a>(block: &'a Block, id: u32) -> Option<&'a Value> {
    for op in &block.ops {
        let Op::Let { local, value, .. } = op else {
            continue;
        };
        if local.0 == id {
            return Some(value);
        }
        if let Some(v) = find_local_def_in_value(value, id) {
            return Some(v);
        }
    }
    None
}

/// Nested-region half of [`find_local_def`] (If/Loop/Lambda only).
pub fn find_local_def_in_value<'a>(value: &'a Value, id: u32) -> Option<&'a Value> {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => find_local_def(then_block, id).or_else(|| find_local_def(else_block, id)),
        Value::Loop {
            header,
            body,
            latch,
        } => find_local_def(header, id)
            .or_else(|| find_local_def(body, id))
            .or_else(|| find_local_def(latch, id)),
        Value::Lambda { body, .. } => find_local_def(body, id),
        _ => None,
    }
}

/// Direct `Op::Let` in `block.ops` only — no nested search through earlier bind values.
///
/// Used when alias chase must stay in the current SSA block (e.g. lifted-lambda heap reachability).
pub fn find_top_level_local_def<'a>(block: &'a Block, id: u32) -> Option<&'a Value> {
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if local.0 == id {
                return Some(value);
            }
        }
    }
    None
}

/// Take/splice top-level ops: each input op becomes zero or more output ops in order.
///
/// Shared entry for passes that expand or remove ops (`inline`, `lambda_lift`, LICM, …).
pub fn flat_map_top_level_ops_in_block(block: &mut Block, f: &mut impl FnMut(Op) -> Vec<Op>) {
    let mut out = Vec::with_capacity(block.ops.len());
    for op in std::mem::take(&mut block.ops) {
        out.extend(f(op));
    }
    block.ops = out;
}

/// Current block only — every top-level `Op::Assign` in sequential order.
pub fn for_each_assign_in_block(block: &Block, f: &mut impl FnMut(&str, Local)) {
    for_each_top_level_op_in_block(block, &mut |op| {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            f(name, Local(*v));
        }
    });
}

/// Every `Assign` to `name` under `block`, including nested If/Loop/Lambda regions.
///
/// Shared by float ABI slot heap typing and mono `ret_ty` slot fixed-type joins.
pub fn for_each_named_slot_assign_in_block(block: &Block, name: &Sym, f: &mut impl FnMut(Local)) {
    for_each_op_in_block(block, &mut |op| {
        if let Op::Assign {
            name: n,
            value: Local(v),
        } = op
        {
            if *n == *name {
                f(Local(*v));
            }
        }
    });
}

/// Pre-loop top-level Lets only — stops before the first `Op::Let { Value::Loop }`.
pub fn for_each_pre_loop_let_in_block(block: &Block, f: &mut impl FnMut(Local, &Value)) {
    for op in &block.ops {
        if matches!(
            op,
            Op::Let {
                value: Value::Loop { .. },
                ..
            }
        ) {
            break;
        }
        if let Op::Let { local, value, .. } = op {
            f(*local, value);
        }
    }
}

/// Whether any `Op::Break` appears under `block` (DFS, all blocks).
pub fn block_has_break(block: &Block) -> bool {
    let mut found = false;
    for_each_block_dfs(block, &mut |b| {
        for_each_top_level_op_in_block(b, &mut |op| {
            if matches!(op, Op::Break) {
                found = true;
            }
        });
    });
    found
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
pub fn collect_alloc_closure_caps(block: &Block, lam_caps: &mut HashMap<Sym, Vec<Local>>) {
    for_each_let_value(block, &mut |_b, value| {
        if let Value::AllocClosure { fun, captures } = value {
            lam_caps.insert(fun.name.clone(), captures.clone());
        }
    });
}

/// Collect lifted funs whose `AllocClosure` has a non-empty env (DFS-safe).
pub fn collect_alloc_closure_env_funs(block: &Block, out: &mut HashSet<Sym>) {
    for_each_let_value(block, &mut |_b, value| {
        if let Value::AllocClosure { fun, captures } = value {
            if !captures.is_empty() {
                out.insert(fun.name.clone());
            }
        }
    });
}

/// Collect Call targets that appear in `methods` (DFS-safe; enters Lambda bodies).
pub fn collect_call_names_in(block: &Block, methods: &HashSet<Sym>, out: &mut HashSet<Sym>) {
    for_each_let_value(block, &mut |_b, value| {
        if let Value::Call { fun, .. } = value {
            if methods.contains(fun.as_str()) {
                out.insert(fun.name.clone());
            }
        }
    });
}

/// Collect `Assign` name → value locals (order-independent; DFS-safe).
pub fn collect_assigns(block: &Block, assigns: &mut HashMap<Sym, Vec<Local>>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Assign { name, value } = op {
                assigns.entry(name.clone()).or_default().push(*value);
            }
        }
    });
}

/// Track FunRef / AllocClosure aliases (SSA + named slots) and which capture
/// slots hold FunRefs.
///
/// **Let-ordered** (not DFS). Shared by mono `directize` and `float_cap_fixup`.
/// Nested If/Loop inherit a clone; Lambda starts fresh (same as directize).
pub fn collect_closure_cap_funrefs(
    block: &Block,
    funref_locals: &mut HashMap<u32, Sym>,
    cap_funs: &mut HashMap<Sym, HashMap<u32, Sym>>,
) {
    let mut aliases = crate::FunRefAliases {
        locals: std::mem::take(funref_locals),
        slots: HashMap::default(),
    };
    aliases.walk_block(
        block,
        crate::FunRefAlloc::Track,
        None,
        &mut |value, aliases| {
            let Value::AllocClosure { fun, captures } = value else {
                return;
            };
            let entry = cap_funs.entry(fun.name.clone()).or_default();
            for (i, cap) in captures.iter().enumerate() {
                if let Some(n) = aliases.resolve(cap.0) {
                    entry.insert(i as u32, Sym::from(n));
                }
            }
        },
    );
    *funref_locals = aliases.locals;
}

/// Total SSA op count in `block` and nested If/Loop/Lambda bodies.
pub fn count_ops(block: &Block) -> usize {
    let mut n = 0;
    for_each_block_dfs(block, &mut |b| n += b.ops.len());
    n
}

/// Memo plan cost heuristic: top-level ops plus weighted nested control (If/Loop).
pub fn body_weight(block: &Block) -> usize {
    let mut n = block.ops.len();
    for op in &block.ops {
        if let Op::Let { value, .. } = op {
            n += let_value_weight(value);
        }
    }
    n
}

fn let_value_weight(value: &Value) -> usize {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => 1 + body_weight(then_block) + body_weight(else_block),
        Value::Loop {
            header,
            body,
            latch,
        } => 1 + body_weight(header) + body_weight(body) + body_weight(latch),
        Value::Call { .. } | Value::IndirectCall { .. } | Value::Builtin { .. } => 1,
        _ => 0,
    }
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

/// Whether `block` has early return.
pub fn has_early_return(block: &Block) -> bool {
    let mut found = false;
    for_each_block_dfs(block, &mut |b| {
        if !found && b.ops.iter().any(|op| matches!(op, Op::Return { .. })) {
            found = true;
        }
    });
    found
}

/// Peel a top-level SSA alias chain to its terminal [`Value`] (no nested-block defs).
pub fn peel_local_to_value<'a>(block: &'a Block, start: u32) -> Option<&'a Value> {
    let mut seen = HashSet::default();
    let mut cur = start;
    loop {
        if !seen.insert(cur) {
            return None;
        }
        match find_top_level_local_def(block, cur)? {
            Value::Local(Local(src)) => cur = *src,
            terminal => return Some(terminal),
        }
    }
}

/// Like [`peel_local_to_value`] starting from [`Block::result`].
pub fn peel_block_result<'a>(block: &'a Block) -> Option<&'a Value> {
    let Local(r) = block.result?;
    peel_local_to_value(block, r)
}

/// Unreachable / exhaustiveness arm (`MatchFail`) — compatible with any result ty.
///
/// Shared by float heap ABI (bottom, not Unit) and mono `ret_ty` If joins.
pub fn block_result_is_bottom(block: &Block) -> bool {
    matches!(
        peel_block_result(block),
        Some(Value::Builtin {
            name: lumia_hir::Builtin::MatchFail,
            ..
        })
    )
}

/// `if` arm that is the Bool literal from `and`/`or` desugaring.
pub fn block_result_is_bool_lit(block: &Block, expect: bool) -> bool {
    matches!(peel_block_result(block), Some(Value::Bool(b)) if *b == expect)
}

/// Eager IO in `block` (not deferred nested-lambda bodies).
///
/// Used to mark lifted `__lam_*` [`crate::ir::CoreFun::effect`] so opt passes that
/// trust `effect.is_pure()` (inline / CSE / const-specialize) do not treat IO
/// thunks as pure. `io_callees` are known effectful top-level names.
pub fn block_has_io(block: &Block, io_callees: &HashSet<Sym>) -> bool {
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

fn value_has_eager_io(value: &Value, io_callees: &HashSet<Sym>) -> bool {
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

/// Every `Op::Let { Value::Loop { .. } }` under `block` (DFS into nested regions).
pub fn for_each_loop_in_block(block: &Block, f: &mut impl FnMut(&Block, &Block, &Block)) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
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
                f(header, body, latch);
            }
        }
    });
}

/// Loops bound directly in `block.ops` (no DFS — matches [`first_direct_loop`] scope per block).
pub fn for_each_direct_loop_in_block(block: &Block, f: &mut impl FnMut(&Block, &Block, &Block)) {
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
            f(header, body, latch);
        }
    }
}

/// Collect SSA locals from `Let { Value::Name(n) }` in one block (`names` may be a singleton).
pub fn collect_name_loads_in_block(block: &Block, names: &HashSet<Sym>, out: &mut HashSet<u32>) {
    for op in &block.ops {
        if let Op::Let {
            local,
            value: Value::Name(n),
            ..
        } = op
        {
            if names.contains(n.as_str()) {
                out.insert(local.0);
            }
        }
    }
}

/// DFS: collect every `Let { Name(name) }` local under `block`.
pub fn collect_name_load_locals(block: &Block, name: &str, out: &mut HashSet<u32>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                local,
                value: Value::Name(n),
                ..
            } = op
            {
                if n == name {
                    out.insert(local.0);
                }
            }
        }
    });
}

/// DFS: collect locals loading any name in `names`.
pub fn collect_name_load_locals_any(
    block: &Block,
    names: &HashSet<Sym>,
    out: &mut HashSet<u32>,
) {
    for_each_block_dfs(block, &mut |b| collect_name_loads_in_block(b, names, out));
}

/// Sequential in-block op walk: every `Op`, then nested If/Loop/Lambda bodies on `Let`.
///
/// Unlike [`for_each_op_value`], visits **all** op kinds (`Assign`, `Return`, …).
/// Nested regions are entered only through `Let` values (same as escape propagate).
pub fn for_each_op_in_block(block: &Block, f: &mut impl FnMut(&Op)) {
    for op in &block.ops {
        f(op);
        if let Op::Let { value, .. } = op {
            for_each_nested_block(value, &mut |nested| for_each_op_in_block(nested, f));
        }
    }
}

/// Mutating counterpart of [`for_each_op_in_block`].
pub fn for_each_op_in_block_mut(block: &mut Block, f: &mut impl FnMut(&mut Op)) {
    for op in &mut block.ops {
        f(op);
        if let Op::Let { value, .. } = op {
            for_each_nested_block_mut(value, &mut |nested| for_each_op_in_block_mut(nested, f));
        }
    }
}

/// Whether any `Op::Let` under control regions (not Lambda) binds an `If`.
pub fn block_has_if_let(block: &Block) -> bool {
    let mut found = false;
    for_each_let_value_ctrl(block, &mut |_b, val| {
        if matches!(val, Value::If { .. }) {
            found = true;
        }
    });
    found
}

/// Sequential in-block walk: each `Op::Let` value, then nested If/Loop/Lambda regions.
///
/// Preserves **in-block op order** (unlike [`for_each_let_value`] DFS). Prefer this
/// for passes that walk one block at a time and recurse into nested control regions.
pub fn for_each_op_value(block: &Block, f: &mut impl FnMut(&Value)) {
    for op in &block.ops {
        if let Op::Let { value, .. } = op {
            f(value);
            for_each_nested_block(value, &mut |nested| for_each_op_value(nested, f));
        }
    }
}

/// Visit nested If/Loop bodies in each top-level `Op::Let` (skips Lambda).
pub fn for_each_ctrl_nested_in_block_mut(block: &mut Block, f: &mut impl FnMut(&mut Block)) {
    for op in &mut block.ops {
        if let Op::Let { value, .. } = op {
            for_each_ctrl_nested_block_mut(value, f);
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
    use lumia_syntax::Sym;
    let mut by_name: HashMap<Sym, FunId> = HashMap::default();
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
    fn map_if_branches_forks_and_returns_both_envs() {
        let v = Value::If {
            cond: Local(0),
            then_block: Box::new(Block {
                ops: vec![Op::Let {
                    local: Local(1),
                    value: Value::Int(1),
                    pure_region: true,
                }],
                result: Some(Local(1)),
            }),
            else_block: Box::new(Block {
                ops: vec![
                    Op::Let {
                        local: Local(2),
                        value: Value::Int(2),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Int(3),
                        pure_region: true,
                    },
                ],
                result: Some(Local(3)),
            }),
        };
        let (then_n, else_n) =
            map_if_branches(&v, &0usize, |n| *n, |b, n| *n += b.ops.len()).expect("If should fork");
        assert_eq!(then_n, 1);
        assert_eq!(else_n, 2);
        assert!(map_if_branches(&Value::Int(0), &0usize, |n| *n, |_, _| ()).is_none());
    }

    #[test]
    fn for_each_let_in_block_ctrl_forks_if_and_sequential_loop() {
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::If {
                        cond: Local(0),
                        then_block: Box::new(Block {
                            ops: vec![Op::Let {
                                local: Local(3),
                                value: Value::Int(10),
                                pure_region: true,
                            }],
                            result: Some(Local(3)),
                        }),
                        else_block: Box::new(Block {
                            ops: vec![Op::Let {
                                local: Local(4),
                                value: Value::Int(20),
                                pure_region: true,
                            }],
                            result: Some(Local(4)),
                        }),
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(2)),
        };
        let mut sum = 0usize;
        let mut control = 0usize;
        let fork = |n: &usize| *n;
        let mut on_let = |_: Local, value: &Value, env: &mut usize| {
            if let Value::Int(n) = value {
                *env += *n as usize;
            }
        };
        let mut on_control = |_: Local, _: &Value, _: &mut usize| {
            control += 1;
        };
        for_each_let_in_block_ctrl(
            &block,
            &mut sum,
            &fork,
            &mut on_let,
            &mut on_control,
            &|dst, t, e| {
                *dst = t + e;
            },
            &|dst, h| *dst += h,
        );
        assert_eq!(sum, 32); // 1 + (1+10) + (1+20) with merge_if = t+e
        assert_eq!(control, 1); // If binding
    }

    #[test]
    fn for_each_op_value_respects_block_order() {
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Int(2),
                    pure_region: true,
                },
            ],
            result: None,
        };
        let mut seen = Vec::new();
        for_each_op_value(&block, &mut |v| {
            if let Value::Int(n) = v {
                seen.push(*n);
            }
        });
        assert_eq!(seen, vec![1, 2]);
    }

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

    fn block_result_is_bottom_sees_match_fail() {
        let block = Block {
            ops: vec![Op::Let {
                local: Local(1),
                value: Value::Builtin {
                    name: lumia_hir::Builtin::MatchFail,
                    args: vec![],
                    result_ty: None,
                },
                pure_region: true,
            }],
            result: Some(Local(1)),
        };
        assert!(block_result_is_bottom(&block));
    }

    #[test]
    fn peel_block_result_follows_local_alias_chain() {
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::Float(1.5),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Local(Local(1)),
                    pure_region: true,
                },
            ],
            result: Some(Local(2)),
        };
        assert!(
            matches!(peel_block_result(&block), Some(Value::Float(v)) if (*v - 1.5).abs() < 1e-9)
        );
    }

    #[test]
    fn block_result_is_bool_lit_sees_alias_chain() {
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::Bool(true),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Local(Local(1)),
                    pure_region: true,
                },
            ],
            result: Some(Local(2)),
        };
        assert!(block_result_is_bool_lit(&block, true));
        assert!(!block_result_is_bool_lit(&block, false));
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
    fn body_weight_counts_nested_control() {
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Call {
                        fun: "f".into(),
                        args: vec![],
                    },
                    pure_region: false,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::If {
                        cond: Local(2),
                        then_block: Box::new(Block {
                            ops: vec![Op::Let {
                                local: Local(3),
                                value: Value::Int(1),
                                pure_region: true,
                            }],
                            result: None,
                        }),
                        else_block: Box::new(Block {
                            ops: vec![],
                            result: None,
                        }),
                    },
                    pure_region: false,
                },
            ],
            result: None,
        };
        // 2 top-level + 1 (If) + 1 (then op)
        assert_eq!(body_weight(&block), 5);
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

    #[test]
    fn for_each_op_in_block_visits_assign_and_nested_ops_in_order() {
        let mut seen = Vec::new();
        let block = Block {
            ops: vec![
                Op::Assign {
                    name: "a".into(),
                    value: Local(0),
                },
                Op::Let {
                    local: Local(1),
                    value: Value::If {
                        cond: Local(2),
                        then_block: Box::new(Block {
                            ops: vec![Op::Assign {
                                name: "t".into(),
                                value: Local(3),
                            }],
                            result: None,
                        }),
                        else_block: Box::new(Block {
                            ops: vec![],
                            result: None,
                        }),
                    },
                    pure_region: true,
                },
            ],
            result: None,
        };
        for_each_op_in_block(&block, &mut |op| {
            if let Op::Assign { name, .. } = op {
                seen.push(name.clone());
            }
        });
        assert_eq!(seen, vec!["a".to_string(), "t".to_string()]);
        assert!(block_has_if_let(&block));
    }

    #[test]
    fn for_each_loop_in_block_finds_nested_loops() {
        let inner = Block {
            ops: vec![],
            result: None,
        };
        let outer_body = Block {
            ops: vec![Op::Let {
                local: Local(1),
                value: Value::Loop {
                    header: Box::new(Block {
                        ops: vec![],
                        result: None,
                    }),
                    body: Box::new(inner),
                    latch: Box::new(Block {
                        ops: vec![],
                        result: None,
                    }),
                },
                pure_region: true,
            }],
            result: None,
        };
        let mut n = 0;
        for_each_loop_in_block(&outer_body, &mut |_, _, _| n += 1);
        assert_eq!(n, 1);
    }

    #[test]
    fn for_each_op_in_block_mut_updates_nested_assign() {
        let mut block = Block {
            ops: vec![Op::Let {
                local: Local(0),
                value: Value::Loop {
                    header: Box::new(Block {
                        ops: vec![],
                        result: None,
                    }),
                    body: Box::new(Block {
                        ops: vec![Op::Assign {
                            name: "x".into(),
                            value: Local(1),
                        }],
                        result: None,
                    }),
                    latch: Box::new(Block {
                        ops: vec![],
                        result: None,
                    }),
                },
                pure_region: true,
            }],
            result: None,
        };
        for_each_op_in_block_mut(&mut block, &mut |op| {
            if let Op::Assign { value, .. } = op {
                *value = Local(2);
            }
        });
        let loop_body = match &block.ops[0] {
            Op::Let {
                value: Value::Loop { body, .. },
                ..
            } => body,
            _ => panic!("expected loop"),
        };
        assert!(matches!(
            &loop_body.ops[0],
            Op::Assign {
                value: Local(2),
                ..
            }
        ));
    }

    #[test]
    fn flat_map_top_level_ops_splices_and_drops() {
        let mut block = Block {
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Break,
                Op::Continue,
            ],
            result: None,
        };
        flat_map_top_level_ops_in_block(&mut block, &mut |op| match op {
            Op::Break => vec![],
            Op::Continue => vec![
                Op::Assign {
                    name: "x".into(),
                    value: Local(0),
                },
                Op::Continue,
            ],
            other => vec![other],
        });
        assert_eq!(block.ops.len(), 3);
        assert!(matches!(&block.ops[0], Op::Let { .. }));
        assert!(matches!(
            &block.ops[1],
            Op::Assign { name, .. } if name == "x"
        ));
        assert!(matches!(&block.ops[2], Op::Continue));
    }

    #[test]
    fn collect_name_load_locals_dfs() {
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(5),
                    value: Value::Name("i".into()),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::If {
                        cond: Local(2),
                        then_block: Box::new(Block {
                            ops: vec![Op::Let {
                                local: Local(6),
                                value: Value::Name("i".into()),
                                pure_region: true,
                            }],
                            result: None,
                        }),
                        else_block: Box::new(Block {
                            ops: vec![],
                            result: None,
                        }),
                    },
                    pure_region: true,
                },
            ],
            result: None,
        };
        let mut ids = HashSet::default();
        collect_name_load_locals(&block, "i", &mut ids);
        assert!(ids.contains(&5));
        assert!(ids.contains(&6));
    }

    #[test]
    fn find_local_def_sequential_and_nested() {
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::If {
                        cond: Local(0),
                        then_block: Box::new(Block {
                            ops: vec![Op::Let {
                                local: Local(2),
                                value: Value::Float(1.0),
                                pure_region: true,
                            }],
                            result: Some(Local(2)),
                        }),
                        else_block: Box::new(Block {
                            ops: vec![],
                            result: None,
                        }),
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(3),
                    value: Value::Local(Local(9)),
                    pure_region: true,
                },
            ],
            result: Some(Local(3)),
        };
        assert!(matches!(
            find_local_def(&block, 2),
            Some(Value::Float(v)) if (*v - 1.0).abs() < f64::EPSILON
        ));
        assert!(matches!(
            find_local_def(&block, 3),
            Some(Value::Local(Local(9)))
        ));
        assert!(find_local_def(&block, 99).is_none());
        assert!(find_top_level_local_def(&block, 2).is_none());
        assert!(matches!(
            find_top_level_local_def(&block, 3),
            Some(Value::Local(Local(9)))
        ));
    }

    #[test]
    fn for_each_named_slot_assign_in_block_nested() {
        let block = Block {
            ops: vec![Op::Let {
                local: Local(1),
                value: Value::If {
                    cond: Local(0),
                    then_block: Box::new(Block {
                        ops: vec![Op::Assign {
                            name: "acc".into(),
                            value: Local(2),
                        }],
                        result: None,
                    }),
                    else_block: Box::new(Block {
                        ops: vec![Op::Assign {
                            name: "acc".into(),
                            value: Local(3),
                        }],
                        result: None,
                    }),
                },
                pure_region: true,
            }],
            result: None,
        };
        let mut srcs = Vec::new();
        let acc = lumia_syntax::Sym::from("acc");
        for_each_named_slot_assign_in_block(&block, &acc, &mut |Local(id)| srcs.push(id));
        srcs.sort_unstable();
        assert_eq!(srcs, vec![2, 3]);
    }
}
