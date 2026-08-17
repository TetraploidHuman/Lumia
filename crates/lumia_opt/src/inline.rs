//! Function inlining (DESIGN §7.2 Inline + Specialization).
//!
//! Release-only: inline small pure leaf functions at direct call sites.
//! Skips `main`, `foreign`, memoized, recursive, and effectful callees.
//! Callees that use `var` (Assign / Name) are allowed: slot names are renamed
//! to `$inl{tag}_…` so they cannot clash with the caller's mutable slots.

use lumia_abi::INLINE_MAX_EXPAND_DEPTH;
use lumia_core::{
    block_calls, collect_defined_locals, collect_slot_names, count_ops, for_each_block_dfs,
    has_early_return, max_local_in_fun, rewrite_block_locals, Block, CoreFun, CoreModule, Local, Op,
    Value,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Max SSA ops in callee body (including nested) before we refuse to inline.
/// Sized to cover small helpers with a loop + vars (e.g. `isPrime`, `collatzSteps`).
const INLINE_MAX_OPS: usize = 32;

pub struct InlinePass;

impl InlinePass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        inline_module(module);
    }
}

fn inline_module(module: &mut CoreModule) {
    // Index only — clone callee bodies on first hit (see `ensure_inline_cached`).
    let indices: HashMap<String, usize> = module
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| is_inlineable(f))
        .map(|(i, f)| (f.name.clone(), i))
        .collect();
    if indices.is_empty() {
        return;
    }

    let mut functions = std::mem::take(&mut module.functions);
    let mut cache: HashMap<String, CoreFun> = HashMap::default();
    let mut name_tag = 0u32;

    for i in 0..functions.len() {
        // Prefetch transitive inlineable callees while `functions` is shared.
        prefetch_inline_callees(&functions[i].body, &indices, &functions, &mut cache);
        let mut next = max_local_in_fun(&functions[i]).saturating_add(1);
        let caller = functions[i].name.clone();
        let mut expanding = HashSet::default();
        inline_block(
            &mut functions[i].body,
            &cache,
            &caller,
            &mut next,
            &mut name_tag,
            &mut expanding,
            0,
        );
    }
    module.functions = functions;
}

/// Walk direct `Call` targets; clone reachable inlineables into `cache` once.
fn prefetch_inline_callees(
    block: &Block,
    indices: &HashMap<String, usize>,
    functions: &[CoreFun],
    cache: &mut HashMap<String, CoreFun>,
) {
    // Cache fill is order-independent — DFS is safe.
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value: Value::Call { fun, .. },
                ..
            } = op
            {
                ensure_inline_cached(fun, indices, functions, cache);
            }
        }
    });
}

fn ensure_inline_cached(
    name: &str,
    indices: &HashMap<String, usize>,
    functions: &[CoreFun],
    cache: &mut HashMap<String, CoreFun>,
) {
    if cache.contains_key(name) {
        return;
    }
    let Some(&idx) = indices.get(name) else {
        return;
    };
    // Insert before recursing so mutual inlineables do not re-enter.
    cache.insert(name.to_string(), functions[idx].clone());
    prefetch_inline_callees(&functions[idx].body, indices, functions, cache);
}

fn is_inlineable(f: &CoreFun) -> bool {
    if f.is_main || f.external.is_some() || f.memo.is_some() {
        return false;
    }
    if !f.effect.is_pure() {
        return false;
    }
    if count_ops(&f.body) > INLINE_MAX_OPS {
        return false;
    }
    if block_calls(&f.body, &f.name) {
        return false; // recursive
    }
    // Early return must stay in the callee; inlining would return from the caller.
    if has_early_return(&f.body) {
        return false;
    }
    true
}

fn inline_block(
    block: &mut Block,
    inlineable: &HashMap<String, CoreFun>,
    caller: &str,
    next: &mut u32,
    name_tag: &mut u32,
    expanding: &mut HashSet<String>,
    depth: usize,
) {
    let mut out: Vec<Op> = Vec::with_capacity(block.ops.len());
    for op in std::mem::take(&mut block.ops) {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } => {
                let mut value = value;
                inline_value(
                    &mut value, inlineable, caller, next, name_tag, expanding, depth,
                );
                if let Value::Call { fun, args } = &value {
                    // Refuse self-recursion, re-entry on the expand stack (mutual
                    // a↔b), and runaway nested expand depth.
                    let can_expand = fun != caller
                        && depth < INLINE_MAX_EXPAND_DEPTH
                        && !expanding.contains(fun);
                    if can_expand {
                        if let Some(callee) = inlineable.get(fun) {
                            if args.len() == callee.params.len() {
                                let (prelude, result) =
                                    materialize_inline(callee, args, next, name_tag);
                                // Callee snapshot may still contain calls to other
                                // inlineable leaves — expand those against the same map.
                                let mut nested = Block {
                                    ops: prelude,
                                    result: Some(result),
                                };
                                expanding.insert(fun.clone());
                                inline_block(
                                    &mut nested,
                                    inlineable,
                                    fun.as_str(),
                                    next,
                                    name_tag,
                                    expanding,
                                    depth + 1,
                                );
                                expanding.remove(fun);
                                let result =
                                    nested.result.expect("inlined function must return a value");
                                out.extend(nested.ops);
                                out.push(Op::Let {
                                    local,
                                    value: Value::Local(result),
                                    pure_region,
                                });
                                continue;
                            }
                        }
                    }
                }
                out.push(Op::Let {
                    local,
                    value,
                    pure_region,
                });
            }
            other => out.push(other),
        }
    }
    block.ops = out;
}

fn inline_value(
    value: &mut Value,
    inlineable: &HashMap<String, CoreFun>,
    caller: &str,
    next: &mut u32,
    name_tag: &mut u32,
    expanding: &mut HashSet<String>,
    depth: usize,
) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            inline_block(
                then_block, inlineable, caller, next, name_tag, expanding, depth,
            );
            inline_block(
                else_block, inlineable, caller, next, name_tag, expanding, depth,
            );
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            inline_block(header, inlineable, caller, next, name_tag, expanding, depth);
            inline_block(body, inlineable, caller, next, name_tag, expanding, depth);
            inline_block(latch, inlineable, caller, next, name_tag, expanding, depth);
        }
        Value::Lambda { body, .. } => {
            inline_block(body, inlineable, caller, next, name_tag, expanding, depth)
        }
        _ => {}
    }
}

fn materialize_inline(
    callee: &CoreFun,
    args: &[Local],
    next: &mut u32,
    name_tag: &mut u32,
) -> (Vec<Op>, Local) {
    let mut body = callee.body.clone();
    let mut remap: HashMap<u32, u32> = HashMap::default();

    // Map params → actuals (no new locals).
    for (p, a) in callee.params.iter().zip(args.iter()) {
        remap.insert(p.0, a.0);
    }

    // Fresh ids for every other local defined in the callee.
    let mut defined: HashSet<u32> = HashSet::default();
    collect_defined_locals(&body, &mut defined);
    for id in defined {
        if callee.params.iter().any(|p| p.0 == id) {
            continue;
        }
        let fresh = *next;
        *next += 1;
        remap.insert(id, fresh);
    }

    rewrite_block_locals(&mut body, &remap);

    // Rename mutable slots so they cannot collide with the caller's vars.
    let mut slot_names: HashSet<String> = HashSet::default();
    collect_slot_names(&body, &mut slot_names);
    if !slot_names.is_empty() {
        let tag = *name_tag;
        *name_tag += 1;
        let name_remap: HashMap<String, String> = slot_names
            .into_iter()
            .map(|n| (n.clone(), format!("$inl{tag}_{n}")))
            .collect();
        rewrite_block_slot_names(&mut body, &name_remap);
    }

    let result = body
        .result
        .expect("inlineable function must return a value");
    // Drop block.result; splice ops into caller.
    (body.ops, result)
}

fn rewrite_block_slot_names(block: &mut Block, remap: &HashMap<String, String>) {
    if remap.is_empty() {
        return;
    }
    for op in &mut block.ops {
        match op {
            Op::Assign { name, .. } => {
                if let Some(n) = remap.get(name) {
                    *name = n.clone();
                }
            }
            Op::Let { value, .. } => {
                rewrite_value_slot_names(value, remap);
            }
            _ => {}
        }
    }
}

fn rewrite_value_slot_names(value: &mut Value, remap: &HashMap<String, String>) {
    match value {
        Value::Name(n) => {
            if let Some(r) = remap.get(n) {
                *n = r.clone();
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_block_slot_names(then_block, remap);
            rewrite_block_slot_names(else_block, remap);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_block_slot_names(header, remap);
            rewrite_block_slot_names(body, remap);
            rewrite_block_slot_names(latch, remap);
        }
        Value::Lambda { body, .. } => rewrite_block_slot_names(body, remap),
        _ => {}
    }
}

#[cfg(test)]
#[path = "inline_tests.rs"]
mod tests;
