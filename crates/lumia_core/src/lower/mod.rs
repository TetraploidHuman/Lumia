//! HIR → Core lowering.

mod ctx;
mod expr;

use crate::ir::{CoreFun, CoreModule};
use crate::lambda_lift::{fixup_closure_float_caps, lift_lambdas};
use crate::mono::{
    directize_funref_calls, ensure_trait_method_stubs, resolve_trait_method_calls,
    specialize_mono_calls,
};
use ctx::CoreLowerCtx;
use expr::lower_expr_block;
use lumia_hir::{Item, Module as HirModule};
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub fn lower_hir(module: &HirModule, fun_types: &HashMap<String, Type>) -> CoreModule {
    lower_hir_with_schemes(module, fun_types, &HashMap::default())
}

/// Lower HIR using inferred types and HM schemes (scheme-driven monomorphization).
pub fn lower_hir_with_schemes(
    module: &HirModule,
    fun_types: &HashMap<String, Type>,
    fun_schemes: &HashMap<String, lumia_ty::Scheme>,
) -> CoreModule {
    let toplevel_funs: HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fun(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect();
    let toplevel_vals: HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Val { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    let trait_method_names: HashSet<String> = module
        .trait_methods
        .keys()
        .map(|(_, m)| m.clone())
        .collect();
    let io_funs: HashSet<String> = fun_types
        .iter()
        .filter_map(|(n, ty)| match ty {
            Type::Fun(_, _, e) if e.has_io() => Some(n.clone()),
            _ => None,
        })
        .collect();
    let mut functions = vec![];
    for item in &module.items {
        match item {
            Item::Fun(f) => {
                let mut ctx = CoreLowerCtx::new(
                    toplevel_funs.clone(),
                    toplevel_vals.clone(),
                    trait_method_names.clone(),
                    io_funs.clone(),
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
                    escaping: HashSet::default(),
                    scheme_poly,
                    mono_of: None,
                });
            }
            Item::Val { name, body, ty: _ } => {
                // Module-level `val` → zero-arg getter `__val_<name>` (pure).
                // Ret type must match inference so codegen roots heap returns.
                let getter = format!("__val_{name}");
                let ret_ty = match fun_types.get(&getter).or_else(|| fun_types.get(name)) {
                    Some(Type::Fun(_, r, _)) => (**r).clone(),
                    Some(t) => t.clone(),
                    None => Type::Int,
                };
                let mut ctx = CoreLowerCtx::new(
                    toplevel_funs.clone(),
                    toplevel_vals.clone(),
                    trait_method_names.clone(),
                    io_funs.clone(),
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
                    escaping: HashSet::default(),
                    scheme_poly,
                    mono_of: None,
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
    let mut adt_variant_names: HashMap<String, Vec<String>> = HashMap::default();
    for adt in &module.adts {
        let mut names = vec![String::new(); adt.variants.len()];
        for v in &adt.variants {
            let idx = v.tag as usize;
            if idx >= names.len() {
                names.resize(idx + 1, String::new());
            }
            names[idx] = v.name.clone();
        }
        adt_variant_names.insert(adt.name.clone(), names);
    }
    for prod in &module.products {
        // Products are tag-0 payloads; print the type name.
        adt_variant_names.insert(prod.name.clone(), vec![prod.name.clone()]);
    }
    let sum_max_arity: HashMap<String, usize> = module
        .adts
        .iter()
        .map(|a| {
            let max = a.variants.iter().map(|v| v.arity).max().unwrap_or(0);
            (a.name.clone(), max)
        })
        .collect();
    let mut core = CoreModule {
        name: module.name.clone(),
        functions,
        hash_adts,
        trait_methods: module.trait_methods.clone(),
        adt_variant_names,
        sum_max_arity,
    };
    lift_lambdas(&mut core);
    directize_funref_calls(&mut core);
    specialize_mono_calls(&mut core);
    fixup_closure_float_caps(&mut core);
    resolve_trait_method_calls(&mut core);
    ensure_trait_method_stubs(&mut core);
    core
}

fn type_is_open(t: &Type) -> bool {
    match t {
        Type::Var(_) => true,
        Type::Fun(ps, r, _) => ps.iter().any(type_is_open) || type_is_open(r),
        Type::List(e) | Type::Set(e) | Type::Task(e) | Type::Channel(e) => type_is_open(e),
        Type::Map(k, v) => type_is_open(k) || type_is_open(v),
        Type::Tuple(ts) | Type::TuplePrefix(ts) | Type::Adt { params: ts, .. } => {
            ts.iter().any(type_is_open)
        }
        _ => false,
    }
}
