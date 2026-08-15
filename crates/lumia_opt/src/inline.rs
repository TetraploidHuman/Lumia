//! Function inlining (DESIGN §7.2 Inline + Specialization).
//!
//! Release-only: inline small pure leaf functions at direct call sites.
//! Skips `main`, `foreign`, memoized, recursive, and effectful callees.
//! Callees that use `var` (Assign / Name) are allowed: slot names are renamed
//! to `$inl{tag}_…` so they cannot clash with the caller's mutable slots.

use lumia_core::{
    block_calls, count_ops, for_each_block_dfs, has_early_return, max_local_in_fun,
    rewrite_block_locals, Block, CoreFun, CoreModule, Local, Op, Value,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Max SSA ops in callee body (including nested) before we refuse to inline.
/// Sized to cover small helpers with a loop + vars (e.g. `isPrime`, `collatzSteps`).
const INLINE_MAX_OPS: usize = 32;

/// Cap nested expand depth (mutual a↔b both inlineable must not recurse forever).
const INLINE_MAX_EXPAND_DEPTH: usize = 8;

pub struct InlinePass;

impl InlinePass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        inline_module(module);
    }
}

fn inline_module(module: &mut CoreModule) {
    let inlineable: HashMap<String, CoreFun> = module
        .functions
        .iter()
        .filter(|f| is_inlineable(f))
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    if inlineable.is_empty() {
        return;
    }

    let mut name_tag = 0u32;
    for fun in &mut module.functions {
        let mut next = max_local_in_fun(fun).saturating_add(1);
        let mut expanding = HashSet::default();
        inline_block(
            &mut fun.body,
            &inlineable,
            &fun.name,
            &mut next,
            &mut name_tag,
            &mut expanding,
            0,
        );
    }
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
                                    params: vec![],
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
            Op::Effect { mut value } => {
                inline_value(
                    &mut value, inlineable, caller, next, name_tag, expanding, depth,
                );
                out.push(Op::Effect { value });
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
    collect_defined(&body, &mut defined);
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

fn collect_defined(block: &Block, defined: &mut HashSet<u32>) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                defined.insert(local.0);
                collect_defined_value(value, defined);
            }
            Op::Effect { value } => collect_defined_value(value, defined),
            _ => {}
        }
    }
}

fn collect_defined_value(value: &Value, defined: &mut HashSet<u32>) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_defined(then_block, defined);
            collect_defined(else_block, defined);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_defined(header, defined);
            collect_defined(body, defined);
            collect_defined(latch, defined);
        }
        Value::Lambda { params, body } => {
            for p in params {
                defined.insert(p.0);
            }
            collect_defined(body, defined);
        }
        _ => {}
    }
}

fn collect_slot_names(block: &Block, names: &mut HashSet<String>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            match op {
                Op::Assign { name, .. } => {
                    names.insert(name.clone());
                }
                Op::Let {
                    value: Value::Name(n),
                    ..
                }
                | Op::Effect {
                    value: Value::Name(n),
                } => {
                    names.insert(n.clone());
                }
                _ => {}
            }
        }
    });
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
            Op::Let { value, .. } | Op::Effect { value } => {
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
mod tests {
    use super::*;
    use lumia_core::{has_assign_or_name, Block, CoreFun, CoreModule, Op, Value};
    use lumia_syntax::BinOp;
    use lumia_ty::{Effect, Type};

    fn pure_add() -> CoreFun {
        // fun add(a, b) { a + b }
        CoreFun {
            name: "add".into(),
            params: vec![Local(0), Local(1)],
            param_names: vec!["a".into(), "b".into()],
            param_tys: vec![Type::Int, Type::Int],
            body: Block {
                params: vec![],
                ops: vec![Op::Let {
                    local: Local(2),
                    value: Value::Binary {
                        op: BinOp::Add,
                        left: Local(0),
                        right: Local(1),
                    },
                    pure_region: true,
                }],
                result: Some(Local(2)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        }
    }

    #[test]
    fn inlines_small_pure_call() {
        let mut module = CoreModule::with_functions(
            "M",
            vec![
                pure_add(),
                CoreFun {
                    name: "main".into(),
                    params: vec![],
                    param_names: vec![],
                    param_tys: vec![],
                    body: Block {
                        params: vec![],
                        ops: vec![
                            Op::Let {
                                local: Local(0),
                                value: Value::Int(1),
                                pure_region: true,
                            },
                            Op::Let {
                                local: Local(1),
                                value: Value::Int(2),
                                pure_region: true,
                            },
                            Op::Let {
                                local: Local(2),
                                value: Value::Call {
                                    fun: "add".into(),
                                    args: vec![Local(0), Local(1)],
                                },
                                pure_region: true,
                            },
                        ],
                        result: Some(Local(2)),
                    },
                    ret_ty: Type::Int,
                    effect: Effect::pure(),
                    is_main: true,
                    memo: None,
                    external: None,
                    foreign_abi: lumia_core::ForeignAbi::C,
                    escaping: HashSet::default(),
                    scheme_poly: false,
                    mono_of: None,
                },
            ],
        );
        inline_module(&mut module);
        let main = module.functions.iter().find(|f| f.name == "main").unwrap();
        let has_call = main.body.ops.iter().any(|op| {
            matches!(
                op,
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } if fun == "add"
            )
        });
        assert!(!has_call, "add call should be inlined");
        let has_add = main.body.ops.iter().any(|op| {
            matches!(
                op,
                Op::Let {
                    value: Value::Binary { op: BinOp::Add, .. },
                    ..
                }
            )
        });
        assert!(has_add, "inlined body should contain add");
    }

    #[test]
    fn inlines_var_slots_with_renamed_names() {
        // fun bump(n) { var x = n; x = x + 1; x }
        let bump = CoreFun {
            name: "bump".into(),
            params: vec![Local(0)],
            param_names: vec!["n".into()],
            param_tys: vec![Type::Int],
            body: Block {
                params: vec![],
                ops: vec![
                    Op::Assign {
                        name: "x".into(),
                        value: Local(0),
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Name("x".into()),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Binary {
                            op: BinOp::Add,
                            left: Local(1),
                            right: Local(2),
                        },
                        pure_region: true,
                    },
                    Op::Assign {
                        name: "x".into(),
                        value: Local(3),
                    },
                    Op::Let {
                        local: Local(4),
                        value: Value::Name("x".into()),
                        pure_region: true,
                    },
                ],
                result: Some(Local(4)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        };
        assert!(has_assign_or_name(&bump.body));
        assert!(is_inlineable(&bump));

        let mut module = CoreModule::with_functions(
            "M",
            vec![
                bump,
                CoreFun {
                    name: "main".into(),
                    params: vec![],
                    param_names: vec![],
                    param_tys: vec![],
                    body: Block {
                        params: vec![],
                        ops: vec![
                            Op::Let {
                                local: Local(0),
                                value: Value::Int(1),
                                pure_region: true,
                            },
                            Op::Assign {
                                name: "x".into(),
                                value: Local(0),
                            },
                            Op::Let {
                                local: Local(1),
                                value: Value::Int(41),
                                pure_region: true,
                            },
                            Op::Let {
                                local: Local(2),
                                value: Value::Call {
                                    fun: "bump".into(),
                                    args: vec![Local(1)],
                                },
                                pure_region: true,
                            },
                        ],
                        result: Some(Local(2)),
                    },
                    ret_ty: Type::Int,
                    effect: Effect::pure(),
                    is_main: true,
                    memo: None,
                    external: None,
                    foreign_abi: lumia_core::ForeignAbi::C,
                    escaping: HashSet::default(),
                    scheme_poly: false,
                    mono_of: None,
                },
            ],
        );
        inline_module(&mut module);
        let main = module.functions.iter().find(|f| f.name == "main").unwrap();
        let has_call = main.body.ops.iter().any(|op| {
            matches!(
                op,
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } if fun == "bump"
            )
        });
        assert!(!has_call, "bump should be inlined");
        // Caller keeps its own `x`; inlined body must use a tagged name.
        let assigns: Vec<&str> = main
            .body
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Assign { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert!(assigns.contains(&"x"));
        assert!(
            assigns.iter().any(|n| n.starts_with("$inl")),
            "inlined slots should be renamed, got {assigns:?}"
        );
    }

    #[test]
    fn skips_effectful() {
        let mut f = pure_add();
        f.effect = Effect::io();
        assert!(!is_inlineable(&f));
    }

    #[test]
    fn skips_early_return() {
        let f = CoreFun {
            name: "early".into(),
            params: vec![Local(0)],
            param_names: vec!["x".into()],
            param_tys: vec![Type::Int],
            body: Block {
                params: vec![],
                ops: vec![Op::Return { value: Local(0) }],
                result: None,
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        };
        assert!(!is_inlineable(&f));
    }

    #[test]
    fn mutual_inlineable_pair_does_not_hang() {
        // a calls b, b calls a — both small/pure. Expand stack must cut the cycle.
        let a = CoreFun {
            name: "a".into(),
            params: vec![Local(0)],
            param_names: vec!["x".into()],
            param_tys: vec![Type::Int],
            body: Block {
                params: vec![],
                ops: vec![Op::Let {
                    local: Local(1),
                    value: Value::Call {
                        fun: "b".into(),
                        args: vec![Local(0)],
                    },
                    pure_region: true,
                }],
                result: Some(Local(1)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        };
        let b = CoreFun {
            name: "b".into(),
            params: vec![Local(0)],
            param_names: vec!["x".into()],
            param_tys: vec![Type::Int],
            body: Block {
                params: vec![],
                ops: vec![Op::Let {
                    local: Local(1),
                    value: Value::Call {
                        fun: "a".into(),
                        args: vec![Local(0)],
                    },
                    pure_region: true,
                }],
                result: Some(Local(1)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        };
        let mut module = CoreModule::with_functions(
            "M",
            vec![
                a,
                b,
                CoreFun {
                    name: "main".into(),
                    params: vec![],
                    param_names: vec![],
                    param_tys: vec![],
                    body: Block {
                        params: vec![],
                        ops: vec![
                            Op::Let {
                                local: Local(0),
                                value: Value::Int(1),
                                pure_region: true,
                            },
                            Op::Let {
                                local: Local(1),
                                value: Value::Call {
                                    fun: "a".into(),
                                    args: vec![Local(0)],
                                },
                                pure_region: true,
                            },
                        ],
                        result: Some(Local(1)),
                    },
                    ret_ty: Type::Int,
                    effect: Effect::pure(),
                    is_main: true,
                    memo: None,
                    external: None,
                    foreign_abi: lumia_core::ForeignAbi::C,
                    escaping: HashSet::default(),
                    scheme_poly: false,
                    mono_of: None,
                },
            ],
        );
        inline_module(&mut module);
        let main = module.functions.iter().find(|f| f.name == "main").unwrap();
        let ops = count_ops(&main.body);
        assert!(ops < 64, "mutual inline must terminate, got {ops} ops");
    }
}
