//! HIR → Core lowering.

use crate::ir::{
    AdtRepr, Block, CoreFun, CoreModule, ListRepr, Local, MapRepr, Op, SetRepr, Value,
};
use crate::lambda_lift::lift_lambdas;
use crate::mono::{
    directize_funref_calls, ensure_trait_method_stubs, resolve_trait_method_calls,
    specialize_mono_calls,
};
use lumia_hir::{Builtin, Expr as HirExpr, Item, Module as HirModule};
use lumia_ty::{Effect, Type};
use std::collections::{HashMap, HashSet};

struct LowerCtx {
    next: u32,
    name_to_local: HashMap<String, Local>,
    mutables: std::collections::HashSet<String>,
    toplevel_funs: std::collections::HashSet<String>,
    toplevel_vals: std::collections::HashSet<String>,
    /// Short trait-method names left unresolved until post-mono resolve.
    trait_method_names: std::collections::HashSet<String>,
}

impl LowerCtx {
    fn new(
        toplevel_funs: std::collections::HashSet<String>,
        toplevel_vals: std::collections::HashSet<String>,
        trait_method_names: std::collections::HashSet<String>,
    ) -> Self {
        Self {
            next: 0,
            name_to_local: HashMap::new(),
            mutables: std::collections::HashSet::new(),
            toplevel_funs,
            toplevel_vals,
            trait_method_names,
        }
    }

    fn fresh(&mut self) -> Local {
        let l = Local(self.next);
        self.next += 1;
        l
    }

    fn bind_name(&mut self, name: String, local: Local) {
        self.name_to_local.insert(name, local);
    }

    fn bind_mutable(&mut self, name: String, local: Local) {
        self.mutables.insert(name.clone());
        self.bind_name(name, local);
    }

    /// Snapshot of name bindings (not `next` — locals stay unique across scopes).
    fn save_bindings(&self) -> (HashMap<String, Local>, HashSet<String>) {
        (self.name_to_local.clone(), self.mutables.clone())
    }

    fn restore_bindings(&mut self, saved: (HashMap<String, Local>, HashSet<String>)) {
        self.name_to_local = saved.0;
        self.mutables = saved.1;
    }
}

pub fn lower_hir(module: &HirModule, fun_types: &HashMap<String, Type>) -> CoreModule {
    lower_hir_with_schemes(module, fun_types, &HashMap::new())
}

/// Lower HIR using inferred types and HM schemes (scheme-driven monomorphization).
pub fn lower_hir_with_schemes(
    module: &HirModule,
    fun_types: &HashMap<String, Type>,
    fun_schemes: &HashMap<String, lumia_ty::Scheme>,
) -> CoreModule {
    let toplevel_funs: std::collections::HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fun(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect();
    let toplevel_vals: std::collections::HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Val { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    let trait_method_names: std::collections::HashSet<String> = module
        .trait_methods
        .keys()
        .map(|(_, m)| m.clone())
        .collect();
    let mut functions = vec![];
    for item in &module.items {
        match item {
            Item::Fun(f) => {
                let mut ctx = LowerCtx::new(
                    toplevel_funs.clone(),
                    toplevel_vals.clone(),
                    trait_method_names.clone(),
                );
                let mut params = vec![];
                for p in &f.params {
                    let l = ctx.fresh();
                    ctx.bind_name(p.clone(), l);
                    params.push(l);
                }
                let (body, _) = lower_expr_block(&mut ctx, &f.body);
                let (ret_ty, effect, param_tys) = match fun_types.get(&f.name) {
                    Some(Type::Fun(ps, r, e)) => ((**r).clone(), *e, ps.clone()),
                    _ => (
                        Type::Unit,
                        if f.is_main {
                            Effect::io()
                        } else {
                            Effect::pure()
                        },
                        vec![Type::Int; f.params.len()],
                    ),
                };
                let scheme_poly = fun_schemes
                    .get(&f.name)
                    .map(|s| s.needs_mono())
                    .unwrap_or_else(|| {
                        type_is_open(&Type::Fun(
                            param_tys.clone(),
                            Box::new(ret_ty.clone()),
                            effect,
                        ))
                    });
                functions.push(CoreFun {
                    name: f.name.clone(),
                    params,
                    param_names: f.params.clone(),
                    param_tys,
                    body,
                    ret_ty,
                    effect,
                    is_main: f.is_main,
                    memo: None,
                    external: f.external.clone(),
                    escaping: HashSet::new(),
                    scheme_poly,
                });
            }
            Item::Val { name, body } => {
                // Module-level `val` → zero-arg getter `__val_<name>` (pure).
                // Ret type must match inference so codegen roots heap returns.
                let getter = format!("__val_{name}");
                let ret_ty = match fun_types.get(&getter).or_else(|| fun_types.get(name)) {
                    Some(Type::Fun(_, r, _)) => (**r).clone(),
                    Some(t) => t.clone(),
                    None => Type::Int,
                };
                let mut ctx = LowerCtx::new(
                    toplevel_funs.clone(),
                    toplevel_vals.clone(),
                    trait_method_names.clone(),
                );
                let (body, _) = lower_expr_block(&mut ctx, body);
                // Getters are nullary; poly lives on the value's Fun scheme / lifted body.
                let scheme_poly = fun_schemes
                    .get(name)
                    .map(|s| s.needs_mono())
                    .unwrap_or(false);
                functions.push(CoreFun {
                    name: getter,
                    params: vec![],
                    param_names: vec![],
                    param_tys: vec![],
                    body,
                    ret_ty,
                    effect: Effect::pure(),
                    is_main: false,
                    memo: None,
                    external: None,
                    escaping: HashSet::new(),
                    scheme_poly,
                });
            }
        }
    }
    let hash_adts: HashSet<String> = module
        .instances
        .iter()
        .filter(|(tr, _)| tr == "Hash")
        .map(|(_, ty)| ty.clone())
        .collect();
    let mut core = CoreModule {
        name: module.name.clone(),
        functions,
        hash_adts,
        trait_methods: module.trait_methods.clone(),
    };
    lift_lambdas(&mut core);
    directize_funref_calls(&mut core);
    specialize_mono_calls(&mut core);
    resolve_trait_method_calls(&mut core);
    ensure_trait_method_stubs(&mut core);
    core
}

fn type_is_open(t: &Type) -> bool {
    match t {
        Type::Var(_) => true,
        Type::Fun(ps, r, _) => ps.iter().any(type_is_open) || type_is_open(r),
        Type::List(e) | Type::Set(e) => type_is_open(e),
        Type::Map(k, v) => type_is_open(k) || type_is_open(v),
        Type::Tuple(ts) | Type::TuplePrefix(ts) | Type::Adt { params: ts, .. } => {
            ts.iter().any(type_is_open)
        }
        _ => false,
    }
}

fn lower_expr_block(ctx: &mut LowerCtx, expr: &HirExpr) -> (Block, Option<Local>) {
    let mut ops = vec![];
    let result = lower_expr(ctx, expr, &mut ops, true);
    (
        Block {
            params: vec![],
            ops,
            result,
        },
        result,
    )
}

fn lower_expr(
    ctx: &mut LowerCtx,
    expr: &HirExpr,
    ops: &mut Vec<Op>,
    pure_region: bool,
) -> Option<Local> {
    match expr {
        HirExpr::Int(n, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Int(*n),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Float(n, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Float(*n),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Bool(b, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Bool(*b),
                pure_region,
            });
            Some(l)
        }
        HirExpr::String(s, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::String(s.clone()),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Char(c, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Char(*c),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Unit(_) => None,
        HirExpr::Var(name, _) => {
            if ctx.mutables.contains(name) {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::Name(name.clone()),
                    pure_region,
                });
                Some(l)
            } else if let Some(l) = ctx.name_to_local.get(name) {
                Some(*l)
            } else if ctx.toplevel_funs.contains(name) {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::FunRef(name.clone()),
                    pure_region,
                });
                Some(l)
            } else if ctx.toplevel_vals.contains(name) {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::Call {
                        fun: format!("__val_{name}"),
                        args: vec![],
                    },
                    pure_region,
                });
                Some(l)
            } else {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::Name(name.clone()),
                    pure_region,
                });
                Some(l)
            }
        }
        HirExpr::Let {
            name,
            value,
            body,
            mutable,
            ..
        } => {
            let v = lower_expr(ctx, value, ops, pure_region);
            let saved = ctx.save_bindings();
            if let Some(l) = v {
                if *mutable {
                    ctx.bind_mutable(name.clone(), l);
                    ops.push(Op::Assign {
                        name: name.clone(),
                        value: l,
                    });
                } else {
                    // `val` may shadow an outer `var` for the duration of `body`.
                    ctx.mutables.remove(name);
                    ctx.bind_name(name.clone(), l);
                }
            }
            let result = lower_expr(ctx, body, ops, pure_region);
            ctx.restore_bindings(saved);
            result
        }
        HirExpr::Assign { name, value, .. } => {
            let v = match lower_expr(ctx, value, ops, pure_region) {
                Some(l) => l,
                None => {
                    // Unit RHS: materialize a 0 local so assign never panics.
                    let l = ctx.fresh();
                    ops.push(Op::Let {
                        local: l,
                        value: Value::Unit,
                        pure_region,
                    });
                    l
                }
            };
            if ctx.mutables.contains(name) {
                ops.push(Op::Assign {
                    name: name.clone(),
                    value: v,
                });
            } else {
                // Immutable binding: ty rejects user assigns; do not mutate an
                // outer `var` shadowed by `val` (and do not mark name mutable).
                ctx.bind_name(name.clone(), v);
            }
            None
        }
        HirExpr::Binary {
            op, left, right, ..
        } => {
            let l = lower_expr(ctx, left, ops, pure_region)
                .expect("ICE: binary operand lowered to Unit; type checker should reject");
            let r = lower_expr(ctx, right, ops, pure_region)
                .expect("ICE: binary operand lowered to Unit; type checker should reject");
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Binary {
                    op: *op,
                    left: l,
                    right: r,
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Unary { op, expr, .. } => {
            let o = lower_expr(ctx, expr, ops, pure_region)
                .expect("ICE: unary operand lowered to Unit; type checker should reject");
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Unary {
                    op: *op,
                    operand: o,
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Call { callee, args, .. } => {
            let mut arg_locals = vec![];
            for a in args {
                if let Some(l) = lower_expr(ctx, a, ops, pure_region) {
                    arg_locals.push(l);
                }
            }
            let dest = ctx.fresh();
            let fun_name = match callee.as_ref() {
                HirExpr::Var(n, _) => Some(n.as_str()),
                _ => None,
            };
            let value = match fun_name {
                Some("listOf") => Value::AllocList {
                    elems: arg_locals,
                    repr: ListRepr::HeapList,
                },
                Some("setOf") => Value::AllocSet {
                    elems: arg_locals,
                    repr: SetRepr::HeapSet,
                },
                Some("mapOf") => Value::AllocMap {
                    flat_pairs: arg_locals,
                    repr: MapRepr::HashOrdered,
                },
                Some(n) if ctx.toplevel_funs.contains(n) || ctx.trait_method_names.contains(n) => {
                    Value::Call {
                        fun: n.to_string(),
                        args: arg_locals,
                    }
                }
                _ => {
                    // Local / expression callee → indirect call (first-class fn).
                    let cal = lower_expr(ctx, callee, ops, pure_region).unwrap_or_else(|| {
                        let l = ctx.fresh();
                        ops.push(Op::Let {
                            local: l,
                            value: Value::Int(0),
                            pure_region,
                        });
                        l
                    });
                    Value::IndirectCall {
                        callee: cal,
                        args: arg_locals,
                    }
                }
            };
            ops.push(Op::Let {
                local: dest,
                value,
                pure_region,
            });
            Some(dest)
        }
        HirExpr::BuiltinCall { name, args, .. } => {
            let mut arg_locals = vec![];
            // Product field checks carry an expected-ADT name as a 3rd HIR arg;
            // Core/runtime only need (obj, index).
            let use_args: &[HirExpr] = if matches!(name, Builtin::AdtField) && args.len() == 3 {
                &args[..2]
            } else {
                args
            };
            for a in use_args {
                if let Some(l) = lower_expr(ctx, a, ops, true) {
                    arg_locals.push(l);
                }
            }
            let is_io = matches!(
                name,
                Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr | Builtin::ReadStdin
            );
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Builtin {
                    name: *name,
                    args: arg_locals,
                },
                pure_region: !is_io,
            });
            Some(dest)
        }
        HirExpr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let c = lower_expr(ctx, cond, ops, pure_region)
                .expect("ICE: if condition lowered to Unit; type checker should reject");
            // Isolate arm bindings so `val`/`var` inside then/else cannot leak.
            let saved = ctx.save_bindings();
            let (then_block, _) = lower_expr_block(ctx, then_branch);
            ctx.restore_bindings(saved.clone());
            let (else_block, _) = lower_expr_block(ctx, else_branch);
            ctx.restore_bindings(saved);
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::If {
                    cond: c,
                    then_block: Box::new(then_block),
                    else_block: Box::new(else_block),
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Loop {
            cond, body, step, ..
        } => {
            // Loop header/body/latch share outer bindings but must not leak
            // names introduced only inside those blocks.
            let saved = ctx.save_bindings();
            let (header, _) = lower_expr_block(ctx, cond);
            ctx.restore_bindings(saved.clone());
            let (body_block, _) = lower_expr_block(ctx, body);
            ctx.restore_bindings(saved.clone());
            let latch = if let Some(s) = step {
                let (b, _) = lower_expr_block(ctx, s);
                b
            } else {
                Block {
                    params: vec![],
                    ops: vec![],
                    result: None,
                }
            };
            ctx.restore_bindings(saved);
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Loop {
                    header: Box::new(header),
                    body: Box::new(body_block),
                    latch: Box::new(latch),
                },
                pure_region: false,
            });
            Some(dest)
        }
        HirExpr::Break(_) => {
            ops.push(Op::Break);
            None
        }
        HirExpr::Continue(_) => {
            ops.push(Op::Continue);
            None
        }
        HirExpr::Return { value, .. } => {
            if let Some(v) = lower_expr(ctx, value, ops, pure_region) {
                ops.push(Op::Return { value: v });
            }
            None
        }
        HirExpr::Alt { .. } => {
            panic!("lumia: Alt reached Core lower; expected typecheck desugar");
        }
        HirExpr::AdtNew {
            adt_name,
            tag,
            args,
            ..
        } => {
            let mut fields = vec![];
            for a in args {
                if let Some(l) = lower_expr(ctx, a, ops, pure_region) {
                    fields.push(l);
                }
            }
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::AllocAdt {
                    adt_name: adt_name.clone(),
                    tag: *tag,
                    fields,
                    repr: AdtRepr::HeapAdt,
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Seq { stmts, .. } => {
            let mut last = None;
            for s in stmts {
                last = lower_expr(ctx, s, ops, pure_region);
            }
            last
        }
        HirExpr::Lambda { params, body, .. } => {
            let mut inner = LowerCtx {
                next: ctx.next,
                name_to_local: ctx.name_to_local.clone(),
                mutables: ctx.mutables.clone(),
                toplevel_funs: ctx.toplevel_funs.clone(),
                toplevel_vals: ctx.toplevel_vals.clone(),
                trait_method_names: ctx.trait_method_names.clone(),
            };
            let mut pls = vec![];
            for p in params {
                let l = inner.fresh();
                inner.bind_name(p.clone(), l);
                pls.push(l);
            }
            let (block, _) = lower_expr_block(&mut inner, body);
            ctx.next = inner.next;
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Lambda {
                    params: pls,
                    body: Box::new(block),
                },
                pure_region,
            });
            Some(dest)
        }
    }
}
