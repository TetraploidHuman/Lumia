//! Function inlining (DESIGN §7.2 Inline + Specialization).
//!
//! Release-only: inline small pure leaf functions at direct call sites.
//! Skips `main`, `foreign`, memoized, recursive, and effectful callees.

use lumia_core::{
    max_local_in_fun, rewrite_block_locals, Block, CoreFun, CoreModule, Local, Op, Value,
};
use std::collections::{HashMap, HashSet};

/// Max SSA ops in callee body (including nested) before we refuse to inline.
const INLINE_MAX_OPS: usize = 24;

pub struct InlinePass;

impl crate::Pass for InlinePass {
    fn name(&self) -> &str {
        "inline"
    }
    fn run(&self, module: &mut CoreModule) {
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

    for fun in &mut module.functions {
        let mut next = max_local_in_fun(fun).saturating_add(1);
        inline_block(&mut fun.body, &inlineable, &fun.name, &mut next);
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
    if calls_name(&f.body, &f.name) {
        return false; // recursive
    }
    // Avoid inlining functions that assign to named slots (caller name clash).
    if has_assign_or_name(&f.body) {
        return false;
    }
    // Early return must stay in the callee; inlining would return from the caller.
    if has_early_return(&f.body) {
        return false;
    }
    true
}

fn has_early_return(block: &Block) -> bool {
    for op in &block.ops {
        match op {
            Op::Return { .. } => return true,
            Op::Let { value, .. } | Op::Effect { value, .. } if value_has_early_return(value) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn value_has_early_return(value: &Value) -> bool {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => has_early_return(then_block) || has_early_return(else_block),
        Value::Loop {
            header,
            body,
            latch,
        } => has_early_return(header) || has_early_return(body) || has_early_return(latch),
        Value::Lambda { body, .. } => has_early_return(body),
        _ => false,
    }
}

fn count_ops(block: &Block) -> usize {
    let mut n = block.ops.len();
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => n += count_ops_value(value),
            _ => {}
        }
    }
    n
}

fn count_ops_value(value: &Value) -> usize {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => count_ops(then_block) + count_ops(else_block),
        Value::Loop {
            header,
            body,
            latch,
        } => count_ops(header) + count_ops(body) + count_ops(latch),
        Value::Lambda { body, .. } => count_ops(body),
        _ => 0,
    }
}

fn calls_name(block: &Block, name: &str) -> bool {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } if value_calls_name(value, name) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn value_calls_name(value: &Value, name: &str) -> bool {
    match value {
        Value::Call { fun, .. } if fun == name => true,
        Value::If {
            then_block,
            else_block,
            ..
        } => calls_name(then_block, name) || calls_name(else_block, name),
        Value::Loop {
            header,
            body,
            latch,
        } => calls_name(header, name) || calls_name(body, name) || calls_name(latch, name),
        Value::Lambda { body, .. } => calls_name(body, name),
        _ => false,
    }
}

fn has_assign_or_name(block: &Block) -> bool {
    for op in &block.ops {
        match op {
            Op::Assign { .. } => return true,
            Op::Let { value, .. } | Op::Effect { value, .. } if value_has_assign_or_name(value) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn value_has_assign_or_name(value: &Value) -> bool {
    match value {
        Value::Name(_) => true,
        Value::If {
            then_block,
            else_block,
            ..
        } => has_assign_or_name(then_block) || has_assign_or_name(else_block),
        Value::Loop {
            header,
            body,
            latch,
        } => has_assign_or_name(header) || has_assign_or_name(body) || has_assign_or_name(latch),
        Value::Lambda { body, .. } => has_assign_or_name(body),
        _ => false,
    }
}

fn inline_block(
    block: &mut Block,
    inlineable: &HashMap<String, CoreFun>,
    caller: &str,
    next: &mut u32,
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
                inline_value(&mut value, inlineable, caller, next);
                if let Value::Call { fun, args } = &value {
                    if fun != caller {
                        if let Some(callee) = inlineable.get(fun) {
                            if args.len() == callee.params.len() {
                                let (prelude, result) = materialize_inline(callee, args, next);
                                out.extend(prelude);
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
                inline_value(&mut value, inlineable, caller, next);
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
) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            inline_block(then_block, inlineable, caller, next);
            inline_block(else_block, inlineable, caller, next);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            inline_block(header, inlineable, caller, next);
            inline_block(body, inlineable, caller, next);
            inline_block(latch, inlineable, caller, next);
        }
        Value::Lambda { body, .. } => inline_block(body, inlineable, caller, next),
        _ => {}
    }
}

fn materialize_inline(callee: &CoreFun, args: &[Local], next: &mut u32) -> (Vec<Op>, Local) {
    let mut body = callee.body.clone();
    let mut remap: HashMap<u32, u32> = HashMap::new();

    // Map params → actuals (no new locals).
    for (p, a) in callee.params.iter().zip(args.iter()) {
        remap.insert(p.0, a.0);
    }

    // Fresh ids for every other local defined in the callee.
    let mut defined: HashSet<u32> = HashSet::new();
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

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_core::{Block, CoreFun, CoreModule, Op, Value};
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
            escaping: std::collections::HashSet::new(),
            scheme_poly: false,
        }
    }

    #[test]
    fn inlines_small_pure_call() {
        let mut module = CoreModule {
            name: "M".into(),
            functions: vec![
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
                    escaping: std::collections::HashSet::new(),
                    scheme_poly: false,
                },
            ],
            hash_adts: std::collections::HashSet::new(),
            trait_methods: std::collections::HashMap::new(),
        };
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
            escaping: std::collections::HashSet::new(),
            scheme_poly: false,
        };
        assert!(!is_inlineable(&f));
    }
}
