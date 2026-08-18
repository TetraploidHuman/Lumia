//! Function inlining (DESIGN §7.2 Inline + Specialization).
//!
//! Release-only: inline small pure leaf functions at direct call sites, and at
//! `IndirectCall` sites whose callee is a proven `FunRef` (SSA-aliased).
//! Skips `main`, `foreign`, memoized, recursive, and effectful callees.
//! Callees that use `var` (Assign / Name) are allowed: slot names are uniquified
//! from the same Local id counter (`$s{id}`), preserving HIR desugar prefixes
//! so Float ABI classifiers still match.

use lumia_abi::INLINE_MAX_EXPAND_DEPTH;
use lumia_core::{
    block_calls, collect_defined_locals, collect_slot_names, count_ops, for_each_block_dfs,
    has_early_return, max_local_in_fun, rewrite_block_locals, Block, CoreFun, CoreModule, Local,
    Op, Value,
};
use lumia_hir::{FOLD_ELEM_PREFIX, FOR_INDEX_PREFIX, FUSE_ACC_PREFIX, LIST_BUILDER_ACC_PREFIXES};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::Arc;

/// Cached callee slice for inlining — params + body only (no full [`CoreFun`] clone).
struct InlineCallee {
    params: Vec<Local>,
    body: Arc<Block>,
}

/// Max SSA ops in callee body (including nested) before we refuse to inline.
/// Sized to cover small helpers with a loop + vars (e.g. `isPrime`, `collatzSteps`).
/// 64 is safe once Domain SR rewrites const-specialized `$c_` clones (unmatched
/// `matmulChecksum$c_2000` inlined into huge `main` previously caused ~7×).
///
/// Override with `LUMIA_INLINE_MAX_OPS` for diagnostics.
fn inline_max_ops() -> usize {
    static CACHED: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("LUMIA_INLINE_MAX_OPS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(64)
    })
}

pub struct InlinePass;

impl InlinePass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        inline_module(module);
    }
}

fn inline_module(module: &mut CoreModule) {
    let max_ops = inline_max_ops();
    // Index only — cache params + Arc<body> on first hit (see `ensure_inline_cached`).
    let indices: HashMap<String, usize> = module
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| is_inlineable(f))
        .map(|(i, f)| (f.name.clone(), i))
        .collect();
    if std::env::var_os("LUMIA_INLINE_DUMP").is_some() {
        let mut rows: Vec<(usize, String)> = module
            .functions
            .iter()
            .filter(|f| f.external.is_none() && !f.is_main)
            .map(|f| (count_ops(&f.body), f.name.clone()))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        eprintln!(
            "[inline] INLINE_MAX_OPS={max_ops} inlineable={}",
            indices.len()
        );
        for (ops, name) in &rows {
            let mark = if indices.contains_key(name) {
                "INLINEABLE"
            } else if *ops > max_ops {
                "too_big"
            } else {
                "blocked"
            };
            if *ops > 40 || indices.contains_key(name) {
                eprintln!("[inline]  {ops:>4}  {mark:<10}  {name}");
            }
        }
    }
    if indices.is_empty() {
        return;
    }

    let mut functions = std::mem::take(&mut module.functions);
    let mut cache: HashMap<String, InlineCallee> = HashMap::default();

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
            &mut expanding,
            0,
        );
    }
    module.functions = functions;
}

/// Walk direct `Call` / `FunRef` targets; cache reachable inlineables once.
fn prefetch_inline_callees(
    block: &Block,
    indices: &HashMap<String, usize>,
    functions: &[CoreFun],
    cache: &mut HashMap<String, InlineCallee>,
) {
    // Cache fill is order-independent — DFS is safe.
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            match op {
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } => {
                    ensure_inline_cached(fun.as_str(), indices, functions, cache);
                }
                Op::Let {
                    value: Value::FunRef(fun),
                    ..
                } => {
                    ensure_inline_cached(fun, indices, functions, cache);
                }
                _ => {}
            }
        }
    });
}

fn ensure_inline_cached(
    name: &str,
    indices: &HashMap<String, usize>,
    functions: &[CoreFun],
    cache: &mut HashMap<String, InlineCallee>,
) {
    if cache.contains_key(name) {
        return;
    }
    let Some(&idx) = indices.get(name) else {
        return;
    };
    // Insert before recursing so mutual inlineables do not re-enter.
    let f = &functions[idx];
    cache.insert(
        name.to_string(),
        InlineCallee {
            params: f.params.clone(),
            body: Arc::new(f.body.clone()),
        },
    );
    prefetch_inline_callees(&functions[idx].body, indices, functions, cache);
}

fn is_inlineable(f: &CoreFun) -> bool {
    if f.is_main || f.external.is_some() || f.memo.is_some() {
        return false;
    }
    if !f.effect.is_pure() {
        return false;
    }
    if count_ops(&f.body) > inline_max_ops() {
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
    inlineable: &HashMap<String, InlineCallee>,
    caller: &str,
    next: &mut u32,
    expanding: &mut HashSet<String>,
    depth: usize,
) {
    let mut out: Vec<Op> = Vec::with_capacity(block.ops.len());
    // Locals proven equal to `FunRef(name)` (SSA aliases) — enables IndirectCall expand.
    let mut funrefs: HashMap<u32, String> = HashMap::default();
    for op in std::mem::take(&mut block.ops) {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } => {
                let mut value = value;
                inline_value(&mut value, inlineable, caller, next, expanding, depth);
                // Resolve FunRef / IndirectCall → direct Call when possible.
                let expand_fun: Option<String> = match &value {
                    Value::Call { fun, .. } => Some(fun.name.clone()),
                    Value::IndirectCall { callee, .. } => funrefs.get(&callee.0).cloned(),
                    Value::FunRef(name) => {
                        funrefs.insert(local.0, name.name.clone());
                        None
                    }
                    Value::Local(Local(src)) => {
                        if let Some(n) = funrefs.get(src) {
                            funrefs.insert(local.0, n.clone());
                        }
                        None
                    }
                    _ => {
                        funrefs.remove(&local.0);
                        None
                    }
                };

                if let Some(fun) = expand_fun.as_deref() {
                    let args: Option<Vec<Local>> = match &value {
                        Value::Call { args, .. } | Value::IndirectCall { args, .. } => {
                            Some(args.clone())
                        }
                        _ => None,
                    };
                    let can_expand = fun != caller
                        && depth < INLINE_MAX_EXPAND_DEPTH
                        && !expanding.contains(fun);
                    if can_expand {
                        if let (Some(args), Some(callee)) = (args.as_ref(), inlineable.get(fun)) {
                            if args.len() == callee.params.len() {
                                let (prelude, result) = materialize_inline(callee, args, next);
                                let mut nested = Block {
                                    ops: prelude,
                                    result: Some(result),
                                };
                                expanding.insert(fun.to_string());
                                inline_block(
                                    &mut nested,
                                    inlineable,
                                    fun,
                                    next,
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
                                funrefs.remove(&local.0);
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
    inlineable: &HashMap<String, InlineCallee>,
    caller: &str,
    next: &mut u32,
    expanding: &mut HashSet<String>,
    depth: usize,
) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            inline_block(then_block, inlineable, caller, next, expanding, depth);
            inline_block(else_block, inlineable, caller, next, expanding, depth);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            inline_block(header, inlineable, caller, next, expanding, depth);
            inline_block(body, inlineable, caller, next, expanding, depth);
            inline_block(latch, inlineable, caller, next, expanding, depth);
        }
        Value::Lambda { body, .. } => {
            inline_block(body, inlineable, caller, next, expanding, depth)
        }
        _ => {}
    }
}

/// Uniquify an inlined mutable slot using the Local id space.
///
/// Keeps HIR desugar prefixes so Float ABI / fold classifiers still match.
fn unique_inline_slot_name(original: &str, id: u32) -> String {
    let prefixes = std::iter::once(FOR_INDEX_PREFIX)
        .chain(std::iter::once(FUSE_ACC_PREFIX))
        .chain(std::iter::once(FOLD_ELEM_PREFIX))
        .chain(LIST_BUILDER_ACC_PREFIXES.iter().copied());
    for p in prefixes {
        if original.starts_with(p) {
            return if p.ends_with('_') {
                format!("{p}s{id}")
            } else {
                format!("{p}_s{id}")
            };
        }
    }
    format!("$s{id}")
}

fn materialize_inline(callee: &InlineCallee, args: &[Local], next: &mut u32) -> (Vec<Op>, Local) {
    // Deep-clone from shared Arc only at expand time (caller mutates remap).
    let mut body = (*callee.body).clone();
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

    // Uniquify mutable slots from the same Local id counter (no `$inl{tag}_` protocol).
    let mut slot_names: HashSet<String> = HashSet::default();
    collect_slot_names(&body, &mut slot_names);
    if !slot_names.is_empty() {
        let name_remap: HashMap<String, String> = slot_names
            .into_iter()
            .map(|n| {
                let id = *next;
                *next += 1;
                let fresh = unique_inline_slot_name(&n, id);
                (n, fresh)
            })
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
