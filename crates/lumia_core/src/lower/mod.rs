//! HIR → Core lowering.

mod ctx;
mod expr;

use crate::ir::{CoreFun, CoreModule, ForeignAbi};
use crate::lambda_lift::{fixup_closure_float_caps, lift_lambdas, refine_channel_elem_hint};
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
    lower_hir_with_schemes(module, fun_types, &HashMap::default(), &[])
}

/// Lower HIR using inferred types and HM schemes (scheme-driven monomorphization).
///
/// `type_at` is the zonked expression-type table from typecheck; used to stamp
/// ground builtin results (e.g. `Channel[T]`) onto Core values.
pub fn lower_hir_with_schemes(
    module: &HirModule,
    fun_types: &HashMap<String, Type>,
    fun_schemes: &HashMap<String, lumia_ty::Scheme>,
    type_at: &[(lumia_syntax::Span, Type)],
) -> CoreModule {
    let type_at: std::rc::Rc<[(lumia_syntax::Span, Type)]> = std::rc::Rc::from(type_at);
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
                    type_at.clone(),
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
                    foreign_abi: f
                        .external
                        .as_deref()
                        .map(ForeignAbi::from_symbol)
                        .unwrap_or_default(),
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
                    type_at.clone(),
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
                    foreign_abi: ForeignAbi::C,
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
            // Match ty: parametric slots only (recursive spines are `Self`).
            let total = sum_parametric_arity(a);
            (a.name.clone(), total)
        })
        .collect();
    let mut core = CoreModule {
        name: module.name.clone(),
        functions,
        hash_adts,
        trait_methods: module.trait_methods.clone(),
        adt_variant_names,
        sum_max_arity,
        channel_elem_hint: None,
        channel_elem_by_local: Default::default(),
        channel_elem_conflicts: Vec::new(),
    };
    lift_lambdas(&mut core);
    refine_channel_elem_hint(&mut core);
    directize_funref_calls(&mut core);
    // Num `a + b` is still Binary until here — rewrite to `__Num_T_add` Call
    // before mono so Float field products get `$…Float…` clones (codegen
    // override alone hits the unspecialized Int-body instance).
    resolve_trait_method_calls(&mut core);
    // Fixpoint: fixup lifts Float/Bool/String/Fun ABI on `__lam_*`; mono clones
    // HOF consumers (`unwrapOr` after `optionMap`, spawn join, …). One more
    // fixup after the last mono pass patches caps once `$Float` clones exist.
    for _ in 0..6 {
        fixup_closure_float_caps(&mut core);
        if !specialize_mono_calls(&mut core) {
            break;
        }
    }
    fixup_closure_float_caps(&mut core);
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

/// Count type parameters for a sum ADT: payload fields that are not recursive
/// spines (`Nat.S`, `UList.Cons` tail). Must stay aligned with
/// `lumia_ty::infer::module` classification.
fn sum_parametric_arity(adt: &lumia_hir::AdtDef) -> usize {
    if adt.name == "Option" || adt.name == "Result" {
        return adt.variants.iter().map(|v| v.arity).sum();
    }
    let arities: Vec<usize> = adt.variants.iter().map(|v| v.arity).collect();
    let has_nullary = arities.iter().any(|&a| a == 0);
    let only_nullary_unary = arities.iter().all(|&a| a <= 1);
    adt.variants
        .iter()
        .map(|v| {
            if v.arity == 0 {
                0
            } else if only_nullary_unary && has_nullary {
                0 // unary recursive spine
            } else if has_nullary && v.arity >= 2 {
                v.arity.saturating_sub(1) // last field recursive
            } else {
                v.arity
            }
        })
        .sum()
}
